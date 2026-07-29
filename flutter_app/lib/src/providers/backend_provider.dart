import 'package:flutter_riverpod/flutter_riverpod.dart';
import '../backend/backend_service.dart';
import '../backend/models.dart';

/// 后端服务单例
final backendServiceProvider = Provider<BackendService>((ref) {
  final service = BackendService();
  ref.onDispose(() => service.dispose());
  return service;
});

/// 后端初始化状态
final backendInitializedProvider = FutureProvider<bool>((ref) async {
  final service = ref.watch(backendServiceProvider);
  await service.initialize();
  return true;
});

/// 原始事件流
final backendEventStreamProvider = StreamProvider<BackendEvent>((ref) {
  final service = ref.watch(backendServiceProvider);
  return service.eventStream;
});

/// 串口列表
final portListProvider = StreamProvider<List<PortDescriptor>>((ref) async* {
  final service = ref.watch(backendServiceProvider);
  final initialized = await ref.watch(backendInitializedProvider.future);
  if (initialized) {
    yield service.getPorts();
    await for (final event in service.eventStream) {
      if (event.type == 'port_list') {
        yield service.getPorts();
      }
    }
  }
});

/// 后端状态
final backendStatusProvider = StreamProvider<BackendStatus>((ref) async* {
  final service = ref.watch(backendServiceProvider);
  final initialized = await ref.watch(backendInitializedProvider.future);
  if (initialized) {
    yield service.getStatus();
    await for (final event in service.eventStream) {
      if (event.type == 'serial_open' ||
          event.type == 'serial_close' ||
          event.type == 'recorder_status') {
        yield service.getStatus();
      }
    }
  }
});

/// 插件列表
final pluginListProvider = StreamProvider<List<PluginSummary>>((ref) async* {
  final service = ref.watch(backendServiceProvider);
  final initialized = await ref.watch(backendInitializedProvider.future);
  if (initialized) {
    yield service.getPlugins();
    await for (final event in service.eventStream) {
      if (event.type == 'plugin_list' || event.type == 'plugin_diagnostics') {
        yield service.getPlugins();
      }
    }
  }
});

/// 日志条目
final logEntriesProvider =
    StateNotifierProvider<LogEntriesNotifier, List<LogEntry>>((ref) {
      return LogEntriesNotifier(ref);
    });

class LogEntriesNotifier extends StateNotifier<List<LogEntry>> {
  final Ref _ref;
  final int _maxEntries = 50000;

  LogEntriesNotifier(this._ref) : super([]) {
    _listen();
  }

  void _listen() {
    _ref.listen(backendEventStreamProvider, (prev, next) {
      next.whenData((event) {
        if (event.type == 'log') {
          final level = _parseLevel(event.data['level']);
          final message = event.data['message'] as String? ?? '';
          final source = event.data['source'] as String? ?? '';
          add(
            LogEntry(
              level: level,
              source: source,
              message: message,
              timestamp: 0,
            ),
          );
        }
      });
    });
  }

  LogLevel _parseLevel(dynamic level) {
    if (level == null) return LogLevel.info;
    final s = level.toString();
    if (s == 'error') return LogLevel.error;
    if (s == 'warn') return LogLevel.warn;
    if (s == 'debug') return LogLevel.debug;
    if (s == 'trace') return LogLevel.trace;
    return LogLevel.info;
  }

  void add(LogEntry entry) {
    state = [...state, entry];
    if (state.length > _maxEntries) {
      state = state.sublist(state.length - _maxEntries);
    }
  }

  void clear() => state = [];
}
