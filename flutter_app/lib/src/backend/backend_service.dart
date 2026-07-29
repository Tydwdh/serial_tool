import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';
import 'ffi_bindings.dart';
import 'models.dart';

/// BackendService — 封装所有 Rust 后端 FFI 调用。
class BackendService {
  late final BackendBindings _bindings;
  bool _initialized = false;
  Timer? _pollTimer;

  final _eventController = StreamController<BackendEvent>.broadcast();
  Stream<BackendEvent> get eventStream => _eventController.stream;

  /// 待处理事件队列（由 Rust 回调填充，poll 循环消费）
  static const _maxPendingEvents = 20000;
  static const _maxEventsPerFrame = 1000;
  static final ListQueue<String> _pendingEvents = ListQueue<String>();
  static int _droppedEvents = 0;

  static void pushEvent(String json) {
    if (_pendingEvents.length >= _maxPendingEvents) {
      _pendingEvents.removeFirst();
      _droppedEvents++;
    }
    _pendingEvents.addLast(json);
  }

  Future<void> initialize() async {
    if (_initialized) return;
    _bindings = BackendBindings();

    final callback = Pointer.fromFunction<EventCallbackNative>(_eventCallback);
    _bindings.wbSetEventCallback(callback, 0);

    final appDir = await _getAppDir();
    final appDirPtr = appDir.toNativeUtf8();
    try {
      final result = _bindings.wbCreate(appDirPtr);
      if (result != 0) throw Exception('后端初始化失败');
      _initialized = true;

      _pollTimer = Timer.periodic(const Duration(milliseconds: 16), (_) {
        _bindings.wbPollEvents();
        _drainPendingEvents();
      });
    } finally {
      calloc.free(appDirPtr);
    }
  }

  void _drainPendingEvents() {
    if (_pendingEvents.isEmpty) return;
    if (_droppedEvents > 0) {
      _eventController.add(
        BackendEvent(
          type: 'notification',
          data: {
            'level': 'warning',
            'message': '前端事件积压，已丢弃 $_droppedEvents 条较早数据',
          },
        ),
      );
      _droppedEvents = 0;
    }
    final count = _pendingEvents.length.clamp(0, _maxEventsPerFrame);
    for (var index = 0; index < count; index++) {
      final json = _pendingEvents.removeFirst();
      try {
        _eventController.add(BackendEvent.fromJson(json));
      } catch (e) {
        // 跳过解析失败的事件，继续处理后续事件
      }
    }
  }

  Future<String> _getAppDir() async {
    // Keep the desktop runner dependency-free: Flutter's Windows plugin
    // symlinks require Developer Mode on some machines. APPDATA is the
    // conventional per-user persistent location for a Windows app.
    final base =
        Platform.environment['APPDATA'] ??
        Platform.environment['LOCALAPPDATA'] ??
        Directory.current.path;
    final directory = Directory(
      '$base${Platform.pathSeparator}hardware_workbench',
    );
    if (!await directory.exists()) await directory.create(recursive: true);
    return directory.path;
  }

  void dispose() {
    _pollTimer?.cancel();
    _pollTimer = null;
    if (_initialized) {
      _bindings.wbDestroy();
      _initialized = false;
    }
    _eventController.close();
  }

  /// 执行命令并解析返回的 JSON。
  /// 如果返回 `{"error": "..."}` 则抛出 Exception。
  Map<String, dynamic> _executeCommand(
    String cmd, [
    Map<String, dynamic>? params,
  ]) {
    final cmdPtr = cmd.toNativeUtf8();
    final paramsJson = params != null ? jsonEncode(params) : '';
    final paramsPtr = paramsJson.toNativeUtf8();
    try {
      final resultPtr = _bindings.wbCmd(cmdPtr, paramsPtr);
      if (resultPtr == nullptr) throw Exception('命令 "$cmd" 返回空指针');
      final result = resultPtr.toDartString();
      _bindings.wbFreeString(resultPtr);
      final parsed = jsonDecode(result) as Map<String, dynamic>;
      if (parsed.containsKey('error')) {
        throw Exception('命令 "$cmd" 失败: ${parsed['error']}');
      }
      return parsed;
    } finally {
      calloc.free(cmdPtr);
      calloc.free(paramsPtr);
    }
  }

  List<PortDescriptor> getPorts() {
    final resultPtr = _bindings.wbGetPorts();
    if (resultPtr == nullptr) return [];
    final result = resultPtr.toDartString();
    _bindings.wbFreeString(resultPtr);
    try {
      final list = jsonDecode(result) as List;
      return list.map((e) => PortDescriptor.fromJson(e)).toList();
    } catch (_) {
      return [];
    }
  }

  List<PluginSummary> getPlugins() {
    final resultPtr = _bindings.wbGetPlugins();
    if (resultPtr == nullptr) return [];
    final result = resultPtr.toDartString();
    _bindings.wbFreeString(resultPtr);
    try {
      final list = jsonDecode(result) as List;
      return list.map((e) => PluginSummary.fromJson(e)).toList();
    } catch (_) {
      return [];
    }
  }

  BackendStatus getStatus() {
    final resultPtr = _bindings.wbGetStatus();
    if (resultPtr == nullptr) return BackendStatus();
    final result = resultPtr.toDartString();
    _bindings.wbFreeString(resultPtr);
    try {
      return BackendStatus.fromJson(jsonDecode(result));
    } catch (_) {
      return BackendStatus();
    }
  }

  // ── 命令封装（抛出异常时由调用方处理） ──

  void openPort(String portName) =>
      _executeCommand('open_port', {'port': portName});
  void closePort(String portName) =>
      _executeCommand('close_port', {'port': portName});
  void sendData(String port, String data, {bool hex = false}) =>
      _executeCommand('send_data', {'port': port, 'data': data, 'hex': hex});
  void refreshPorts() => _executeCommand('refresh_ports');
  void toggleRecording() => _executeCommand('toggle_recording');
  void setTerminalPaused(bool paused) =>
      _executeCommand('set_terminal_paused', {'paused': paused});
  void enablePlugin(String id) => _executeCommand('enable_plugin', {'id': id});
  void disablePlugin(String id) =>
      _executeCommand('disable_plugin', {'id': id});
  List<Map<String, dynamic>> fetchMarketplace([String? url]) {
    final result = _executeCommand(
      'marketplace_fetch',
      url == null ? null : {'url': url},
    );
    return (result['plugins'] as List? ?? const [])
        .whereType<Map>()
        .map((plugin) => Map<String, dynamic>.from(plugin))
        .toList(growable: false);
  }

  void installMarketplacePlugin(Map<String, dynamic> plugin) =>
      _executeCommand('marketplace_install', {'plugin': plugin});
  void uninstallMarketplacePlugin(String id) =>
      _executeCommand('marketplace_uninstall', {'id': id});
  void setBaudRate(String rate) =>
      _executeCommand('set_baud_rate', {'rate': rate});
  void setSerialConfig(Map<String, dynamic> config) =>
      _executeCommand('set_serial_config', config);
  void setSendConfig(Map<String, dynamic> config) =>
      _executeCommand('set_send_config', config);
  Map<String, dynamic> getConfig() => _executeCommand('get_config');
  void setSelectedPort(String port) =>
      _executeCommand('set_selected_port', {'port': port});
  void setDtr(String port, bool value) =>
      _executeCommand('set_dtr', {'port': port, 'value': value});
  void setRts(String port, bool value) =>
      _executeCommand('set_rts', {'port': port, 'value': value});
  void saveConfig() => _executeCommand('save_config');
  void loadConfig() => _executeCommand('load_config');
  void setLayout(Map<String, dynamic> layout) =>
      _executeCommand('set_layout', {'layout': layout});
  void replayLoad(String path) =>
      _executeCommand('replay_load', {'path': path});
  void replayPlay() => _executeCommand('replay_play');
  void replayPause() => _executeCommand('replay_pause');
  void replayStop() => _executeCommand('replay_stop');
  void replayStepForward() => _executeCommand('replay_step_forward');
  void replayStepBackward() => _executeCommand('replay_step_backward');
  void replaySeek(int positionMs) =>
      _executeCommand('replay_seek', {'position_ms': positionMs});
  void replaySetSpeed(double speed) =>
      _executeCommand('replay_set_speed', {'speed': speed});
  void replaySetPolicy(String policy) =>
      _executeCommand('replay_set_policy', {'policy': policy});
  void replayAddBookmark([String? name]) => _executeCommand(
    'replay_add_bookmark',
    name == null ? null : {'name': name},
  );
  void replayRemoveBookmark(int positionMs) =>
      _executeCommand('replay_remove_bookmark', {'position_ms': positionMs});
  List<Map<String, dynamic>> replayFiles() {
    final result = _executeCommand('replay_list_files');
    return (result['files'] as List? ?? const [])
        .whereType<Map>()
        .map((file) => Map<String, dynamic>.from(file))
        .toList(growable: false);
  }

  String? pickReplayFile() =>
      _executeCommand('replay_pick_file')['path'] as String?;
  String? pickTerminalExportPath(String suggestedName) =>
      _executeCommand('pick_terminal_export_path', {
            'suggested_name': suggestedName,
          })['path']
          as String?;

  void submitDynamicForm(String panelId, Map<String, dynamic> values) =>
      _executeCommand('dynamic_form_changed', {
        'panel_id': panelId,
        'values': values,
      });
  void triggerDynamicFormAction({
    required String panelId,
    required String fieldId,
    String? action,
    required Map<String, dynamic> values,
  }) => _executeCommand('dynamic_form_action', {
    'panel_id': panelId,
    'field_id': fieldId,
    'action': ?action,
    'values': values,
  });
  String? pickDynamicFormFile({
    required String pluginId,
    required String title,
    required List<dynamic> filters,
  }) =>
      _executeCommand('dynamic_form_pick_file', {
            'plugin_id': pluginId,
            'title': title,
            'filters': filters,
          })['path']
          as String?;
}

// 全局事件回调（C ABI — 必须是顶级函数）
void _eventCallback(Pointer<Utf8> jsonPtr, int userData) {
  if (jsonPtr == nullptr) return;
  BackendService.pushEvent(jsonPtr.toDartString());
}
