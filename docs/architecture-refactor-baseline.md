# Architecture Refactor Baseline — Phase 0 Audit (事实记录)

> 生成时间：2026-08-23 / 分支：`main` @ `0fc66b3` / 审计方式：`rg + Cargo.toml + 源码只读`
> 本文件仅记录事实，不含设计主张。对应 `todo.txt` §27。

## 1. Workspace 成员与依赖图

`Cargo.toml` workspace members (11 crates + vendor):

```
crates/app  crates/core  crates/databus  crates/extension  crates/lua_host
crates/marketplace  crates/panels  crates/recorder  crates/testing
crates/transport  crates/updater  vendor/egui_tiles
```

`cargo tree` 隐式依赖（由 `Cargo.toml` 推导）：

- `tool-core` : `serde/serde_json/log` — 无 UI
- `tool-databus` : `crossbeam-channel/parking_lot + tool-core` — 无 UI
- `tool-transport` : `serialport/tungstenite + tool-core/databus` (+ windows-sys) — 无 UI
- `tool-recorder` : `tool-core/databus` — 无 UI
- `tool-extension` : `tool-core/databus/lua_host/transport/testing` — 无 UI（经 lua_host 间接 mlua）
- `tool-lua-host` : `mlua(lua54 vendored)/regex + tool-core/databus/transport/testing` — 无 UI
- `tool-marketplace` : `tool-updater` — 无 UI
- `tool-updater` : `reqwest/zip/sha2/url` — 无 UI
- `tool-panels` : `egui/egui_tiles + tool-core/databus/extension/marketplace/recorder/transport` — **唯一含 egui 的库 crate**
- `hardware-workbench-app` : `eframe/egui/egui_tiles/rfd/tokio + 9 内部 crates (core/databus/extension/lua_host/marketplace/panels/recorder/transport/updater)` — God Object 宿主

**结论：`tool-core/databus/transport/recorder/extension/lua_host/marketplace/updater/testing` 均无 UI 依赖；UI 污染仅在 `tool-panels` 与 `app`。**

## 2. WorkbenchApp 职责审计 (`crates/app/src/app/mod.rs:23-83`)

40 字段，按 `todo.txt §7` 三分类：

### A. Application State（应入 `tool-application` 或底层 crate）

| 字段 | 类型 | 说明 |
|------|------|------|
| `bus` | `DataBus` | 统一事件总线，`with_history_limit(20_000)` |
| `transport` | `TransportManager` | `new(bus)+set_repaint_waker(ctx)`，worker 线程经 DataBus 发 `transport.serial.*` |
| `plugin_manager` | `PluginManager` | `new(bus,transport.clone())` + `set_host_services(dialog_sender,file_broker)` + `discover_roots([exe/plugins])` |
| `recorder` | `JsonlRecorder` | `new(bus)` 订阅 DataBus 写 JSONL |
| `serial` | `SerialUiState(state.rs:213)` | 13 子字段：`selected_port/baud_rate/data_bits/stop_bits/parity/port_aliases/port_groups/port_profiles/network_ports/auto_reconnect/pending_reconnect/...` |
| `recorder_path` | `String` | 默认 `logs/session-{ts}.jsonl` |
| `file_broker` | `Arc<FileAccessBroker>` | 插件 FS 权限网关 |
| `dialog_receiver/file_browse_subscription/contribution_set_value_subscription/ui_set_status_subscription` | `Receiver/Subscriptions` | LuaHost 宿主服务通道 |
| `replay_analyzer` | `ReplayAnalyzerState` | `generation + JoinHandle<ReplayAnalyzerResult> + AtomicBool cancel` |
| `periodic_send` | `PeriodicSendState` | `thread::spawn + AtomicBool + Mutex<Option<(StatusLevel,String)>>` |
| `marketplace` | `MarketplaceState(runtime/marketplace.rs:10)` | `refresh_job/install_job: JoinHandle + AtomicU64 progress` |
| `network_proxy_url` | `String` | 代理配置 |

### B. Presentation State（必须留在 `app/panels`）

`panels: PanelManager`, `terminal_panel: TerminalPanel`, `dynamic_panels: DynamicPanels`, `plugins_panel: PluginsPanel`, `replay_panel: ReplayPanel`, `bottom_log_panel: LogPanel`, `notifications: NotificationQueue`, `toast_overlay: ToastOverlay`, `send: SendUiState(state.rs:156, 12字段)`, `layout_dirty`, `last_auto_save_time`, `pending_command`, `key_recording`, `command_palette: CommandPaletteState`, `contribution_states: HashMap`, `plugin_summaries_cache: OnceCell<Vec<PluginSummary>>`, `monospace_font_size`, `ui_theme: AppTheme`, `theme_path/theme_dir`, `recent_workspaces`

### C. Ambiguous（跨层）

`keymap: Keymap`, `commands: CommandRegistry`, `panel_registry: PanelRegistry`, `update_state: UpdateState(state.rs:271, JoinHandle+AtomicU64)`, `serial.network_host/port` vs `network_ports`, `terminal_panel.merge_window_ms/max_entries/log_max_entries` — 既持久化到 `workspace.json` 又每帧驱动渲染；`config.rs:345 build_config_snapshot` 将 Panel 私有字段提升为持久配置。

**判定：`WorkbenchApp` 同时承担程序入口/生命周期/UI/配置/快捷键/命令/业务状态/transport/recorder/plugin/replay/workspace 持久化 12 职责，符合 God Object 定义。**

## 3. 耦合证据（≥10 处业务逻辑在 `WorkbenchApp`/`re-render`）

| # | 位置 | 方法 | 耦合业务 |
|---|------|------|----------|
| 1 | `app/mod.rs:438` | `eframe::App::update` | `tick_pre_ui → draw_shell → tick_post_ui → export → toast → request_repaint_after 80/250ms` 帧编排即业务调度 |
| 2 | `commands.rs:207` | `refresh_ports_impl` (160行) | `BTreeSet diff + pending_reconnect 指数退避 1<<a*100 min30000` 10次上限 + 网络端口合并排序 |
| 3 | `commands.rs:383/430/471` | `toggle/open/reconnect_port` | 参数校验 `parse_data_bits/parity` + `open_network_serial` + `close_port_blocking 3s` + 读写 `serial.port_profiles` |
| 4 | `runtime/periodic_send.rs:27` | `tick_periodic_send` | 校验后克隆5字段进 `thread::spawn`，`AtomicBool+Mutex` 回传 |
| 5 | `runtime/replay.rs:10` | `tick_replay/rebuild_replay` | 清 `terminal/log/charts` + `publish ui.replay.reset` + `seek` 闭包 + `ingest_all_pending` |
| 6 | `runtime/plugin.rs:96` | `tick_plugin_lifecycle` | `process_pending → rebuild_plugin_commands → sync_dynamic_panels → take_cleanup_requests` |
| 7 | `replay_task.rs:26` | `launch_replay_analyzer_background` | `read_to_string → run_replay_analyzer_with_cancel → generation 过期丢弃` |
| 8 | `runtime/marketplace.rs:58` | `tick_marketplace` | 每帧 `join` 2个 `JoinHandle` 轮询，回填 `plugins_panel` |
| 9 | `runtime/keys.rs:8/38` | `handle_keys/flush_pending_command` | `ctx.input → pending_command → execute_command`，录制时 `save_config()` 直接落盘 |
| 10 | `runtime/update.rs:7` | `tick_update` | `want_restart → save_config → write_update_manifest → launch_helper → exit(0)` |
| 11 | `runtime/autosave.rs:7` | `tick_auto_save/tick_recorder_status` | 60s `save_config()` + `reap_stopping/reap_error` 在帧回调内 IO |
| 12 | `config.rs:304/355` | `build_config_snapshot/save_config` | `panels.clone().discard_dynamic_tabs` + 原子写盘 |
| 13 | `commands.rs:12` | `export_terminal_data` | `rfd::FileDialog` 阻塞 UI + `write_utf8_csv` |
| 14 | `panel_registry.rs:149` | `render_devices/*` | `fn(&mut WorkbenchApp,&mut Ui)` 注册表在 `tiles.rs:43` 分派 |
| 15 | `ui/device_panel.rs:14` | `device_panel` (708行) | 直写 `serial.*` + `recorder.start/stop/pause/resume/set_mode` |

`runtime/*` 与 `ui/*` 均以 `impl WorkbenchApp` 横向扩展；`CommandRegistry`/`PanelRegistry` 固化 `fn(&mut WorkbenchApp)`。

## 4. Command / Panel 注册表

- **`command_registry.rs:60`** `enum CommandHandler { Builtin(fn(&mut WorkbenchApp)), Plugin{plugin_id,command_id} }`，`handler.run(&mut app)`。`builtin()` 注册 11 条：`$RefreshPorts/$OpenPort/$ReconnectPort/$Send/$ClearTerminal/$StartRecording/$AddBookmark/$ToggleBottomPanel/$ToggleRightDock/$CommandPalette`。
- **`panel_registry.rs:33`** `enum PanelRender { Builtin(fn(&mut WorkbenchApp,&mut Ui)), Dynamic{suffix} }`，`tiles.rs:17 WorkbenchTiles{app:&'a mut WorkbenchApp}` 持 `&mut` 分派。

**禁止的 `Fn(&mut WorkbenchApp)` 作为业务 handler 已确认存在。**

## 5. Panel 与业务耦合 (`crates/panels`)

`panels/Cargo.toml:14-29` 直接依赖 `tool-core/databus/extension/marketplace/recorder/transport` — 4 业务 crate。

| Panel | 持有业务状态 | 直连证据 |
|-------|--------------|----------|
| `Terminal(terminal.rs:84)` | `RingSubscription + BTreeMap<String,PortData: VecDeque<TerminalEntry>> + merge_window_ms/max_entries(50k)` | `subscribe_ring_bounded(prefix("transport.serial."),65536)` + `SERIAL_RX/TX` 过滤；`push_entry()` 含 merge/history/clear/export 闭环 |
| `Log(log.rs:35)` | `RingSubscription + VecDeque<LogEntry> max 50k` | `subscribe_ring_bounded(prefix("log."),65536)` |
| `Replay(replay.rs:12)` | **`manager: ReplayManager` 直接持有** | `manager.load/play/pause/tick/status/policy/seek` 全直调 |
| `Plugins(plugins.rs:58)` | `MarketplaceState{registry,installing}` | `ui(&mut self, ui, manager:&mut PluginManager)` 形参即 `tool-extension`; `manager.enable/disable/summaries` |
| `Dynamic(dynamic/mod.rs:14)` | `bus + 9×Subscription + ports: Vec<SerialPortDescriptor>` | 9 个 `subscribe_lossy_bounded(UI_PANEL_CREATE/REMOVE/...)` + `Serial` 下拉直传 ` &[SerialPortDescriptor]` |
| `Chart/Attitude/Gauge` | `Subscription + series/samples` | `prefix("protocol.")` / `PROTOCOL_IMU_ATTITUDE` — 经 DataBus 弱耦合 |
| `Manager(manager.rs:17)` | 纯布局 | 零业务依赖 |

Terminal 的 entry 存储/merge/history/clear/export 全部在面板内，未下沉到 `recorder/app`；`replay` 与 `plugins` 为强耦合。

## 6. DataBus / Event 统一性

- **`tool-core lib.rs:171`** `Payload{Empty/Bytes/Text/Json}`，`Event{id,timestamp_ms,topic,source,direction,payload,metadata:Value}`，`topics` 17 常量 + `topic_matches(*通配)` + `mark_derived_event` + `config::atomic_write_*` + `Clock(SystemClock/FrozenClock)`.
- **`tool-databus lib.rs:15`** `TopicFilter{All/Exact/Prefix/And/MetadataEq}`，`DataBus{Inner{subscribers:Mutex<Vec<Subscriber>>, history:VecDeque<Arc<Event>>(20k), next_id}}`，`SubscriberSink{Channel(Sender), Ring{Weak<Mutex<VecDeque>>, capacity}}`，`publish(mut event)` 分配 id 入 history 遍历 `matches_event → try_send`，`Full→dropped++`，`RingSubscription{queue,dropped}` 有界丢最旧。
- **结论：单一 DataBus，无第二套 bus。** 需扩展 `AppEvent` 还是用现有 `Event` 由 Phase 2 决定；`Application` 不应新建 `AppEventBus`。

## 7. Transport

`transport/lib.rs:1` `TransportManager{bus, inner:Mutex<HashMap>, repaint_waker:Arc<dyn RepaintWaker>}`，`network.rs` WebSocket(JSON-RPC gcode, 7125) 与 `windows_native.rs` 串口线程各 `thread + bounded channel + DataBus publish(serial_rx_event/serial_tx_event)`。`serial_topics::{SERIAL_RX,SERIAL_TX}` 在 `tool_core::topics` 兼容 re-export。重连指数退避在 `app/commands.rs` 而非 `transport` 内（待下沉）。

## 8. Recorder / Replay

`recorder/lib.rs:1` 三模块 `format/recorder/replay`，零耦合可独立演进。

- `recorder.rs` `JsonlRecorder::new(bus)` 订阅 DataBus 异步写 JSONL，`stats()/current_path()/start(path,mode)/stop()/add_bookmark()`。
- `replay.rs` `ReplayManager::new(bus)` / `load(path)/play/pause/tick/status:ReplayStatus/policy/seek_*`，`ReplayAnalyzerState` 在 `app/replay_task.rs` 另线程 `run_replay_analyzer_with_cancel(budget 30k instr)`。
- `format.rs` `RecordMode` + 过滤策略纯函数。

**Panel 禁止直调 `recorder/replay`，当前 `ReplayPanel` 违规持有 `ReplayManager`。**

## 9. Extension / LuaHost / Marketplace

- `extension/lib.rs:11` `PluginManager::new(bus,transport)` + `set_host_services(dialog_sender,file_broker)` + `discover_roots([PathBuf])` + `enable/disable/summaries/diagnostics/process_pending/take_cleanup_requests`；`permission/manifest/spec/host_services` 子模块。
- `lua_host/lib.rs:186` `LuaHost{bus,transport,worker:Option<LuaWorker{JoinHandle}}}` / `LuaPluginRuntime{event_sender:Sender<Event>(4096), stop/alive:AtomicBool, outcome}`，`run_plugin(LuaRunConfig{script_name,timeout_ms,source,context,permissions})` 起线程 `mlua` + `instruction hook` + `process_tasks(YIELD_READ_LINE/WRITE_LINE_AND_EXPECT/EXPECT/SLEEP/WAIT_PAUSED)` 消费 `LineBufferMap`。
- `marketplace` `Registry/RegistryPlugin` + `retire_old_plugin_dirs`，`app/runtime/marketplace.rs` 每帧轮询 `JoinHandle`。

## 10. Workspace Persistence

`config.rs:23 PersistedConfig{schema_version, panels:PanelManager, selected_port, baud_rate, data_bits, stop_bits, parity, recorder_path, enabled_plugins, port_aliases, port_groups, send_history(200), line_ending, port_profiles, recent_workspaces, auto_reconnect, keymap, monospace_font_size(13 clamp10-24), ui_theme(skip), theme_path(alias custom_theme_path 相对 theme_dir), terminal_merge_window_ms(5), terminal_max_entries(50k), log_max_entries(50k), command_usage_order, network_proxy_url, network_ports:Vec<NetworkSerialConfig>}`。

`tool_core::config::atomic_write_json(先.tmp再.backup再rename)`，`parse_persisted_config` 处理 `v0→CURRENT_SCHEMA_VERSION=1` 迁移，`FutureVersion` 写保护。`build_config_snapshot` 在 `WorkbenchApp` 内 `panels.clone().discard_dynamic_tabs()`。

分类：`serial.* / network_ports / auto_reconnect / recorder_path / terminal_* / enabled_plugins` 属 Application；`panels/dock/theme/recent_workspaces/keymap` 属 Presentation；但 Rust 类型未拆分（待 Phase 1 拆为 `ApplicationConfig/EguiConfig`）。

## 11. 已知风险

- Terminal 50k `String*4` 常驻 panel 堆，merge 含 `rfind('\n')` 拆分，多次 `format_hex`，与业务解析无隔离。
- `tool-panels` 直连 4 业务 crate，`app` 依赖 9 内部 crate，`WorkbenchApp` 是唯一粘合点。
- `periodic_send / replay / marketplace / update` 各持 `JoinHandle + AtomicBool` 在 `tick_*` 每帧轮询，非 channel 统一调度。
- 高频终端当前无 `seq + delta` 增量接口，每帧全量 `collect_visible_rows()` 生成导出字符串。

---
*本文件为 Phase 0 基线，后续 Phase 的迁移以此为准，不复述设计哲学。*
