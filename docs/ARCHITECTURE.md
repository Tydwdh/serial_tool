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

## Application / Presentation 边界

| 层 | 职责 | 代表 |
|---|---|---|
| Application (`tool-application`) | 行为与状态 | `Workbench::dispatch(AppCommand)->Result<CommandOutcome,AppError>` / `query_*` / `tick(now)` / `TerminalService(seq)` / `ApplicationConfig` |
| Presentation (`tool-panels` + `app` UI) | 呈现与交互 | `TerminalPanel/LogPanel/ChartPanel + DynamicPanels` / `PanelRegistry::Builtin(fn(&mut WorkbenchApp,&mut Ui))` / `CommandPalette` / `rfd` |

判定规则（todo.txt §7）：换个 UI 仍需知道即 `Application`，仅用于当前 `egui` 的 `scroll/selection/hover/dock` 即 `Presentation`.

## Terminal 高频特殊处理

`TerminalService` 持有 `RingSubscription(prefix("transport.serial."),65_536)`，`push_event` 做 `merge(同port同方向≤merge_window_ms且不以\n结尾则拼接)`，`enforce_limit(50_000>` 按 `seq` 最旧淘汰)，`query_terminal_since` 供 headless/Flutter 增量拉取。

## WorkbenchApp 定位（收敛中）

`WorkbenchApp { workbench: Workbench, panels/*, send, notifications, ... }` — `bus/transport/recorder/plugin_manager` 均经 `workbench.*` 代理，`app` 仅剩 `eframe` 生命周期、`dock/shortcuts/dialogs/themes`。剩余 `Self{ bus/transport/plugin_manager/recorder }` 字段已 `#[allow(dead_code)]` 待物理删除（下一小步）。

## 验证

```bash
cargo check --workspace
cargo test --workspace
cargo test -p tool-application  # architecture/headless 契约
cargo tree -p tool-application | grep -i egui  # 0 行
rg "egui|eframe" crates/application             # 仅注释/测试
```

## 剩余工作（朝 todo.txt §69 全量达标）

1. `tool-panels/Cargo.toml` 去 `tool-transport/recorder/extension/marketplace`，`ReplayPanel/PluginsPanel` 改收 `ReplayView/PluginViewState` DTO（`replay_view.rs/plugin_view.rs` 已占位）。
2. `crates/app/Cargo.toml` 去 `tool-transport/recorder/extension/marketplace/lua_host/updater/databus` 直连，仅留 `tool-application + tool-panels + egui/eframe + open/serde`，删除 `WorkbenchApp` 重复字段。
3. 补 `todo.txt §39` Headless 用例：`send routing/terminal bound/replay lifecycle/plugin enable-disable/event emission`。
