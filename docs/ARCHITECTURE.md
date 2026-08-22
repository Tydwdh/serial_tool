# Architecture

> `cargo check --workspace && cargo test --workspace` 为契约。

## Crate 依赖图

```text
tool-core (Event/Payload/Config)
  ▲
  ├── tool-databus (TopicFilter/DataBus)
  ├── tool-transport (TransportManager, SerialConfig)
  ├── tool-recorder (JsonlRecorder/ReplayManager)
  └── tool-extension → tool-lua-host
          ▼
      DataBus ──► tool-application (Workbench/AppCommand/Query/Event)
                    ▲  不依赖 egui/eframe/rfd
        ┌───────────┴───────────┐
     tool-panels             hardware-workbench-app (eframe shell)
     (egui)                       dock/shortcuts/dialogs/themes
```

`tool-application` 允许依赖 `core/databus/transport/recorder/extension/lua_host/marketplace/updater`，**禁止** `egui/eframe/egui_tiles/egui_extras/egui_material_icons/rfd/tool-panels/hardware-workbench-app`。

## Application / Presentation 边界

| 层 | 职责 | 例子 |
|---|---|---|
| Application (`tool-application`) | 行为与状态 | `Workbench::dispatch(AppCommand)`, `TerminalService`, `ApplicationConfig` |
| Presentation (`tool-panels` + `app`) | 呈现与交互 | `TerminalPanel/LogPanel`, `PanelRegistry`, `CommandPalette`, `rfd` 文件选择 |

## Command / Query / Event

- **Command** (`AppCommand`)意图：`RefreshPorts/Connect/SendText/StartRecording/LoadReplay/EnablePlugin/ClearTerminal...`，由 `Workbench::dispatch` 执行，返回 `CommandOutcome::Done|Pending`，错误为 `AppError`。
- **Query** 只读 DTO：`query_transport/query_recording/query_replay/query_plugins/query_terminal_since(seq,limit)`，不暴露 `&mut TransportManager`。
- **Event** 事实：复用 `tool-databus/DataBus`，`transport.serial.* / log.system / ui.* / plugin.command.*`，高频终端用 `TerminalService::entries_since(seq,limit)->TerminalDelta{entries,next_seq,truncated,dropped}` 增量消费，不每帧 clone 全量。

## State 归属

- **Application**：连接状态、自动重连、录制/回放状态、插件启停、终端 entry 存储/merge/上限。
- **Presentation**：`PanelManager` 布局、`monospace_font_size/ui_theme/theme_path`、`UiState::Send/CommandPalette/ShortcutRegistry/ToastOverlay`。

`PersistedConfig` (`app/config.rs`) 在序列化上仍单文件 `workspace.json`，Rust 类型上 `ApplicationConfig` 与 egui 配置分离。

## Plugin 边界

- **Runtime** (`tool-extension` + `tool-lua-host`) 在 `Workbench`：`discover_roots/enable/disable/refresh`，`ExecutePluginCommand` 发布 `plugin.command.execute`。
- **Presentation** (`tool-panels/dynamic`) 仍在 `tool-panels`：`DynamicPanels::ingest` 消费 `ui.panel.create` 等 topic，`is_allowed` 鉴权。

## 如何新增能力

1. 在 `tool-application::command::AppCommand` 加变体
2. 在 `workbench.rs` 实现 `dispatch` 分支 + `query`/`TerminalService` 扩展
3. 写 `crates/application/tests/headless.rs` 用例（不启动 egui）
4. 最后在 `crates/panels` 加渲染、`crates/app` 加 `UiCommand`/快捷键

## 如何新增 Panel

- 纯展示 Panel：直接在 `tool-panels` 新增 `struct FooPanel`，在 `PanelRegistry::builtin` 注册 `fn(&mut WorkbenchApp,&mut Ui)`（仅 UI 行为）。
- 需业务 Panel：先在 `tool-application` 暴露 `AppCommand/Query`，Panel 通过 `app.workbench.query_*()` 读取、`dispatch()` 写入。

## 约束

- `Workbench` 非 `Arc<Mutex>`，`&mut self`  ownership 由 `WorkbenchApp { workbench }` 持有；未来桥接再包 `Arc<Mutex>`。
- 同步 `dispatch` 为主，worker 仍经 `thread + channel + DataBus` 异步。
- API 暴露 `String/Vec/enum/PathBuf` 现实 DTO，不透传 `&InternalManager`。
