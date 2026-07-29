import 'dart:ffi';
import 'dart:io';
import 'package:ffi/ffi.dart';

/// FFI type definitions for the Rust backend DLL.
///
/// All functions are exported from `tool_backend.dll` with C ABI.

// C string = pointer to null-terminated UTF-8
typedef CString = Pointer<Utf8>;

// Event callback: void (*)(const char* json, void* user_data)
typedef EventCallbackNative = Void Function(CString, IntPtr);
typedef EventCallbackDart = void Function(Pointer<Utf8>, int);

// wb_create: int32_t (*)(const char* app_dir)
typedef WbCreateNative = Int32 Function(CString);
typedef WbCreateDart = int Function(Pointer<Utf8>);

// wb_destroy: void (*)()
typedef WbDestroyNative = Void Function();
typedef WbDestroyDart = void Function();

// wb_set_event_callback: void (*)(EventCallback, void*)
typedef WbSetEventCallbackNative =
    Void Function(Pointer<NativeFunction<EventCallbackNative>>, IntPtr);
typedef WbSetEventCallbackDart =
    void Function(Pointer<NativeFunction<EventCallbackNative>>, int);

// wb_poll_events: void (*)()
typedef WbPollEventsNative = Void Function();
typedef WbPollEventsDart = void Function();

// wb_cmd: char* (*)(const char* cmd, const char* params_json)
typedef WbCmdNative = CString Function(CString, CString);
typedef WbCmdDart = Pointer<Utf8> Function(Pointer<Utf8>, Pointer<Utf8>);

// wb_free_string: void (*)(char* s)
typedef WbFreeStringNative = Void Function(CString);
typedef WbFreeStringDart = void Function(Pointer<Utf8>);

// wb_get_ports: char* (*)()
typedef WbGetPortsNative = CString Function();
typedef WbGetPortsDart = Pointer<Utf8> Function();

// wb_get_plugins: char* (*)()
typedef WbGetPluginsNative = CString Function();
typedef WbGetPluginsDart = Pointer<Utf8> Function();

// wb_get_status: char* (*)()
typedef WbGetStatusNative = CString Function();
typedef WbGetStatusDart = Pointer<Utf8> Function();

/// Loads the Rust backend DLL and returns all FFI function bindings.
class BackendBindings {
  late final DynamicLibrary _lib;

  late final WbCreateDart wbCreate;
  late final WbDestroyDart wbDestroy;
  late final WbSetEventCallbackDart wbSetEventCallback;
  late final WbPollEventsDart wbPollEvents;
  late final WbCmdDart wbCmd;
  late final WbFreeStringDart wbFreeString;
  late final WbGetPortsDart wbGetPorts;
  late final WbGetPluginsDart wbGetPlugins;
  late final WbGetStatusDart wbGetStatus;

  BackendBindings() {
    // Load the shared library based on platform
    if (Platform.isWindows) {
      // Try multiple locations for the DLL
      final paths = [
        'tool_backend.dll',
        'backend/tool_backend.dll',
        '../target/release/tool_backend.dll',
        '../target/debug/tool_backend.dll',
      ];

      DynamicLibrary? lib;
      for (final path in paths) {
        try {
          lib = DynamicLibrary.open(path);
          break;
        } catch (_) {
          continue;
        }
      }

      // Try loading from PATH or next to the executable.
      lib ??= DynamicLibrary.open('tool_backend.dll');
      _lib = lib;
    } else if (Platform.isMacOS) {
      _lib = DynamicLibrary.open('libtool_backend.dylib');
    } else {
      _lib = DynamicLibrary.open('libtool_backend.so');
    }

    _bindFunctions();
  }

  BackendBindings.fromLibrary(this._lib) {
    _bindFunctions();
  }

  void _bindFunctions() {
    wbCreate = _lib.lookupFunction<WbCreateNative, WbCreateDart>('wb_create');

    wbDestroy = _lib.lookupFunction<WbDestroyNative, WbDestroyDart>(
      'wb_destroy',
    );

    wbSetEventCallback = _lib
        .lookupFunction<WbSetEventCallbackNative, WbSetEventCallbackDart>(
          'wb_set_event_callback',
        );

    wbPollEvents = _lib.lookupFunction<WbPollEventsNative, WbPollEventsDart>(
      'wb_poll_events',
    );

    wbCmd = _lib.lookupFunction<WbCmdNative, WbCmdDart>('wb_cmd');

    wbFreeString = _lib.lookupFunction<WbFreeStringNative, WbFreeStringDart>(
      'wb_free_string',
    );

    wbGetPorts = _lib.lookupFunction<WbGetPortsNative, WbGetPortsDart>(
      'wb_get_ports',
    );

    wbGetPlugins = _lib.lookupFunction<WbGetPluginsNative, WbGetPluginsDart>(
      'wb_get_plugins',
    );

    wbGetStatus = _lib.lookupFunction<WbGetStatusNative, WbGetStatusDart>(
      'wb_get_status',
    );
  }
}
