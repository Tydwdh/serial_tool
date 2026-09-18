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

## WorkbenchApp 定位（已收敛）

`WorkbenchApp { workbench: Workbench, panels/*, send, notifications, ... }` — `bus/transport/recorder/plugin_manager` 均经 `workbench.*` 代理，`app` 仅剩 `eframe` 生命周期、`dock/shortcuts/dialogs/themes`。原 `Self{ bus/transport/plugin_manager/recorder }` 重复字段**已物理删除**（不再是待办）。

## 验证

CI 的 **Windows「Test & Lint」作业**（`.github/workflows/ci.yml`，toolchain 钉 `1.92.0`）实际执行：

```bash
cargo fmt --all --check                        # 工作区级
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

本机复跑（`<TOOLCHAIN>` 换成 `+1.92.0` 以对齐 CI）：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo tree -p tool-panels | grep -E '^[├└]── '   # 顶格 = depth-1 直接依赖；用 '^\s*' 会误捕 depth-2+
cargo tree -p tool-application | grep -i egui    # 0 行
```

注意事项（都是实测结论，别照抄旧写法）：

- `Cargo.toml` 有 `default-members = ["crates/app"]`，因此**裸跑** `cargo clippy --all-targets` /
  `cargo test --all-targets` 只覆盖 `crates/app` 一个包，`crates/application/tests/*.rs` 与其他 crate
  既不执行也不被 lint。必须带 `--workspace`（CI 已改为带 `--workspace`）。
- `rg "egui|eframe" crates/application` **不为 0 行**：`crates/application/src/**` 有 **4 处**在注释里合法提到 egui
  （`task.rs:136`、`workbench.rs:236`、`model/mod.rs:1`、`service/terminal.rs:3`，均为 doc/行注释）。
  判断 UI 隔离要看 `Cargo.toml` 与 `crates/application/tests/architecture.rs`，不要用这条 grep。
- MSRV 差异：本机 stable 是 `1.98.0`，CI 钉 `1.92.0`。**必须用 `+1.92.0` 复跑**——历史上两套工具链
  对 lint 的判定确实相反，并因此暴露过两个 red gate（**均已修复**）：
  * `clippy::nonminimal_bool` @ `crates/application/src/workbench.rs:1158`
    （`!network.is_some()` → `network.is_none()`，t11 等价重写，无 lint 豁免）；
  * `clippy::let_and_return` @ `crates/app/src/panel_registry.rs:73-79`（整个 wasm-only 的 `web()`；
    收敛后的返回表达式在 `:78`）：t13 去掉中间的 `let registry = …;` 直接返回，无 lint 豁免。
  **当前状态（修复后）**：`cargo +1.92.0 fmt --all --check`、
  `cargo +1.92.0 clippy --workspace --all-targets -- -D warnings`、
  `cargo +1.92.0 test --workspace --all-targets` 三条均 **exit 0**，
  test 为 **506 passed / 0 failed / 7 ignored**（21 targets）。
  注意 CI 的 `wasm` 作业 clippy（`:195`）**同样带 `-D warnings`**，且必须用
  `--target wasm32-unknown-unknown` 才会 lint 到 wasm-only 代码 —— 只跑宿主侧门抓不到上面第二个红门。

## 剩余工作（原「朝 todo.txt §69 全量达标」）

> **依据核对提示**：本条原引用 `todo.txt §39/§69` 作为验收依据，但 `todo.txt`
> **既不在仓库中、也不在 git 历史里**（`git log --all -- todo.txt` 为空），该依据无法核对。
> 下文一律改为用「可执行的命令 + 可引用的代码位置」作为依据。

### 已完成

1. **panels 不再越过 DTO 直连领域 crate** —— `crates/panels/Cargo.toml` 现在只剩
   `tool-application / tool-core / tool-databus / tool-platform`（native 段额外 `open`）；
   `tool-transport / tool-recorder / tool-extension / tool-marketplace` 均已不在依赖列表。
   `ReplayPanel/PluginsPanel` 改收 `ReplayView / PluginViewState / MarketplaceView` DTO。
   核对：
   ```bash
   cargo tree -p tool-panels | grep -E '^[├└]── '   # depth-1 直接依赖里不应出现上述 4 个 crate（顶格锚点）
   ```
2. **`WorkbenchApp` 重复服务字段已物理删除** —— `crates/app/src/workbench_app.rs` 不再持有
   `bus/transport/recorder/plugin_manager`；`bus` 只是 `WorkbenchApp::new` 的局部变量，
   服务统一经 `workbench: Workbench` 访问。核对：
   ```bash
   grep -nE 'pub\(crate\) (bus|transport|recorder|plugin_manager)\b' crates/app/src/workbench_app.rs  # 0 行
   ```
3. **Headless 契约测试已补齐** —— `crates/application/tests/headless.rs` 现覆盖
   `send routing / terminal bound / replay lifecycle / plugin enable-disable / event emission` 五类。
   核对：
   ```bash
   cargo test --workspace --all-targets
   ```
4. **CI 的 Windows 作业不再被 `default-members` 收窄** —— `.github/workflows/ci.yml` 的
   clippy 与 test 均带 `--workspace`，与 Linux 作业一致。核对：见下方「验证」段。
5. **新增 app 侧架构守卫（本轮）** —— `crates/app/tests/architecture.rs` 的 4 条断言把
   "`crates/panels` 不得直连领域 crate（transport/recorder/extension/marketplace/lua-host/updater）"
   与"app 不得重新引入已删依赖"变成**可失败的契约**（而非仅靠文档约定）。
   它由 `cargo test --all-targets` 执行（app 的 integration test target）。核对：
   ```bash
   cargo test --all-targets        # 应见 tests\architecture.rs -> 4 passed
   ```

### 仍未做（本轮明确不做，缺口如实标注）

6. **`crates/app` 仍直连 7 个内部 crate**（缺口 + 原因；`tool-application` / `tool-panels` 除外）：
   `tool-core`（调用点**多处**；计数口径如下）、
   `tool-platform`（调用点**多处**；计数口径如下）、
   `tool-databus`、`tool-transport`、`tool-marketplace`、`tool-lua-host`、`tool-updater`。
   > **计数口径（可复现；此前写的"约 79 处 / 30+ 处"口径未定义，已更正）**：在 `crates/app/src` 下
   > 按 `.rs` 文件统计 —— 限定路径形式 `tool_core::` = **30 处**、`tool_platform::` = **44 处**；
   > 若把裸名用法（`Event::`、`PortId::`、`SerialSettings` 等经 `use` 引入的名字）一并计入，
   > 则分别约 **137** / **113** 处。两者都是安全的"多处"，引用时请连口径一起写。
   > 核对命令：
   > ```bash
   > Get-ChildItem -Recurse crates\app\src -Filter *.rs | Select-String 'tool_core::'    # 30
   > Get-ChildItem -Recurse crates\app\src -Filter *.rs | Select-String 'tool_platform::' # 44
   > ```
   > 另有 **wasm 段**直连的 `tool-plugin-api` / `tool-plugin-runtime`
   > （`crates/app/Cargo.toml:43-44`，仅在 `cfg(target_arch = "wasm32")` 下启用），
   > 上面那 7 个未含它们；两段合计才是 app 的全部内部直连。
   > 核对（该 grep 命中 **11** 行 = 上面 7 个 + wasm 段 2 个 + `tool-application` / `tool-panels`）：
   > ```bash
   > grep -nE '^\s*tool-' crates/app/Cargo.toml
   > ```
   - 主要原因：`tool-core`/`tool-platform` 的用法贯穿 wasm 与 native 两套 UI 代码，
     收敛需要 `tool-application` 先提供等价 DTO 与构造入口，属独立迭代，不是"打磨"。
   - `tool-transport` 在 `crates/app` 的生产引用共 **5 处**（早期估为 2 处，已按实测更正）：
     | 位置 | 用途 | 备注 |
     |---|---|---|
     | `app/src/commands.rs:312` | `tool_transport::natural_sort_key(&port.port_name)` | 端口自然排序 |
     | `app/src/app/mod.rs:15` | `use tool_transport::RepaintWaker;`（用于 `Arc<dyn RepaintWaker>`） | 重绘唤醒器 |
     | `app/src/ui/bottom_panel.rs:617` | `tool_transport::parse_hex(input_trim)`（HEX 输入实时校验） | 生产调用 |
     | `app/src/ui/bottom_panel.rs:707` | `tool_transport::parse_hex(...)` | 生产调用 |
     | `app/src/ui/bottom_panel.rs:694` | `hex_preview(&self.send.input)`（模块级 `use` 在 `:1104`） | 生产调用 |

     **所需 application 能力（下一轮候选，本轮不做）**：re-export `RepaintWaker`、暴露 `event_bus()`
     共享总线句柄、以及端口自然排序能力（`natural_sort_key` 或等价 DTO 排序）；
     `parse_hex` / `hex_preview` 需等价的应用层入口，或保留 `tool-transport` 直连。
   - `tool-databus`：`app/mod.rs` 的 `DataBus::new()`、`bus.publish(..)` 与四个面板的 `::new(&bus)`；
     `web.rs`、`settings_panel.rs`、`perf.rs`、`web_perf.rs` 另有引用。
     所需 application 能力：一个返回共享总线句柄的 accessor（供 presentation 订阅），
     否则 app 无法构造面板订阅。
7. **`crates/panels` 内部仍自持 `DataBus` 订阅**（`Terminal/Log/Chart/Attitude/Gauge/Dynamic/data_table`
   各自 `subscribe_*`）。这属更大的 Presentation 数据流重构，本轮不动。
8. **`crates/app` 存在死代码子树（实测，未删）**：
   `crates/app/src/ui/bottom_panel.rs:249-310` 的 `legacy_send_panel_body` 带 `#[allow(dead_code)]`，
   全工作区 **0 调用**，块长 **62 行**（含 `#[allow]` 行，249..310；早期估为"约 350 行"，已按实测更正）。
   核对：
   ```bash
   grep -rn 'legacy_send_panel_body' crates/   # 只应命中定义处 1 行
   ```
9. **`crates/panels` 有一组已被 application DTO 取代、但仍导出的死符号（实测，未删）**：
   `replay_view.rs` 的 `ReplayView`（`:7`，含 `:55 impl From<&ReplayStatusView> for ReplayView`）
   与 `plugin_view.rs` 的 `InstalledPluginRow`（`:6`）/ `PluginViewState`（`:14`）/
   `PluginUiCommand`（`:21`）—— 工作区内**除自身定义与 `lib.rs` 的 `pub use` 外 0 引用**。
   注意 `PluginUiCommand` 存在**同名不同类型**：`tool_panels::plugin_view::PluginUiCommand`（死）vs
   `plugin_api::host::PluginUiCommand`（在 `web_plugin_host.rs`、`mlua_engine.rs`、`web_lua.rs` 中大量使用）。
   清理时不要误删后者活的那组。核对：
   ```bash
   grep -rn 'tool_panels::PluginUiCommand' crates/   # 0 行（死的那个没有消费者）
   ```
   > 早期文档曾称这些文件为"已占位"（即待用），实测是"已被取代、可直接删除"，属相反结论。
10. **`egui_extras` 是可证明的死依赖（实测，本轮裁决不做）**：
   只在 `Cargo.toml:35`（workspace 声明）与 `crates/app/Cargo.toml:16` 出现；
   **`crates/app` 内 0 引用**，全 `crates/` 的 `.rs` 中 0 处代码引用
   （唯一命中是 `crates/application/tests/architecture.rs:13` 的禁用语清单字符串字面量）。
   **影响面（实测）**：移除后 `Cargo.lock` 减少 **35 行**（**0 增 35 删**，6374 → 6339 行），
   消失的包恰为 **3 个**：`egui_extras` / `enum-map` / `enum-map-derive`，均无其它引用者。
   > **数字出处与测量差异（重要）**：
   > * **35 行**来自 t10 reviewer 在**仓库外完整副本**（`%TEMP%\t10-lockfull`）中的独立测量，
   >   并由 t16 在另一个仓库外副本中**双向复现通过**（删除 → 6339；写回 → 6374）——数字可靠。
   > * analyst 早前在**共享工作区**用临时探针曾报 **37 行**（且给了逐包 14/11/13，三数之和 38 与 37 亦不自洽）。
   >   该数字**不可信、已作废**：测量方式不当（在共享工作区临时改 tracked 文件），且事后无法复现。
   > * **分析人员此前"`cargo metadata` 不会剪除锁条目"的结论是错的，已撤回**。真因是那次用了
   >   `--no-deps`：**带 `--no-deps` 只做工作区成员解析，不会重写 `Cargo.lock`；去掉它才会正常重解析并剪除**。
   >   实测（同一副本、同一命令、只差该 flag）：移除声明后 `--no-deps` → 6374 行不变；
   >   不带 `--no-deps` → **6339 行**。故 35 行**无需联网、无需 `generate-lockfile`** 即可复现。
   > * **标准复跑配方（全程离线、仓库零改动）**：在仓库外**完整副本**中
   >   ① `cargo metadata --offline --format-version 1`（幂等，应仍 6374）；
   >   ② 删除 `crates/app/Cargo.toml` 的 `egui_extras.workspace = true` 后再跑同一条命令 → **6339 行**、
   >      `egui_extras`/`enum-map`/`enum-map-derive` 三包消失；
   >   ③ 写回该行后再跑 → **回到 6374**、三包恢复（双向对照可证"不是 harness 使然"）。
   >   副本须含全部 `Cargo.toml` 与 `src/`（**manifest-only 副本会报 `no targets specified in the manifest`**）；
   >   `--offline` 依赖本机 cargo registry 缓存，全程无网络。
   > * **测量纪律**：凡需改动工作区的测量一律在**仓库外临时副本**上做；只读取证用 `--locked` ——
   >   `cargo tree` 与不带 `--locked` 的 `cargo metadata` **会重写 `Cargo.lock`**，
   >   在验证/评审窗口期间会让别人的冻结取证失效。
   核对：
   ```bash
   Get-ChildItem -Recurse crates -Include *.rs -File | Select-String 'egui_extras'
   ```
   > **本轮不做的理由**：captain 裁定本轮延后（避免让验证/评审对象漂移），且 `Cargo.lock`
   > 不在任何任务的 inScope。
   > **下一轮立项必须把 `Cargo.lock` 纳入 inScope**，否则任何"删死依赖"都无法落地。

### 本轮裁掉项

11. `crates/app/Cargo.toml` 删除 `crossbeam-channel`（本 crate 0 引用，工作区其它 crate 仍在用）；
    `crates/panels/Cargo.toml` 删除 `tool-marketplace`。两者都会改写 `Cargo.lock` 的依赖边
    （无版本变更、无新增行）。

