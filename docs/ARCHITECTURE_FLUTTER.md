# 硬件调试工作台 — Flutter 前端架构

## 概述

将原有 egui 前端替换为 Flutter，保留全部 Rust 后端逻辑不变。
通过 C FFI 实现 Dart ↔ Rust 双向通信。

## 架构分层

```
┌──────────────────────────────────────────────────┐
│  Flutter (Dart)                                   │
│  ┌──────────────────────────────────────────────┐ │
│  │  UI Widgets (Material 3)                     │ │
│  │  ├─ Shell (DockLayout)                       │ │
│  │  ├─ TerminalPanel / LogPanel / ChartPanel    │ │
│  │  ├─ SenderPanel / ReplayPanel / PluginPanel  │ │
│  │  └─ SettingsPanel / CommandPalette / Popups  │ │
│  ├──────────────────────────────────────────────┤ │
│  │  State (Riverpod)                            │ │
│  │  ├─ BackendService (FFI 封装)                │ │
│  │  ├─ StreamProvider (事件流)                   │ │
│  │  └─ StateNotifier (面板状态)                  │ │
│  ├──────────────────────────────────────────────┤ │
│  │  FFI Bridge (dart:ffi)                       │ │
│  │  ├─ wb_create / wb_destroy                   │ │
│  │  ├─ wb_cmd / wb_query                        │ │
│  │  └─ EventCallback → StreamController         │ │
│  └──────────────────────────────────────────────┘ │
├──────────────────────────────────────────────────┤
│  C FFI Boundary (JSON strings + callbacks)        │
├──────────────────────────────────────────────────┤
│  Rust (cdylib)                                    │
│  ┌──────────────────────────────────────────────┐ │
│  │  WorkbenchBackend                            │ │
│  │  ├─ TransportManager (串口)                  │ │
│  │  ├─ PluginManager (Lua 插件)                 │ │
│  │  ├─ JsonlRecorder (录制)                     │ │
│  │  ├─ ReplayManager (回放)                     │ │
│  │  ├─ ConfigStore (配置)                       │ │
│  │  └─ EventBridge (DataBus → Callback)        │ │
│  └──────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────┘
```

## 通信协议

### Rust → Dart (事件推送)

Rust 通过函数指针回调向 Dart 推送 JSON 事件：
```json
{"kind": "serial_data", "port": "COM3", "direction": "rx", "data": "SGVsbG8=", "ts": 1234567890}
{"kind": "port_list", "ports": [{"name": "COM3", "type": "usb_serial", "vid": "1A86", "pid": "7523"}]}
{"kind": "log", "level": "info", "source": "app", "message": "串口已打开"}
{"kind": "notification", "level": "info", "message": "端口已连接"}
{"kind": "recorder_status", "recording": true, "stats": {"events": 42, "bytes": 1024}}
{"kind": "replay_status", "state": "playing", "position": 0.5}
{"kind": "plugin_diagnostics", "diagnostics": [...]}
{"kind": "plugin_list", "plugins": [...]}
```

### Dart → Rust (命令调用)

Dart 通过 FFI 同步/异步调用 Rust：
```c
// 查询（同步返回 JSON）
char* wb_query(void* backend, const char* cmd, const char* params_json);

// 命令（异步，通过事件回调返回结果）
void wb_cmd(void* backend, const char* cmd, const char* params_json);
```

## 数据流

```
串口硬件 → TransportManager → DataBus → EventBridge → FFI Callback
                                              ↓
                                        Dart StreamController
                                              ↓
                                        Riverpod StateProvider
                                              ↓
                                        Flutter Widget (rebuild)
```

## 状态管理 (Riverpod)

```dart
// 后端单例
final backendProvider = Provider<BackendService>((ref) => BackendService());

// 事件流
final eventStreamProvider = StreamProvider.autoDispose<BackendEvent>((ref) {
  return ref.watch(backendProvider).eventStream;
});

// 串口列表
final portListProvider = StateNotifierProvider<PortListNotifier, List<PortDescriptor>>((ref) {
  return PortListNotifier(ref.watch(backendProvider));
});

// 终端条目
final terminalEntriesProvider = StateNotifierProvider.autoDispose.family<TerminalNotifier, List<TerminalEntry>, String>((ref, port) {
  return TerminalNotifier(ref.watch(backendProvider), port);
});
```

## 面板迁移顺序

| 优先级 | 面板 | 复杂度 | 说明 |
|--------|------|--------|------|
| P0 | Shell (DockLayout) | 高 | 整体布局框架，必须先完成 |
| P0 | 串口终端 | 高 | 最核心功能，数据量大 |
| P0 | 发送器 | 中 | 与终端配对 |
| P1 | 日志面板 | 低 | 数据结构简单 |
| P1 | 录制/回放 | 中 | 控制逻辑复杂 |
| P1 | 插件面板 | 中 | 生命周期管理 |
| P2 | 图表/姿态/仪表 | 中 | 数据可视化 |
| P2 | 设置面板 | 中 | 配置表单 |
| P2 | 命令面板 | 低 | 搜索+执行 |
| P2 | 插件市场 | 低 | 网络请求+列表 |

## 目录结构

```
serial_tool/
├── crates/
│   ├── backend/           # NEW: FFI 后端库
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs     # 入口 + C FFI 导出
│   │       ├── backend.rs # WorkbenchBackend
│   │       ├── event.rs   # BackendEvent 定义
│   │       └── bridge.rs  # DataBus → FFI 桥接
│   ├── core/              # 不变
│   ├── databus/           # 不变
│   ├── transport/         # 不变
│   ├── lua_host/          # 不变
│   ├── extension/         # 不变
│   ├── recorder/          # 不变
│   ├── panels/            # 剥离 egui，保留数据模型
│   ├── updater/           # 不变
│   └── marketplace/       # 不变
├── flutter/               # NEW: Flutter 项目
│   ├── lib/
│   │   ├── main.dart
│   │   ├── src/
│   │   │   ├── backend/   # FFI 绑定 + BackendService
│   │   │   ├── providers/ # Riverpod providers
│   │   │   ├── models/    # Dart 数据模型
│   │   │   ├── panels/    # 各面板 Widget
│   │   │   ├── widgets/   # 通用组件
│   │   │   └── theme/     # 主题
│   │   └── ...
│   ├── pubspec.yaml
│   └── ...
├── Cargo.toml
└── ...
```

## Rust FFI API

```c
// ─── 生命周期 ───
void* wb_create(const char* app_dir);
void  wb_destroy(void* backend);
void  wb_poll_events(void* backend);  // 在主线程定期调用，处理事件队列

// ─── 事件回调 ───
typedef void (*EventCallback)(const char* json_event, void* user_data);
void wb_set_event_callback(void* backend, EventCallback cb, void* user_data);

// ─── 查询（同步，返回 JSON 字符串，需 wb_free_string）───
char* wb_query(void* backend, const char* cmd, const char* params_json);

// ─── 命令（异步，结果通过事件回调返回）───
void wb_cmd(void* backend, const char* cmd, const char* params_json);

// ─── 内存管理 ───
void wb_free_string(char* s);
```