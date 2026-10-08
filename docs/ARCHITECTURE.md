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
| Application (`tool-application`) | 行为与状态 | `Workbench::dispatch(AppCommand)`, `send_plan::plan_send`（send 命令 → (任务种类, 字节) 的唯一决策点，无 `cfg` 门控、wasm 侧同调）, `ApplicationConfig` |
| Presentation (`tool-panels` + `app`) | 呈现与交互 | `TerminalPanel/LogPanel`, `PanelRegistry`, `CommandPalette`, `rfd` 文件选择 |

## Command / Query / Event

- **Command** (`AppCommand`)意图：`RefreshPorts/Connect/SendText/StartRecording/LoadReplay/EnablePlugin/ClearTerminal...`，由 `Workbench::dispatch` 执行，返回 `CommandOutcome::Done|Pending`，错误为 `AppError`。
- **Query** 只读 DTO：`query_transport/query_recording/query_replay/query_plugins`，不暴露 `&mut TransportManager`。（第 46 轮删掉 `query_terminal_since(seq,limit)`：它读的是 Application 内那份零生产使用者的重复终端 store，见「Terminal 高频数据路径」。）
- **Event** 事实：复用 `tool-databus/DataBus`，`transport.serial.* / log.system / ui.* / plugin.command.*`，高频终端的增量消费只有一处：呈现层 `TerminalPanel` 自订阅 `transport.serial.*` 后每帧 `ingest_pending()`，不每帧 clone 全量（见「Terminal 高频数据路径」）。

## State 归属

- **Application**：连接状态、自动重连、录制/回放状态、插件启停、终端 entry 存储/merge/上限。
- **Presentation**：`PanelManager` 布局、`monospace_font_size/ui_theme/theme_path`、`UiState::Send/CommandPalette/ShortcutRegistry/ToastOverlay`。

`PersistedConfig` (`app/config.rs`) 在序列化上仍单文件 `workspace.json`，Rust 类型上 `ApplicationConfig` 与 egui 配置分离。

## Plugin 边界

- **Runtime** (`tool-extension` + `tool-lua-host`) 在 `Workbench`：`discover_roots/enable/disable/refresh`，`ExecutePluginCommand` 发布 `plugin.command.execute`。
- **Presentation** (`tool-panels/dynamic`) 仍在 `tool-panels`：`DynamicPanels::ingest` 消费 `ui.panel.create` 等 topic，`is_allowed` 鉴权。

## 如何新增能力

1. 在 `tool-application::command::AppCommand` 加变体
2. 在 `workbench.rs` 实现 `dispatch` 分支 + `query_*` 扩展（Application 侧**没有**终端读模型可扩：那份 `TerminalService` 第 46 轮按「零生产使用者」删除，终端数据只在呈现层，见「Terminal 高频数据路径」）；
   **`dispatch` 有两套实现**（native `workbench.rs` / wasm `application/src/web.rs`，变体数已实测不同：
   仅 native 有 `LoadReplay`，仅 web 有
   `InstallMarketplacePlugin`/`LoadReplayText`），只改一侧就是埋下「同一命令在两端行为不同」
   > **第 20 轮按这条清单查出并删掉的两个变体**：`ExportTerminal` 与 `ExportLog`（都是
   > `#[cfg(not(target_arch = "wasm32"))]`，所以 wasm 侧的穷尽 `match` 从来没见过它们）。
   > 两者**全仓零构造点**（含测试），插件层也拿不到 `AppCommand`（`lua_host`/`plugin_runtime`/
   > `extension` 里 0 引用），于是 `ExportTerminal` 的处理器 `dispatch_export_terminal` 成了
   > 终端导出的**第二份实现**（与活路径同样落到 `write_exported_content`，连带
   > `ServiceTerminal::export_job` → `TerminalExportJob` → `render_export` → `csv_escape`/
   > `format_dir` 一条只被它用的链，共 −111 行）；而 `ExportLog` 的处理器是个**明确写着占位**的
   > no-op。占位背后那条设计约束留着，别顺手改回来：**日志导出归 `LogPanel`（presentation），
   > application 层不碰 recorder**。
   > 终端导出的活路径只有一条：`crates/app/src/commands.rs` 的
   > `export_terminal_data` → `Workbench::spawn_file_export` → `write_exported_content`
   > —— 别再在 `dispatch` 里加第三个入口。
3. 判定/解码类逻辑放**无 `cfg` 门控**的共享模块（发送侧的先例是 `send_plan::plan_send`），
   不要在两个 `dispatch` 里各抄一份 —— HEX 解析曾抄到 4 份，后果即两端判定相反
4. 写 `crates/application/tests/headless.rs` 用例（不启动 egui）
5. 最后在 `crates/panels` 加渲染、`crates/app` 加 `UiCommand`/快捷键

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
| Application (`tool-application`) | 行为与状态 | `Workbench::dispatch(AppCommand)->Result<CommandOutcome,AppError>` / `query_*` / `tick(now)` / `ApplicationConfig` |
| Presentation (`tool-panels` + `app` UI) | 呈现与交互 | `TerminalPanel/LogPanel/ChartPanel + DynamicPanels` / `PanelRegistry::Builtin(fn(&mut WorkbenchApp,&mut Ui))` / `CommandPalette` / `rfd` |

判定规则：换个 UI 仍需知道即 `Application`，仅用于当前 `egui` 的 `scroll/selection/hover/dock` 即 `Presentation`.

## Terminal 高频数据路径（native 只剩一条：第 46 轮）

活的那一条全在呈现层：`crates/panels/src/terminal.rs` 的 `TerminalPanel::new(&bus)` 自己
`subscribe_ring_bounded(TopicFilter::prefix("transport.serial."), 65_536)`、自己持有那份
`TerminalStore`，每帧由组合根调 `ingest_pending()`（native `crates/app/src/runtime/plugin.rs`、
web `crates/app/src/web/mod.rs`，两边都进 `record_terminal_ingest` 的 perf 采样），预算
`MAX_INGEST_PER_FRAME = 500`；`push_event` 把「按 idle 定稿 / ≤`MAX_TERMINAL_BLOCK_BYTES` 分块」
交给共用的 `TerminalAssembler` + `TerminalStore`，`set_max_entries` 夹 `max(100)` 后由
`trim_to_limit` 按 seq 淘汰最旧。**两项用户设置活的落点就是这里**：`terminal_merge_window_ms`
在 `push_event` 里同步成 `assembler.idle_finalize_ms`，`terminal_max_entries` 经
`set_max_entries`→`enforce_max_entries`，两端各自还有 `app/mod.rs::apply_persisted_panel_settings`
（启动回填）、`config.rs::load_config_from_path`（重载）、`ui/settings_panel.rs`（用户改值）三处入口。

**曾经并存的第二条（本轮删除）**：`crates/application/src/service/terminal.rs`（145 行）+
`crates/application/src/model/terminal.rs`（27 行）里那份 `TerminalService` 用同一个前缀**再开一条
65_536 环**（`subscribe_ring_bounded` 是 `VecDeque::with_capacity`，实测 `size_of::<Event>() = 136`
→ **8,912,896 B ≈ 8.5 MiB 在 `Workbench::new` 当场分配**），再配第二份 `TerminalStore`
（`DEFAULT_MAX_ENTRIES = 50_000`），`Workbench::tick` 每帧 `drain_limited(2048)` 把同一批字节重新
装配一遍。release 实测（同一夹具、600 帧、删前/删后各跑一遍）：

| 会话形态 | 删前 `Workbench::tick()` | 删后 | 幻影调用自身（插桩单测） |
|---|---|---|---|
| 空载（0 事件/帧） | 74.0 ns/帧 | 79.8 ns/帧 | 45.5 ns/帧，store 0 条 |
| 115200 连续流（1×192 B/帧） | 1,501.3 ns/帧 | 47.5 ns/帧 | 1,487.2 ns/帧，store 600 条 |
| 突发（8×1 KB/帧） | 14,862.7 ns/帧 | 46.3 ns/帧 | 14,436.8 ns/帧，store 4,800 条 |

即 **application 层的每帧终端开销从「与面板同量级」降到与流量无关的 ~50 ns 噪声带**；空载那一行
删前删后都在噪声里（真正消失的是 45 ns 与那条环的 8.5 MiB），有流量时才是实打实的第二份装配
工作（1.45 µs/帧与 14.8 µs/帧）。面板那份代码两个构建完全相同，不参与对比。

**为什么四道门从没抓到它（cfg 根因）**：`service/terminal.rs` 挂在
`#[cfg(not(target_arch = "wasm32"))] pub(crate) mod terminal;` 上，`model` 整个模块也是
`cfg(not(target_arch = "wasm32"))` —— wasm 那道 clippy 连这个模块都看不见，那条重复订阅对
web 平台的门天然隐形；native 那道 `dead_code` 又因为它被 `tick`/`dispatch` 调着而必然沉默。
于是「第二条订阅」在两边的门里都不显眼，只有按符号查使用者才看得见：它的 9 处调用 + 2 条用例
全在 `crates/application/tests/headless.rs`，生产侧 0 读者。`tool-panels` 依赖 `tool-application`、
反向没有边（加依赖就是往守卫在乎的架构里新增一条边），所以那些断言无法改指活面板 —— 本轮按
「删代码即删其自测用例」处理。仍在守的语义：稳定 seq 与「清空后游标不回退」由 `TerminalStore`
自己的 `clear_keeps_terminal_ids_monotonic_for_incremental_clients` 守，条数上限与按 seq 淘汰由
面板的 `max_entries_is_global_across_ports_and_keeps_newest_events` /
`lowering_max_entries_trims_immediately` 守；面板侧不存在 `entries_since` 那种「调用方持游标分页」
的读法（导出走 `TerminalStore::iter_after(Some(last_id))`，是另一件事）。

## WorkbenchApp 定位（已收敛）

`WorkbenchApp { workbench: Workbench, panels/*, send, notifications, ... }` — `bus/transport/recorder/plugin_manager` 均经 `workbench.*` 代理，`app` 仅剩 `eframe` 生命周期、`dock/shortcuts/dialogs/themes`。原 `Self{ bus/transport/plugin_manager/recorder }` 重复字段**已物理删除**（不再是待办）。

## 验证

CI 的门禁**分散在三个作业里，一共不止三条命令** —— 别按门的名字推覆盖面，下面每一条都对着 `.github/workflows/ci.yml` 核实过（toolchain 钉 `1.92.0`）。Windows「Test & Lint」作业（`test`）跑：

```bash
cargo fmt --all --check                                   # 工作区级
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo test -p tool-panels -p tool-databus -- --ignored     # step 名「Pressure gates」
```

- 第四条常被漏：`tool-panels` / `tool-databus` 里那批 `#[ignore]` 用例的断言是**结构性**的（行索引保持完整、无损订阅不丢事件），debug 下就能当回归门禁 ⇒ **改动落在这两个 crate 上就必须连它一起跑**，否则会出现"本机三件套全绿、CI 红"。故意 ignore 的**计时**用例不在这一条里（它们按 `docs/performance.md` 手工 `--release --nocapture` 采集，且这一条刻意只选这两个 crate，免得把 `crates/app` 里依赖安静调度的计时测试拉进 CI）。
- 「WASM check」作业（`wasm`）：`cargo check` 与 `cargo clippy --target wasm32-unknown-unknown --workspace --exclude tool-extension --exclude tool-lua-host --exclude tool-marketplace --exclude tool-transport --exclude tool-updater`（第 60 轮起从"只 lint 主程序一个包"扩到全部能在 wasm32 上编译的成员；这 5 个 crate 在 wasm 下编不过，裸 `--workspace` 会在 getrandom 上直接失败）。**原生构建会把 wasm-only 模块整个排除**，所以只跑原生那几条时，`cfg(wasm32)` 里的改动等于没验。
- Ubuntu「.deb」作业（`linux-deb`）：`cargo check --workspace` → `cargo clippy --workspace --all-targets --exclude hardware-workbench-app -- -D warnings` → 再一条**不带** `--exclude` 的整包 clippy（告警继续打印但不红作业）→ `cargo test --workspace --all-targets`。`--exclude` 的来由见下文「Linux clippy 的 `-D warnings` 缺口」。

本机复跑（`<TOOLCHAIN>` 换成 `+1.92.0` 以对齐 CI）：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --target wasm32-unknown-unknown --workspace --exclude tool-extension --exclude tool-lua-host --exclude tool-marketplace --exclude tool-transport --exclude tool-updater -- -D warnings
cargo test --workspace --all-targets
cargo test -p tool-panels -p tool-databus -- --ignored   # 只在动了这两个 crate 时是硬门
cargo tree -p tool-panels | grep -E '^[├└]── '           # 顶格 = depth-1 直接依赖；用 '^\s*' 会误捕 depth-2+
cargo tree -p tool-application | grep -i egui            # 0 行
```

注意事项（都是实测结论，别照抄旧写法）：

- `Cargo.toml` 有 `default-members = ["crates/app"]`，因此**裸跑** `cargo clippy --all-targets` /
  `cargo test --all-targets` 只覆盖 `crates/app` 一个包，`crates/application/tests/*.rs` 与其他 crate
  既不执行也不被 lint。必须带 `--workspace`（CI 已改为带 `--workspace`）。
- `rg "egui|eframe" crates/application/src` **不为 0 行**：第 46 轮删掉幻影终端管道与 `model` 模块后剩
  **3 行**（`task.rs` 与 `workbench.rs` 的 worker 文档在说「不外借 UI 状态」「不在 egui 回调里写大文件」，
  `workbench.rs` 的导出闭包同理）。**别按行号核对，行号一直在漂**：上一版记的 `task.rs:136`、
  `workbench.rs:240` 现在分别是 137、229，而它列的 `model/mod.rs:1`、`service/terminal.rs:3` 已经不存在。
  判断 UI 隔离要看 `Cargo.toml` 与 `crates/application/tests/architecture.rs`，不要用这条 grep。
- MSRV 差异：本机 stable 是 `1.98.0`，CI 钉 `1.92.0`。**必须用 `+1.92.0` 复跑**——历史上两套工具链
  对 lint 的判定确实相反，并因此暴露过两个 red gate（**均已修复**）：
  * `clippy::nonminimal_bool` @ `crates/application/src/workbench.rs` 的 `dispatch` 连接分支（原 `:1158`，
    该文件已随 dispatch 拆分移位，按符号找）：`!network.is_some()` → `network.is_none()`，t11 等价重写，无 lint 豁免）；
  * `clippy::let_and_return` @ `crates/app/src/panel_registry.rs:73-79`（整个 wasm-only 的 `web()`；
    收敛后的返回表达式在 `:78`）：t13 去掉中间的 `let registry = …;` 直接返回，无 lint 豁免。
  > **现存豁免的清单不在本文重复维护** —— 唯一真相是
  > `crates/app/tests/lint_exemptions.rs` 里钉住的那份（扫描 `crates/**/*.rs`，按
  > 「文件 + lint → 条数」比对；第 17 轮重测后是 10 条：7 条 clippy + 3 条 `dead_code`，
  > 三条各自的形状（两种 `cfg_attr` + 一条普通 `#[allow]`）写在那份守卫的注释里，
  > 对应下面 Linux / wasm 死代码那张表）。加一条豁免必须显式改那份清单，测试会红；
  > 摘一条则应同时把清单收紧。本仓第 6 轮一次摘光后发现 3 条早已空转，
  > 第 8 轮把两处渲染参数墙（`render_rows_view` 20 参、`render_log_rows` 14 参）
  > 收进命名字段输入结构体后又退掉 2 条 `too_many_arguments`，
  > **第 17 轮再摘光重测，又判出 6 条空转**（`panel_registry.rs` 的 `PanelKind` 一条 +
  > `gauge.rs` 的 5 条：`ZoneKind` / `from_config` / `parse_zones` / `parse_zone_color` /
  > `parse_zone_kind`）—— 它们护着的东西在 wasm 上早就是活引用了：`PanelKind` 在
  > `crates/app/src/web/mod.rs` 里既被构造也被 `match`，gauge 那几个由
  > `crates/panels/src/dynamic/ingest.rs` 在总线驱动下调用，而 `DynamicPanels` 在 wasm 侧是活的。
  > **判空转的正确做法（这一条值钱在避坑）**：一次摘光之后**分别**跑两条 clippy ——
  > native 的 `--workspace --all-targets` 与 wasm 那条（`--target wasm32-unknown-unknown --workspace`
  > 减 5 个 native-only crate，作用域见文首 CI 小节）
  > —— 两边都不点名的才是死的。只跑 native 会得出完全相反的结论：`cfg_attr(target_arch =
  > "wasm32", …)` 护着的东西在 native 构建里本来就活着，native 永远不会抱怨它。
  > 另外**别把同文件的另一条豁免顺手带走**：`gauge.rs` 那条 `too_many_arguments` 在 wasm
  > 侧仍被点名，摘光时它必须回来，否则 wasm 门直接红。
  > **第 28 轮按这套流程重查了一次：10 条全部复发，一条没删**（各站点的新证据：参数最少 8 个 ——
  > 不是这里曾写过的"最少 9 个"，`ConfigLoadResult` 两臂体积差 ≥1089B vs ≥88B）。
  > 同一个坑的**第三种形状，比"只跑 native"更阴**：`-D warnings` 下某个 crate 编译失败，
  > 依赖它的 crate **根本不会被编译**，所以"摘光全部豁免"可能只报出最前面几个站点就收工
  > （第 28 轮 native 停在 `tool-transport`，`crates/app` 那 4 条完全没被看到）。
  > 照字面执行会把"没检查"读成"没复发"，正好是"只跑 native"那个错误的镜像。
  > **必须再去掉 `-D warnings` 各跑一遍**收集全部告警，才谈得上判空转。
  > 顺带一条取数的教训：想数"自第 N 轮以来某文件改了几次"，用
  > `git rev-list --count <第 N 轮的提交>..HEAD -- <文件>`；`git log --grep=… | tail -1`
  > 抓到的可能是更早的、正文里偶然提到同一词的提交，于是把历史全算进去
  > （第 28 轮就是这么得到"每个被豁免文件都改过"这个假前提的 —— 实际只有 `config.rs` 变了 3 次）。
  >
  > **第 29 轮把其中 2 条 `too_many_arguments` 真的消掉了，不是摘下来**（棘轮 10 → 8）：
  > `transport/src/network.rs` 的 `spawn_network_worker` 与 `network_worker_loop` 收同一串
  > **8 个位置参数**，其中 `stop` / `alive` / `connecting` 是**三个相邻的同型
  > `Arc<AtomicBool>`**。实测三种对调（stop↔alive、alive↔connecting、stop↔connecting）
  > `cargo check --workspace` **全部零告警通过** —— 危险是真的。但"测试能不能抓到"要分开说：
  > `alive`↔`connecting`（初值都是 `true`，唯一同值的一对）**让 `cargo test -p tool-transport`
  > 32/32 全绿**，只有 `crates/application/tests/headless.rs` 的网络用例抓得住；
  > 另两种之所以红，是因为 `stop` 初值为 `false` 而另两个为 `true`，换错的 worker 读到
  > `stop == true` 直接在握手前退出 —— **是初值救了测试，不是测试设计到位**。
  > 收成一个 `pub(crate) struct NetworkWorkerContext`（沿用第 8 轮 `RowsViewInputs` 那一式，
  > 无 trait / 无泛型 / 无 builder）之后，两条 `#[allow]` 删除，函数各自只剩一个参数，
  > 调用点六个 `thread_*` 临时别名剩一个（`waker` 必须留在外面取，否则 `parking_lot` 的守卫
  > 会跨 `thread::spawn` 存活 —— 与原代码同一做法）。
  > **准确的说法**：命名字段拦的是**写法**不是**取值** —— 位置错排在结构体字面量语法上不存在了、
  > 漏字段 E0063、把非标记塞进标记位 E0308，但 `alive: Arc::clone(&connecting)` 这种同型对调
  > 照样编译得过（实测），只是必须把矛盾写在字段名旁边。这条限度也写进了结构体的 doc，
  > 免得后来人把它读成类型层的保证。
  > 顺带：那三个标记各自第一次有了自己的注释（关闭请求 / `AliveGuard` 负责的存活位 /
  > 握手完成或失败后置假的连接位），原先它们只存在于名字里。
  >
  > **第 30 轮同法收掉 `panels/lib.rs::next_auto_scroll_state`，棘轮 8 → 7**（3 个相邻 bool +
  > 4 个相邻 f32；6 组相邻同型对调里 **5 组零告警编译得过**，唯一的例外是 `bool`↔`f32` 那组）。
  > 普查（在改动前的树上做，且**分两层**：A 换调用点实参、B 换签名形参）出来两条**比我原判断更严重**
  > 与**和我原判断相反**的结论，都记下来：
  > ① **A 层六行全静默** —— 因为全仓没有任何用例驱动过那个唯一的生产调用点
  > （`table.rs` 的 `AutoScrollState::update`），所以"命名巧合"连一层测试兜底都没有；
  > ② **真正会静默上线的那一对不是我以为的 `offset_y`↔`previous_offset_y`**（它至少有
  > 1 个用例能区分，虽然另有两条用例两值都是 `100.0`、构造上永远分辨不出 —— 与第 29 轮
  > "初值救测试"同一形状），而是 **`content_height`↔`viewport_height`**：换过之后
  > `(content - viewport).max(0.0)` 钳成 0，`at_bottom` 恒真，**现有全部期望值照旧成立**。
  > 另记一处结构管不到的重复：`table.rs:241` 为了算"该不该修复贴底"又**独立**按同一顺序
  > 把 `(offset_y, content_height, viewport_height)` 传给 `scroll_is_at_bottom` 一次
  > （3 参，不触发 `too_many_arguments`，所以那处没有豁免也没有本轮的命名保护）。
  > **第 31 轮把那六行从"静默"变成"对调即红"**（711 → 718，多的是 7 条新用例）：`table.rs`
  > 的测试模块新增一套 `Frame` + `drive()` 的式子，**按真实帧**驱动 `AutoScrollState::update`，
  > 每只用例只改自己判别的那一两个字段。普查 A 层重跑：**六对同型字段全部至少被一条用例抓到**
  > （最薄的一对是 `content_height`↔`viewport_height`，恰好 2 条 —— 删用例时要记得），
  > 另加 5 个探针（`scroll_is_at_bottom` 那两处位置对调、`previous_offsets` 收成共用一份、
  > 悬停注到别处、按下事件丢掉）也都红。那处三参重复按"只绑一次 `viewport_height`"收掉，
  > 没动任何签名。**仍无覆盖的是别的东西**：`update` 之外没有别的调用点，而面板整体的
  > 像素级行为照旧只能在真窗口里看。
  > **顺带查到的 egui 0.35 注入事实（本仓以后写 UI 用例都要用）**：`RawInput` **已经没有
  > `pointer` 字段**，`hover_pos()` / `primary_down()` 只能由 `events` 里的
  > `Event::PointerMoved` / `Event::PointerButton` 推出来。两个坑：① `PointerButton`
  > 无论按下还是抬起**都会顺带写下位置**，所以"本帧没有悬停"必须**一个指针事件都不发**，
  > 补一个抬起会把"无悬停"污染成"悬停在别处"；探针据此发现了我第一版自检用例是**假绿**的
  > （只用按下帧，破坏 `PointerMoved` 的变异体照样通过自检）。② egui 跨 pass 保留指针状态，
  > 所以"无悬停"那一帧要**换一个新 `Context`**，在同一 ctx 上再跑一次 `RawInput::default()`
  > 读到的是上一帧的位置。每帧一个新 `Context` 还有个好处：`update` 只读指针、只写自己那份
  > map/set，帧间连续性全在 `AutoScrollState` 里，断言就不依赖 egui 的重绘调度了。
  >
  > **第 32 轮把 `gauge.rs::draw_arc` 的弧线几何抽成可测函数，棘轮 7 → 6**（718 → 723）。
  > 原先 9 个位置参数里有 `radius`/`start_deg`/`sweep_deg`/`from_t`/`to_t` **五个相邻 f32**，
  > 而"前两个是角度、后两个是 [0,1] 归一化位置"只写在签名上方那句注释里。改动前逐对普查：
  > **4/4 编译零告警，且 `tool-panels` 209 条测试全绿**（gauge 原有 11 条用例只管取值/分区/配色/
  > 状态文字，一条都不碰弧线）。做法是把纯计算抽出来：`ArcSpec{center,radius,start_deg,sweep_deg,
  > from_t,to_t}` + `arc_points(&ArcSpec, steps) -> Vec<Pos2>`，`draw_arc` 只剩"取点 +
  > `len() >= 2` 才 `painter.line`"四个参数，`#[allow]` 删除。
  > **渲染等价不靠"能编译"糊过去**：临时逐字符抄了一份 `git show HEAD` 的旧实现，在
  > 3×4×5×4×6×6×5 的输入网格上按 `to_bits()` **逐位**比了 **1,520,640 个点、0 处不等**，
  > 随后把这份临时用例删掉（公式在测试里留第二份，早晚和实现各说各话）；事后我独立复核过，
  > 新函数体与旧循环**逐行同序**，只差 `spec.` 前缀。新增 5 条用例每对相邻字段至少被一条抓到
  > （把 `arc_points` 内的字段读法改成对调来验，最小偏离 90° 弧度 / 105 px 半径，
  > 而浮点噪声 ≤1e-5，所以不是踩 epsilon 边界蒙过的）。`steps == 0` 那个 `0/0 = NaN` 的形状
  > **没有偷偷"修"**：三处调用点共用同一个 `arc_steps = 60` 绑定，0 不可达，真到 0 也不画，
  > 现状用一条用例钉住并注明"要改是有意的决定"。
  > **本轮明确没有做到的**：危险没消失，它**搬了家**。用一个合法的 `ArcSpec` 字面量把
  > `sweep_deg` 的 270.0 塞进底轨的 `from_t` 槽位 ⇒ clippy 零告警、214 条测试照旧全绿。
  > 新用例钉的是**字段名在 `arc_points` 里的含义**，没有东西钉 `paint` 往这些字段**填了什么**；
  > 那一层要 `Painter`/`Ui` 驱动，第 31 轮那套事件注入只对纯判定管用，所以停手而不是再造一层抽象。
  > 结论要说准：`from_t: sweep_deg` 如今在源码里自我暴露，但**"参数墙收掉"≠"调用点有覆盖"**。
  >
  > **第 33 轮删掉 `serial.rs::render_port_editor` 末尾那段恒等价的同步块，同法收掉参数墙，
  > 棘轮 6 → 5**（723 → 725，多的 2 条是一条源码形状守卫拆出来的两个用例）。这段
  > `if selected_group != current_group { … }` 带着"为未来调用者兜底"的注释活了很久，普查证明
  > 那个调用者不存在、于是它是**逐字重复**：`current_group` 全文件只被它自己读；两个调用点都
  > `let mut selected_group = group_name.to_owned();` 并把同一个 `group_name` 当 `current_group`
  > 传入，进入时必然相等；唯一能改它的是 `ComboBox::show_ui` 里的两个 `selectable_value`，而
  > **egui 0.35 把"写值"和"标脏"放在同一支**（`src/ui.rs:1944`
  > `if response.clicked() && *current_value != selected_value { *current_value = selected_value;
  > response.mark_changed(); }`；`Response::changed()` 读的就是这个 flag，`src/response.rs:593/605`），
  > 所以"不同于初值"⟺那两个 `if … .changed()` 本帧已跑过，而它们做的 `remove`/`insert` 与那段
  > 一模一样。按"不留投机兜底"删块 + 删参数，注释一并删掉。参数墙沿用第 29/30/32 轮那一式：
  > `PortEditorInputs`（字段名与原参数逐字相同，私有、无 trait/泛型/builder），
  > 参数 **8 → 2**、函数 **68 → 53 行**、`#[allow]` 删除，公共 API 与 serde 字段不动。
  > **一处结构上被迫的偏离**：`metadata` 不能写成 `&'a mut SerialPortMetadata<'a>` —— `&mut`
  > 对内部生命周期**不变**，那等于要求元数据引用的数据活得和调用点局部 `alias`/`selected_group`
  > 一样久，实测 8 条错（4×E0597 + E0507 + 3×lifetime）；rustc 给的建议是把 `render_grouped_port`
  > 全部参数并成一个生命周期，那是**把墙摊开**不是收掉。正解是单给 `'b`，也就是原参数
  > `&mut SerialPortMetadata<'_>` 本来就省略掉的两个生命周期。另记：闭包要同时握
  > `selected_group` 与 `metadata`，所以字段必须在函数开头解构成局部量（同 `RowsViewInputs`）。
  > **本轮的"覆盖"答案与前几轮不同，要说白**：这条路径改动前后**都没有任何行为用例**驱动
  > （`crates/` 下 grep `grouped_port_list_ui` / `SerialPortMetadata` 零测试命中，消费者只有
  > `device_panel.rs:347` 与 `web/serial.rs:265`），而第 31 轮那套事件注入要真 `Context` 才测得出
  > 下拉的点击，本轮没有为此再造一层抽象。所以补的是**源码形状守卫**：读文件、归一行尾（这步不是
  > 装饰：`.gitattributes` 钉了 `*.rs text eol=lf`、`serial.rs` 也确实是纯 LF，但盘上
  > `app/src/config.rs` 与 `core/src/lib.rs` 两个文件是**整文件 CRLF**，赌不得）、把注释
  > 与字符串内容抹成空格再扫，钉 `current_group` 全文件 0 次、两处调用点各一条同名绑定、
  > `PortEditorInputs {` 字面量里不得有 `X: &mut Y` 且 `X != Y`，外加两条防空转下界
  > （字面量恰好 2 个、函数体顶层语句 ≥11 条）。**红证明在两个调用点各做了一遍**（第二遍故意挑
  > 窄屏那一支，证明确实是逐个字面量在查而不是盯死一处）——
  > 两个版本都**编译零告警通过**，守卫随即红并点名 `alias: &mut selected_group` /
  > `selected_group: &mut alias` 两侧，恢复后回绿。所以结论是：写错**会在调用点源码里自我暴露，
  > 并且被这条形状守卫生效**，不能说成"错误无法表达"。
  **当前状态（修复后）**：判据是本文「验证」一节**按作业列出的那几条门** —— fmt、整包
  `clippy --workspace --all-targets -- -D warnings`、wasm 那道 clippy、整包测试，**外加**改动落在
  `tool-panels` / `tool-databus` 时要跑的 `--ignored` 压力门禁。早先这里写作"三条均 **exit 0**"，
  那是**按门的名字推覆盖面**的说法，留着会误导下一轮：原生构建把 wasm-only 模块整个排除，
  只跑原生那三条时，`cfg(wasm32)` 里的改动等于没验。
  用例数**不要按这里的数字判断对错**，它每轮都会随新用例增长（写下这一段时是 590，
  第 10 轮「九处传输错误接中文映射」复核时是 675，`ignored` 一直是 7 条依赖安静调度的计时用例，
  它们刻意不进 CI）；判据始终是那些门各自的退出码。
  历史构成（590 = Task 6 落 `send_plan` 前的
  578 + `send_plan` 的 6 条单测，该模块无 `cfg` 门控所以跑在 native 测试目标里
  + Task 6 复审轮的 1 条 `headless.rs` 端口命名形态用例
  + 终审修波的 5 条：`crates/panels/src/sender.rs` 的 HEX 门禁 3 条、
  `crates/recorder/src/recorder.rs` 的「Stop 之后 panic」2 条）。
  > **计数口径（引用数字前先看这条）**：`cargo test` 打的是**测试槽位**，不是唯一断言数。
  > `crates/app/tests/manifest_deps.rs` 既是独立 test target，又被两份守卫各用 `#[path]`
  > 引一次，所以它的 13 条元测试在 **3 个 target 各跑一遍**（13 条 → 39 槽）。
  > 架构守卫这轮改动的净增量因此是 **15 条唯一用例 / 41 个槽位**（532 → 573），
  > **不是**「多了 41 条测试」。把槽位数当覆盖数会高估 3 倍。
  注意 CI 的 `wasm` 作业里那一步名为 **「Lint Web target」** 的 clippy **同样带 `-D warnings`**，
  且必须用 `--target wasm32-unknown-unknown` 才会 lint 到 wasm-only 代码 —— 只跑宿主侧门抓不到上面第二个红门。
  （这里按**步骤名**引用而不写行号：`ci.yml` 的行号会随任何一次插入漂移，本分支就出现过
  一次 —— Task 4 在 Linux 作业里加了 8 行，把当时记成 `:195` 的这一步推到了别处。）

### Linux clippy 的 `-D warnings` 缺口

Windows 作业的 clippy 带 `-D warnings`，Linux 作业过去不带 —— clippy 回归能在 Linux 侧静默通过。
本轮把 `crates/transport` 里**只有非 Windows 构建才会出现**的两个警告修掉了：

- `unused variable: wake`（`open_serial` 的 `#[cfg(not(windows))]` 分支）：那个元组槽只为对齐
  Windows 分支而存在，`PortHandle::wake` 本身是 `#[cfg(windows)]` 字段。现在该分支只绑 `join`，
  块尾直接返回 `thread::spawn(..)`（不留中间 `let`，否则触发 `clippy::let_and_return`）。
- `unreachable_expression`（`serial_permission_message`）：`#[cfg(target_os = "linux")]` 块以
  `return format!(..)` 结尾，后面还跟着无条件尾表达式，Linux 上那段是死代码。改为互斥 cfg 块对，
  两条消息文本逐字不变。

**但整包 `-D warnings` 仍会把 Linux 作业当场变红**：`crates/app` 的自动更新链路在 Linux 下被
`cfg(not(target_os = "linux"))` 关掉后，剩下 9 处死代码诊断（实测 12 条，分布在 bin 与 bin-test 两次构建）：

| 位置 | 诊断 |
| --- | --- |
| `crates/app/src/runtime/update.rs:157` | `start_update_check` / `start_update_download` / `force_check_update` never used |
| `crates/app/src/state.rs:407` | `UpdateState` 多字段 never read |
| `crates/app/src/state.rs:443` | `CheckResult` 的 `version`/`download_url`/`sha256`/`cached` never read |
| `crates/app/src/state.rs:447` | `CheckResult::changelog` never read |
| `crates/app/src/state.rs:462` | `UpdateState::pinned_update_sha256` never used |
| `crates/app/src/state.rs:478` | `cached_check_result` never used |
| `crates/app/src/ui/status_bar.rs:256` | `clippy::needless_return`（Linux 早返回块） |
| `crates/app/src/ui/status_bar.rs:266` | `draw_update_icon` never used |
| `crates/app/src/workbench_app.rs:78` | `WorkbenchApp::update_state` never read |

> 这不是「本地跑不了所以不知道」—— 上面这张表是用 **cfg 形状复刻**在 Windows 宿主上实测得到的：
> 把源码里的 `#[cfg(windows)]`/`#[cfg(target_os = "windows")]` 换成 `#[cfg(target_arch = "wasm32")]`
> （本机为假），`#[cfg(not(windows))]`/`#[cfg(target_os = "linux")]` 换成
> `#[cfg(not(target_arch = "wasm32"))]`（本机为真），宿主构建编译的就是 Linux 原生构建的那一半代码，
> 依赖无需交叉编译。39 个站点全部映射、无残留后跑
> `cargo clippy --workspace --all-targets -- -D warnings`，报错**只**落在 `hardware-workbench-app`
> 一个包上 —— 即 `tool-transport` / `tool-updater` 与其余 12 个 crate 的 Linux 形状是干净的。
> 复刻完 `git checkout -- crates/` 还原。

**因此 `.github/workflows/ci.yml` 的 Linux clippy 步骤改为**：
`cargo clippy --workspace --all-targets --exclude hardware-workbench-app -- -D warnings`
+ 原有的整包 `cargo clippy --workspace --all-targets`（app 的警告继续打印，但不红作业）。
补齐 app 的 Linux cfg 覆盖（给更新链路加 cfg，或让 Linux 真正启用更新器）后删掉 `--exclude` 即完全对齐。

本轮实际跑过的非 Windows 验证（`libudev-sys` 的 build script 需要 Linux sysroot，
`--target x86_64-unknown-linux-gnu` 在本机不可用）：

```bash
# 不含 C 依赖的 6 个 crate，用 android 三重作为 not(windows) 代理 —— exit 0
cargo +1.92.0 clippy -p tool-transport -p tool-platform -p tool-core -p tool-databus \
        -p tool-recorder -p tool-testing --target x86_64-linux-android --all-targets -- -D warnings
# 整包 Linux cfg 形状（复刻配方见上）：只有 hardware-workbench-app 报错
# 全仓 grep：平台相关 cfg 站点只出现在 app / transport / updater，且没有任何 cfg(unix) 代码
```

### 测试随机红的一类：`sleep(N)` 之后断言「后台线程已经赶到」

**形状**：`enable` / `dispatch` / `thread::spawn` → `thread::sleep(N ms)` → 断言后台线程的效果此刻已经可见。
N 是「本机通常够快」的猜测，不是任何契约：机器被别的 cargo 作业占满时先红的是断言，看起来像产品 bug。
第 36 轮把这条**先量出来再改**（本机 16 核，配方见下），改前改后同配方复跑：

| 用例（改前的红点） | cpu×2 · churn×4 | 改后（cpu×2 · churn×4 · churn×6） |
| --- | --- | --- |
| `headless_workbench_can_dispatch_and_query` @ `crates/application/tests/headless.rs:58` | **12/20** · **27/30** | **0/20** · **0/30** · **0/20** |
| `manager::tests::discovers_manifest_and_enables_lua_plugin` @ `crates/extension/src/manager.rs:947` | 0/20 · **7/30** | **0/20** · **0/30** · **0/20** |

`cpu` = 纯忙等 hog，`churn` = 每轮再分配 1 MiB 的忙等 hog，数量是 `cores × 倍数`；`N/M` = M 轮里红 N 轮。
两条红点原文都与编排器目击的一致（`assertion left == right failed: left: 0 right: 1` /
`assertion failed: wb.task_snapshots().iter().any(...)`）。**第二条在纯 CPU hog 下 20/30/40 三轮共 90 次都不红**，
只有把缓存一起搅浑才复现 —— 说明「赌时长」的失败面比「CPU 慢」更宽，只按纯 CPU 配方下结论会漏。

**规矩**（这两条用的判据，下一轮照抄）：

- 等**真条件**，不等时长：条件必须是「被断言的那件事本身」（任务落到 `Failed`、插件运行时被收割），
  而不是「轮数用完」或「睡完了」。
- 上界必须是**墙钟 deadline**，并且**等不到要大声失败**：消息里带 `Instant::now().elapsed()` 与最后一轮
  观测到的值。既不静默通过，也不无限挂。
- 断言异步**数量**时「恰好 N」不许降级成「至少 N」：先等生产者**已经收尾**（extension 那把钥匙是
  `reap_finished()` → 线程 `join`，`join` 本身就是「脚本里所有 `publish` 都可见」的边界），收尾之后
  一次性 `drain()` 再计数。轮询到第 1 条就停会悄悄放过重复发布。
- **不新增共享 helper**：headless 复用本文件已有的 `tick_until`（`crates/application/tests/headless.rs:151`，
  另有 30 处调用），extension 就地写 `while cond && elapsed < budget`（同仓 `headless.rs:637/794` 已是这形状）。
- 改完必须**证明它还能红**：两条用例共 5 个变异（重复发布 / 脚本根本不执行 / 运行时永不收尾 /
  连接错误被吞成成功 / 任务 kind 改名），全部还原并 `cmp` 逐字节核过 —— 消息见 CHANGELOG 同轮条目。

**复现配方**（脚本只在 `target/` 下，不入库、不加 dev-dependency；`--full` 跑整包并列出红的用例名）：

```bash
cargo +1.92.0 test -p tool-extension --lib -p tool-application --test headless --no-run
# 先 --runs 1 --load 0 确认名字过滤只命中 1 条用例（`--exact` 配部分名会跑 0 条还 exit 0）
python target/r36-load.py --runs 30 --load 4 --hog churn --phase before
python target/r36-load.py --runs 20 --load 2 --hog cpu   --targets extension,headless
python target/r36-load.py --runs 2 --load 4 --hog churn --full \
       --targets extension,lua_host,recorder,application,headless
```

> cargo 换一次依赖就换一次测试二进制的哈希名：拿旧哈希跑出来的「0 红」是自欺欺人。脚本按 mtime 取最新
> 并把文件名打在第一行，跑之前先核一眼。

**全仓 `thread::sleep` 54 处的三桶分类**（`grep -rn "thread::sleep" crates/ --include=*.rs` 命中
54 行；`crates/app/src/runtime/timing.rs:30` 是注释提及，真正的调用 **53 处** —— 第 37 轮把 `settle`
的一处定长睡眠换成 `settle_until` 的 2ms 步长 + 一处去重收尾的短排空，净 **+1**；第 39 轮把
`natural_runtime_end_also_requests_cleanup` 的 50×5ms 轮数循环并进 `settle_until` 唯一那处步长，
净 **−1**。`recorder.rs` 的 `sleep(500)` 换成同一条 10ms 轮询步长，不增不减）：

*(a) 生产码 —— 不是 flake，别动*：`application/src/workbench.rs:1255` 重连退避 ·
`transport/src/lib.rs:881` 关旧 worker 的带 deadline join · `updater/src/lib.rs:604/649/658/680`
helper 启动重试 / 等主程序退出的宽限 / 替换重试 · `app/src/runtime/timing.rs:60` 帧节拍
`wait_until_deadline`（睡眠就是它要实现的机制本身）· `app/src/app/mod.rs:39` Drop 里 2s deadline 的
join，超时降级 detach。**两条要盯着的**：`app/src/app/mod.rs:39` 目前没有任何断言依赖「线程真的退了」，
一旦将来有，它就从 (a) 掉进 (b)；`extension/src/scenario.rs:204/223` 是**产品码**
（`SerialPluginScenarioRunner`，`examples/plugin_scenario.rs` 在用），`pump(窗口)` 是场景 DSL
「等 N 毫秒」这一步的语义本身，不能改成「等条件」。

*(b) 测试里的同类 —— 下一轮队列*（严重度按上面 churn×4 整包 2 次的实测「见红」排；第 38 轮后
补一句：`scenario_covers_retry_cancel_and_timeout` 的红已经全部挪到下面的 #12，不再是 #1 那类
窗口到点；第 39 轮关掉 3 与 6，队列里现在剩 4/5/7/8/9/10/11/12 + 本轮新登记的 13）：

1. ~~`extension/src/scenario.rs:398`（用例 `scenario_covers_retry_cancel_and_timeout` 里
   `scenario.timeout_ms = 500`）—— 固定墙钟预算被当成通过条件。~~ **第 38 轮已修**：正向
   `ExpectTx` 改用实测兜底 `TX_BACKSTOP_MS = 5_000`（抬高窗口不花时间 —— `expect_event` 每 1ms
   轮询、命中即刻返回，空载整包仍 0.26s）。把窗口临时抬到 8000ms 让每步都命中、量「步骤开始 →
   命中」的墙钟（19 轮）：派发命令 → 该任务第一条 TX 最坏 **3270ms**（churn×4 跑整包时；cpu×2
   606ms；空载 1ms），同一个任务下一拍最坏 **630ms**（空载 40ms = fixture 的 `sleep_ms(40)`）——
   两档都远超 500ms，所以红的一直不止第 2 步。两档制（5000/1000）的初版被自己打脸：1000ms 那档
   在 churn×4 复红 1/8（红在第 3 步），于是并成一个数。同机背靠背复测（`N/M` = M 轮红 N 轮）：
   **正向窗口到点 改前 5/32 → 改后 0/16**；单跑本用例 cpu×2 0/20 · churn×4 0/30，改后同配方
   0/20 · 0/30。**跑整包 libtest（16 线程并行）才是见红的环境**；剩下的红**全部**在下条 #12，
   改前它就占 10/32 —— 改前那些跑在第 2 步早退、根本没走到第 8 步，所以两列不能直接相减。
2. ~~`extension/src/manager.rs` `settle()` —— 无条件睡满 20×25ms，7 处调用共用，每条白付 500ms。~~
   **第 37 轮已修**：换成按条件有界的 `settle_until(manager, budget, |m| …)`，7 处判据逐字取自各自主断言，
   见下方 (c)；churn×6 改前 4 条依赖 register 事件流的用例见红（8/10/5/4 每 30 轮），改后同一配方 **0/30**。
   同文件里剩下的 `natural_runtime_end_also_requests_cleanup`（下条）与 `scenario.rs` 用例仍不在本轮范围内。
3. ~~`recorder/src/recorder.rs:887`（`stats_updated_during_recording`）—— `sleep(500)` →
   `assert!(events_written > 0)`，**见红 1/2**（`recorder.rs:890 expected some events written, got 0`）。
   教科书形状。~~ **第 39 轮已修**：改成轮询 `rec.stats().events_written` 真离开 0（worker 每写一条
   与字节数同批递增；判据逐字取自随后那条断言，`let s = rec.stats();` 与那条 `assert!` 原样留着），
   30s 兜底、命中即返回。打点：「发布 → 首条写出」churn×4 跑整包最坏 **1.236s**、中位 52ms。
   **按轮交替**的背靠背复测（`target/r39-ab.py`，同一批 hog 下改前/改后二进制一轮一轮换着跑，
   机器抖动两边同摊）：整包 churn×4 改前 **2/10** → 改后 **0/10**、churn×6 **2/8 → 0/8**、
   cpu×2 **0/10 → 0/10**；单跑本用例两侧 0/20 与 0/30（**见红必须跑整包**，与 1/2 那次的结论同）。
   顺带：那 500ms 白等一没，`tool-recorder --lib` 空载整包 **0.52s → 0.12s**。
4. `application/tests/headless.rs:834`（`plugin_enable_disable_transitions_state_and_rejects_unknown_ids`）——
   `for _ in 0..200 { process_plugin_lifecycle(); if 能重新启用 { break } sleep(5) }` → `assert!(reenabled)`
   红在 `:836`，**见红 1/2**。预算是**轮数**不是墙钟。
5. `application/tests/headless.rs:1199/1240/1271`（`plugin_restart_handoff_…`）—— 同 4 的轮数预算形状，
   其中 `:1271` 那一段轮完并不断言（只是收尾），红的是前两段。
6. ~~`extension/src/manager.rs:1499`（`natural_runtime_end_also_requests_cleanup`）—— 50×5ms 轮数预算 +
   事后 `assert_eq!(take_cleanup_requests(), …)`；本轮同配方未单独见红，机制与 4 相同。~~
   **第 39 轮已修**（编排器在 64 个搅缓存 hog 下亲目击红：`manager.rs:1505 left: [] right:
   ["panel.finished"]`）。复用同文件的 `settle_until` 而不是再写一个 helper：给它加一个 **pump
   闭包参数**，由调用点说明「推进这条条件的是哪个入口」—— 这里 `reap_finished`，另 7 处仍
   `process_pending`。**为什么不干脆两种都排**：`process_pending` 不回收运行时，把 `reap_finished`
   并进 helper 会把 `cmd.notfound` / `cmd.late` 两处「等插件处于 Running」的判据提前踢出 Running，
   修一处 flake 造两处假红。判据 `!cleanup_requests.is_empty()` 逐字取自随后那条 `assert_eq!`，
   "恰好一条"没降级（判据成立时该插件已被移出 `lua_runtimes`，不再可能追加）。打点：
   「enable → 清理请求出现」churn×4 最坏 **1.387s**（旧 250ms 预算连它的零头都不够，这就是红的机制），
   兜底 30s ≈ 21 倍。整包按轮交替复测：cpu×2 **3/10 → 0/10**、churn×4 **3/10 → 0/10**、
   churn×6 **1/8 → 0/8**；单跑本用例两侧 0/20 与 0/30。
7. `lua_host/src/lib.rs:1489/1514/1541/1561/1583/1607/1630/1794` —— `thread::spawn` 里 `sleep(25)` 再发布，
   脚本侧 `ctx.bus.wait(topic, 500)` 用自己的预算接：**赌的是「刺激方一定在 475ms 内被调度到」**。
   2/2 未见红（同族里概率最低，机制一模一样）。
8. `lua_host/src/lib.rs:1346/1348` —— `sleep(250)` → `inject_rx` → `wait_for_log(…)`：注入要落在插件正
   `expect` 的窗口里，机器慢就错过。未见红。
9. `app/src/runtime/timing.rs:182`（`wait_until_deadline_returns_early_on_cancel`）—— 另一线程 `sleep(10)`
   置 cancel，断言 `elapsed < 500ms`：把「响应延迟」钉成断言时，负载直接吃掉余量。未见红。
10. `application/src/task.rs:342/369/394` —— `for _ in 0..100/200 { drain_events(); if 到位 { return/break }
    sleep(1) }`，预算用尽后 `panic!("task did not complete")`：**已经大声**，但预算仍是轮数。
    （`:355` 是任务体里「我在干活」的模拟，不是等待。）
11. `transport/src/lib.rs:1735` —— `sleep(50)` → `stop` → `join`，注释自己写明「主要验证不 panic」：
    没有断言依赖这次睡眠，所以不会假红，但这条用例也没钉住任何行为 —— 属于「待加强」，不是 flake。
12. `extension/src/scenario.rs:398` 同一条用例的第 8 步 `ExpectNoTx { timeout_ms: Some(80) }` ——
    **第 38 轮量清了、没修**（关闭它要动生产语义，是用户可见 API 问题）。它既证明不了「取消让循环
    停下来」，自身又是这条用例现在的头号红因：改后整包 9/16 红在这里，消息
    `Some("step 8: 等待窗口内出现了意外 TX")`，而且**进门 0ms 就命中**。
    形状：取消命令的派发与 TX 事件的送达都要抢 CPU（实测派发最坏 3.3s；端口 worker 还会把相隔
    40ms 的三次写攒进同一毫秒发布），而这个重试循环总共只跑 120ms —— 负载下它经常在取消送达前
    就自己跑完了，19 轮探测里观测到 PING 条数 **4 / 5 / 6 都有**（空载恰好 4 条 = 第一次 3 条 +
    第二次被取消前的 1 条）。于是两头都坏：**假红** = 取消前已写出、还没被前面步骤消费的 PING
    留在共享 backlog 里，被这一步算成违规（`expect_event` 进门先扫 backlog，所以**加大窗口救不了
    可靠**）；**假绿** = 窗口里静悄悄、循环其实跑完了 —— 看得到条数的那 19 轮负载探测里一个实例都没
    抓到（多出的那几条总是同批涌进窗口、先把假红触发了），所以它是**结构性**暴露，不是已观测到的通过。
    **「整场恰好 4 条 PING」的断言没加**：观测日志确实拿得到（`report.cases[0].raw_packets` ←
    `observed` 过滤 RX/TX，`scenario.rs:144` 与 `packet_log`），但 4 条在负载下不成立，退成
    `<= 6` 又没有区分力（取消坏掉的变异恰好也是 6 条）。牙还在：把 fixture 的 `scenario.cancel`
    改成空操作，空载单跑立刻红在第 8 步 —— 可负载下同一变异 churn×4 整包 8/8、cpu×2 单跑 13/13
    全红，**消息与假红逐字相同**，所以这句话在负载下不带信息。三角不可能：可靠需要「写 → 被观测」
    的往返 < 循环周期，有牙需要缺席窗口 > 循环周期，合起来窗口就要 > 派发延迟（≈3.3s），而缺席
    检查每次**通过**都得白等那么久。关闭它的代价（择一，都要动生产/DSL）：`ExpectNoTx` 只统计
    **本步开始之后**发布的 TX（现在它把步前就躺在 backlog 里的旧事件也算违规），80ms 窗口即可
    既可靠又有牙；或给 DSL 一个「排空」步、让 `Cancel` 等到插件真处理完再返回。测试侧唯一自洽的
    形状是把 fixture 换成**永不自行结束**的循环（`while true` + 取消后写一条终止标记），把「停了」
    变成正向期望 —— 那是重写这条用例，不是改窗口。

13. ~~`recorder/src/replay.rs:1381`（`seek_performance_with_many_events`）把耗时钉成定长墙钟断言~~ ——
    **【已闭环】第 39 轮实测到（churn×4 整包改前 6/10、改后 3/10 红在它），后续轮已把整把尺子换掉**：
    现用例没有绝对预算，改为**同运行内的缩放比**（一份 20k 事件文件只 load 一次，靠 seek 到递增位置
    得三档 5k/10k/20k，相邻翻倍 ⇒ log2 比值即指数；两条独立估计量各管一头 —— `min` 比值指数 ≤ 1.75
    管牙齿、每轮最小比值指数 ≤ 1.50 管"绝不因负载假红"，四条防空转断言排在比值之前；
    数字依据与 A/B 实测在用例文档注释与 CHANGELOG 的 seek 条目里）。
    **换尺子换来的已知代价**：只放大常数、不改形状的回归在新尺子下不可见（实测 ≤1.08 全绿）——
    要守那种回归就得回到绝对预算，而绝对预算正是删掉的那条（负载下假红、平方级下没牙）。
    本条留档只为解释"为什么这里没有绝对墙钟"。

*(c) 有意节奏 / 已经是「真条件 + deadline + 大声失败」，别动*：
`application/tests/headless.rs:165`（就是 `tick_until` 本体的步长，本轮两条改用它是直接受益者）、
`:637`（回放推进的 deadline 轮询，预算用尽后仍有 `assert_eq!(published, 20)` 兜着）、
`:794`（5s deadline + `assert!(script_started, …)`）· `extension/src/manager.rs:954`（本轮新写的 2ms 步长，
等的是运行时收尾）· `extension/src/manager.rs` 的 `settle_until`（第 37 轮把 7 处固定 500ms 的 `settle`
收成的条件有界等待：2ms 步长、墙钟 deadline、判据逐字取自各自主断言，等不到带 `elapsed()` + 最后观测红掉；
形状照搬 `headless.rs:151` 的 `tick_until`；**第 39 轮给它加了一个 pump 闭包参数**
（`impl FnMut(&mut PluginManager)`，由调用点说明「推进这个条件的是哪个入口」：另 7 处 `process_pending`、
`natural_runtime_end_also_requests_cleanup` 那处 `reap_finished`）—— 变异探针两侧都做过：pump 传错
就是 30.00s 红并打印最后观测，不存在"反正都能过"）·
`recorder/src/recorder.rs` 的**七处收割等待**（按符号找：`while rec.reap_stopping().is_none()` 与
`start_stop_lifecycle` 里那个 `loop { if let Some(r) = rec.reap_stopping() … }`；**别按行号**，本文件
测试模块一年内漂了三次。原记的 `:791/:906/:1246` 三处带 5s、`:826/:856/:936/:1042` 四处**完全无上界**，
都挂在本桶里"别动"。第 39 轮读明白了：`reap_stopping()` 返回 `None` 有两条结构不同的路 ——
`let s = self.stopping.take()?`（压根没人可收割）与 `!join.is_finished()`（线程还没退），那四处
`while … is_none()` 把两条都读成"再转一圈"，所以**坏了是挂死而不是假红**；也没有契约保证那条 worker
必退（它走完 `stop` 要等一轮 `recv_timeout(100ms)`，之后还要 flush + 写 `.summary.json`，那是磁盘与
调度的性质）—— 照第 38 轮留的话办："加 deadline，不是缩短"。七处统一成同一形状：`budget = 30s` +
环内 `assert!(started.elapsed() < budget, … is_stopping={})`。**30s 是量出来的**：临时把上界抬到 60s
打点、跑整包记「`stop()` → 收割到手」的墙钟，churn×4 最坏 **1.435s**、churn×6 最坏 **3.337s**、
中位 100~150ms（就是 `recv_timeout(100ms)` 那道结构性下限）—— 原来那 5s 只剩 1.5 倍余量，
**一次通过的运行就能吃掉三分之二**，所以 5s 那三处也一并抬到 30s：抬高上界不改变任何通过路径上的
断言，只把挂死换成红。变异证词：把 `reap_stopping` 的判活改成恒假，`pause_resume`、`stop_is_idempotent`、
`start_stop_lifecycle` 各在 **30.0s** 红并带 `is_stopping=true`（前两处原本是永远挂、第三处原本 5s 红）。
同文件的 `wait_thread_finished`（原 `:979`，10s deadline + 环内 `assert!`）**没动**：它等的是一条除了
panic / 置位什么都不做的手工线程，与磁盘和调度无关）·
`transport/src/network.rs:323/357`（deadline 轮询 + `assert!(closed, …)`）· `:336`（Windows 释放端口的
**物理**宽限，不是赌线程）· `transport/src/lib.rs:1891` `wait_thread_finished`（注释已写明「时限只是兜底、
不是通过条件」—— 这一族的样板）· `lua_host/src/lib.rs:2206/2212`（5s deadline 循环，超时消息带已收日志）·
`updater/src/lib.rs:1400`（测试桩 HTTP 服务的 accept 轮询，带 deadline）。

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
5. **架构守卫改为「解析后的真实依赖名」** —— 两份守卫（`crates/app/tests/architecture.rs`、
   `crates/application/tests/architecture.rs`）过去都拿 `Cargo.toml` 做**文本**判定，
   评审在仓库外副本实证了两个合法清单即可绕过：
   ```toml
   my-transport = { package = "tool-transport", path = "../transport" }   # 声明名 ≠ 真实包名
   "tool-transport" = { path = "../transport" }                            # 加引号的键
   ```
   评审后补上**第三个同类向量**（提权声明在另一份文件里，只看成员清单同样漏检）：
   ```toml
   # 根 Cargo.toml
   [workspace.dependencies]
   sneaky-transport = { package = "tool-transport", path = "crates/transport" }
   # 成员 crates/panels/Cargo.toml
   sneaky-transport.workspace = true      # 成员侧完全不出现 tool-transport 字样
   ```
   > 本仓 `+1.92.0` 实测（不只是临时工作区）：该写法合法，`cargo tree -p tool-panels --locked`
   > 顶格出现 `tool-transport v1.2.0`，`Cargo.lock` 的 `tool-panels` 依赖表多出 `"tool-transport",`，
   > 而修复前两份守卫**都还是绿的**（`cargo test -p hardware-workbench-app --test architecture`
   > 与 `-p tool-application --test architecture` 均 exit 0）。
   > 注意 `[workspace.dependencies]` 里的 `path` 以**工作区根**为基准解析，
   > 所以这里必须写 `crates/transport`；照抄成员侧的 `../transport` 会让 cargo 直接报错。
   > 根表也**没有** `dev-dependencies` 这类别名表可藏：1.92.0 实测 `[workspace.dev-dependencies]`
   > 不参与继承（`workspace.dependencies was not defined`），继承只读 `[workspace.dependencies]`。
   现在判定统一到 `crates/app/tests/manifest_deps.rs`（后者用
   `#[path = "../../app/tests/manifest_deps.rs"]` 引同一份实现，不复制第二份）：
   * `dependency_names(manifest, workspace_renames)` 用 `toml` 解析清单，返回**解析后的真实 crate 名**，
     覆盖 `[dependencies]/[dev-dependencies]/[build-dependencies]` 与任意
     `[target.<cfg>.*dependencies]`，重命名以 `package` 字段为准；第二个参数是**根清单**
     `[workspace.dependencies]` 的「别名 -> 真实包名」表（`workspace_dependency_renames_from_root()`，
     根路径由 `env!("CARGO_MANIFEST_DIR")` 推两级，两个守卫 crate 各自都推得对），
     于是 `sneaky-transport.workspace = true` 也会解析成 `tool-transport`。
     被判定清单自己的 `[workspace.dependencies]` 与 `[package] name` 依旧不算依赖边。
   * `violations()` 做**精确集合比较**，因此 `egui` 不会吃掉 `egui_kittest`、
     `tool-application` 不会吃掉 `tool-application-extra`（子串匹配给的是恒真/恒假假证据）。
   * 判定器自身带 13 条元测试（含上述三个绕过向量、近亲反例、根别名表口径、
     根清单定位可达性，以及两条 `#[should_panic]` 防空转夹具：无 `[package] name`
     与零依赖清单都必须炸），先红后绿；它同时是独立 test target，故守卫编不过时元测试仍可单独跑
     （代价见上「计数口径」：这 13 条要占 39 个槽位）。
   * UI 禁令覆盖面从「`tool-application` 的 4 个可达内部 crate（实际 11 个）」改为
     **推导**的 `crates/*/Cargo.toml` 全集，按 `[package] name` 跳过两个 presentation crate；
     `tool-platform`（`application` 与 `panels` 的无条件共同基座）另有单条锁定断言，
     禁用语也补上了 `rfd` 与 `egui_kittest`。非 presentation crate 同时被禁止反向依赖
     `tool-panels` / `hardware-workbench-app`。
   * **根清单守卫**（`root_workspace_dependencies_do_not_rename_to_banned_crates`）：
     `[workspace.dependencies]` 不是依赖边、所有按清单扫描的守卫都看不见它，
     因而它是「改一行就能给 13 份清单投毒」的唯一位置；现在禁止其中任何
     `package = ` 指向 UI / 领域 / presentation crate。
   * **防空转**：扫描目录读不到 → panic；扫到的清单份数 < 15、检查份数 < 13、
     根 `workspace.members` < 16、根 `[workspace.dependencies]` 条目 < 15 → 当场红；
     单份清单解析出零依赖 → 红；根清单推导出错（读到的文件没有 `[workspace.dependencies]`）→ 红；
     被跳过的集合必须**恰好**等于 presentation crate 集合（多跳=静默放行，也会红）。
     `every_workspace_member_is_covered_by_the_scan` 进一步要求 `crates/` 之外的成员显式登记。
   核对（反向自检，必须真能红）：
   ```bash
   # ① 往 crates/panels/Cargo.toml 加 my-transport = { package = "tool-transport", path = "../transport" }
   cargo +1.92.0 test -p hardware-workbench-app --test architecture    # 红，点名 tool-transport
   # ② 往 crates/platform/Cargo.toml 加 egui = "0.35"
   cargo +1.92.0 test -p tool-application --test architecture           # 红，点名 tool-platform
   # ③ 根 [workspace.dependencies] 加 sneaky-transport = { package = "tool-transport", path = "crates/transport" }
   #    且 crates/panels/Cargo.toml 加 sneaky-transport.workspace = true
   cargo +1.92.0 test -p hardware-workbench-app --test architecture     # 红，点名 tool-transport
   cargo +1.92.0 test -p tool-application --test architecture           # 红，点名根清单投毒
   # ④ 只投毒根清单、没有任何成员继承
   cargo +1.92.0 test -p tool-application --test architecture           # 只有根守卫红（成员扫描仍绿：确实没有边）
   # ⑤ 还原本节所有 Cargo.toml 改动（Cargo.lock 会随 ③/④ 变动，必须一并还原）
   cargo +1.92.0 test --workspace --all-targets                          # 绿
   ```
   > 踩过的坑（已修）：按目录名跳过 presentation 写成
   > `path.display().to_string().contains("crates/app")` 时，Windows 分隔符是 `\`，
   > 该判定**恒假**，两个 UI crate 会混进检查集让守卫当场恒红；故改为按 `[package] name` 跳过。
   > 同理根 `Cargo.toml` 的 `members` 已补上 `crates/recorder`（此前只靠 path-dep 隐式纳入）。
   > **口径边界（不要过度解读）**：判定读的是每个成员**自己那份清单 + 根清单的别名表**，
   > 所以锁的是**直接依赖边**。三条边界要分清楚：
   > ① 传递闭包不在范围内 —— 「`tool-application` 的闭包里没有 UI」这条更强的断言仍归
   >    `cargo tree -p tool-application | grep -i egui`（见上「验证」段），测试里不许调 `cargo`，
   >    故闭包检查无法做成常驻门；`[patch.crates-io]` 换掉某个 crate 的**来源**同样看不见
   >    （它不改名字，只改内容），那是闭包问题而不是清单解析问题。
   > ② 重命名可以写在另一份文件里：`alias.workspace = true` + 根表 `alias = { package = "real" }`
   >    以前会少报，现在由别名表解析，且根表自身另有守卫（上一轮文档曾把这条写成
   >    「路径依赖不写 `package =` 就藏不住重命名」——那句只对③成立，对这条是沉默的）。
   > ③ 不写 `package =` 的路径依赖确实藏不住：1.92.0 实测根表写
   >    `sneaky-transport = { path = "crates/transport" }` 会 `no matching package named
   >    sneaky-transport` 直接构建失败，这个口子是 cargo 自己关的。

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
   - `tool-transport` 在 `crates/app` 的生产引用共 **2 处**（曾为 5 处：3 处 HEX 直连已随
     「HEX 判定下沉 `tool-core`」收敛，见下）：
     | 位置 | 用途 | 备注 |
     |---|---|---|
     | `app/src/commands.rs:312` | `tool_transport::natural_sort_key(&port.port_name)` | 端口自然排序 |
     | `app/src/app/mod.rs:15` | `use tool_transport::RepaintWaker;`（用于 `Arc<dyn RepaintWaker>`） | 重绘唤醒器 |
     | ~~`app/src/ui/bottom_panel.rs` `tool_transport::parse_hex` ×2~~ | 已改为 `Workbench::validate_hex` | 已收敛 |
     | ~~`app/src/ui/bottom_panel.rs` `tool_transport::hex_preview`~~ | 已改为 `tool_core::hex_preview` | 已收敛 |

     **所需 application 能力（下一轮候选，本轮不做）**：re-export `RepaintWaker`、暴露 `event_bus()`
     共享总线句柄、以及端口自然排序能力（`natural_sort_key` 或等价 DTO 排序）。
   - **HEX 判定的残留重复（实测，刻意未收敛）**：判定与解码的唯一真相已在 `tool_core::{parse_hex,
     parse_hex_strict}`（native 与 wasm 共用），但仍有 3 处各自实现：
     | 位置 | 为什么留 |
     |---|---|
     | `crates/panels/src/sender.rs:539` `hex_preview` | 与**曾经存在的** `tool_core::hex_preview` **渲染不同**：空输入返回 `""` 而非 `"—"`、无 `|ASCII|` 列、无 32 字节截断、报错带具体原因。二者当时谁都不该被悄悄替换 —— 那会改变界面输出。**第 11 轮已把 `tool_core::hex_preview` 连同它的三条用例删掉**（它当时在生产代码里已无调用点，"要删它就得先确认没有插件/文档依赖"这条前置已核实：全仓 `.rs` 里除 `sender.rs` 那份同名局部函数外零 token 命中，`plugins/` 与 `plugin-marketplace/` 是 11 个 Lua 文件、0 个 `.rs`，Lua 侧只能经宿主注册名触达 Rust，而没有任何宿主注册过它）。**`sender.rs` 这一份是活的，名字也一样，别照抄上面那句当成"它也没人用"**：按调用点核，`sender.rs:434` 调的就是本文件 `:539`。 |
     | `crates/panels/src/sender.rs:519` `hex_error` | 同上：它只判「能不能发」并渲染红色标签文案（`"HEX 中包含无效字符：{token}"`），文案本身是渲染结果。它是**校验器**而非解码器，不被 `grep parse_hex` 命中，容易被漏数。**测试覆盖是本轮补的**：此前该文件 `#[test]` 数为 0，把严格档的 `len() != 2` 放松成 `len() > 2` 全套 585 条仍全绿；现在 `sender.rs` 的 `mod tests` 钉着整张真值表（`gate_has_exactly_one_verdict_per_input_and_mode`、`strict_mode_rejects_every_length_except_two`）与那个分裂格（`gate_and_decoder_disagree_on_repeated_0x_prefix`），上述变异实测打红 2 条。 |
     | `crates/plugin_runtime/src/web_lua.rs:2508` `parse_hex` | wasm 插件 Lua API `ctx.serial.send_hex_to` 的后端，语义与 `tool_core::parse_hex` 不同（要求偶数长度、不认 `0x` 前缀），收敛即改动已发布文档（`docs/lua-plugin-api.md`）承诺的插件行为；且 `tool-plugin-runtime` 目前不依赖 `tool-core`，新增该边会改写 `Cargo.lock`（本轮约束为锁文件不变）。 |
     另：`crates/lua_host/src/codec.rs` 的 `from_hex` 是**有意**独立的 Lua 侧解析器，其归一化规则与
     `tool_core::parse_hex` 对齐，由该文件的 `#25` 用例用同一段输入双向比对钉住。
   - **周期发送已并入同一份发送判定（本轮闭合）**：native 的周期发送后台线程此前调
     `tool_transport::send_impl_to`，绕过 `send_plan`，于是同一段输入在「点一次发送」和
     「开着周期发送」之间有三种不同结果：多行 HEX 按行拆成多帧（点发送只投一帧）、
     纯空白文本被 `input.trim().is_empty()` 静默当成发送成功（web 会真的投出「文本+换行」）、
     非法 HEX 报 `translate_error` 的「无效HEX：…」而不是 `SendPlanError` 的「HEX 解析失败：…」。
     现在 `TransportEndpoint::send` 只做投递，命令由 `send_plan::send_command` 构造、字节由
     `send_plan::plan_send` 解码，`send_impl_to` 随最后一个调用点一起删除（留着它就是留第二份规则）。
     「输入框内容 → 命令」的构造点从 4 处（native 点发送 `bottom_panel.rs::do_send`、native 周期发送、
     wasm 点发送 `web.rs::send_web_current`、wasm 周期发送）收成 1 处；第 5 处
     `crates/panels/src/sender.rs:346-356` 仍是独立构造（panels 不得依赖 `tool-application`，
     见本文件依赖边界守卫），它产的是 `SendAction` 而非 `AppCommand`，两处构造由同一条
     「文本追加后缀、HEX 不追加」规则约束，该规则本身在 `send_plan::send_command` 的单测里钉死。
     守卫：`crates/application/src/send_plan.rs::send_command_is_the_one_payload_rule`（规则层）
     与 `crates/application/tests/headless.rs::periodic_send_endpoint_uses_the_shared_send_plan`
     （真实 loopback 端口上的**帧边界与错误文案**：多行 HEX 恰好一帧四字节、空白文本仍投递、
     非法 HEX 一帧都不上线）。变异核对：把投递改回 `bytes.chunks(2)` 逐块发送，后者立刻红
     （收到 3 条 script 而不是 2 条）。
   - **发送路径的残留差异（实测，刻意未收敛）**：`send_plan::plan_send` 收敛的是**判定**，
     不是判定的**输入**。任务种类现在只有一个决策点，但它的唯一路由输入 `is_network: bool`
     仍由各平台自己算：
     | 差异 | native | web | 为什么留 / 后果 |
     |---|---|---|---|
     | `is_network` 的**计算规则** | `Workbench::is_network_port` 比对 `app_config.network_ports` 的字符串；本轮起同时认 `display_name()`（`host:port`）与 `port_id()`（`network://host:port`）两种形态 | `WebApplication::is_network_port` 查 `network_ports` 的键，键只在 `port_id()` 形态下写入（`crates/application/src/web.rs` 的 `RegisterNetworkPort`） | 两种形态都**被持久化**（native 的 `workspace.json` 存 `network_ports`、web 存浏览器设置），统一命名会打断已存工作区，故**不统一命名**。跨平台误分类是**响亮**的：native 形态的 id 到了 web 会走到 `WebSerialTransport::enqueue_command` → `TransportError::PortNotConnected`（`crates/platform/src/web_serial.rs:504`），字节发不出去而不是发往错误设备。native 此前**内部不自洽**（`RemoveNetworkPort` 认两种形态、`is_network_port` 只认一种），本轮闭合，由 `crates/application/tests/headless.rs::network_port_ids_are_recognized_in_both_naming_forms` 双向钉住（两形态→`send_network`，普通串口名→`send_serial`）。 |
     | HEX **预检的规则来源**（round-1 把这一行记成「预检档位」的两端分歧，**那个分歧不成立** —— 依据的站点是死代码，见本行末与下方更正） | **活着的**按钮门禁是共享面板 `crates/panels/src/sender.rs:324-330`：它传 `*view.hex_strict`（默认 `true`，`crates/app/src/state.rs:301`），「严格」勾选框也是活的（`sender.rs:208`）。本平台自己的 `Workbench::validate_hex` **在本平台连调用点都不存在** —— 原有的两个（曾传 `strict = false`）位于 `legacy_send_panel_body` 的死子树内，已随该子树一起删除（见本文件第 8 条） | 按钮门禁是**同一个** `sender.rs:325-330`（`crates/app/src/web/mod.rs` 的 `shared_sender_ui`（文件头 `use ... sender_ui as shared_sender_ui` 起此别名，均按符号定位）也调它）；`web_hex_error` → `WebApplication::validate_hex` → `tool_core` 只出现在 `send_web_current`（定义 `crates/app/src/web/mod.rs:1829`，预检站点 `:1847`）这一条路上，而它的**可达手势当时只有命令面板、不是键位**：web 的键位处理器当时在 `:3117-3119` 就把 `CMD_SEND` 拦下直接 `return`（注释称"发送键位由面板自己处理"，可 `crates/panels` 内 `Key::Enter` 的发送处理是 **0 处**，面板提示却写着「Ctrl+Enter 发送」（`sender.rs:248`）—— 这条 web 键位疑似死绑定先于本分支，未在本轮改动），于是 `execute_web_command`（def `:3123`）的 `CMD_SEND` 分支（`:3125`）只由命令面板的确认路径到达 **【本轮更正并已修复】**：那个早退已删，`$Send` 的默认键位（`shared_keymap.rs` 的 `(CMD_SEND, "Enter", ctrl)`，两平台同一张默认表）在 web 与 native 一样可达，两侧都由 `send_web_current` / `do_send` 消费。 | 「`"abc"` 在 native 判可发（按钮亮着）、在 web 判不可发」这句 round-1 的结论是**错的**：两端按钮都由 panels 那份校验器按 `hex_strict` 判，默认严格档下 `"abc"` 在**两端都被拒**（`sender.rs:529`：规范化长度 3 ≠ 2）。真正的残留是**规则来源**、不是档位：活的门禁用 `crates/panels/src/sender.rs:519` 那第三份 `hex_error`（只剥**一层** `0x`），而发送与 web 那条面板外发送路（`send_web_current`）的预检用 `tool_core`（`normalize_hex_token` 的 `trim_start_matches` 反复剥），于是默认严格档下 `"0x0xAB"` 一边判非法（规范化后 `0xAB`，长度 4 ≠ 2）、一边判合法（`AB`）：**两端**都会把这串的发送按钮灰掉，而 `dispatch` 其实接受它 —— 且这条路**用户能走到**：native 的 Ctrl+Enter 走 `crates/app/src/commands.rs:210` 的 `cmd_send_if_ready`（只查「端口已打开 + 输入非空」，**不做 HEX 预检**）→ `do_send` → `dispatch`，web 走 `send_web_current`（命令面板确认或 Ctrl+Enter，预检用 `tool_core`，同样放行），两边都会把这串真发出去。行为保持的收敛不动它（改了会改变渲染出的 UI）。**本波已把这个分裂两侧的实际行为都变成机器断言**：门禁侧 `crates/panels/src/sender.rs::gate_and_decoder_disagree_on_repeated_0x_prefix`（连同该文件新增的真值表），解码侧 `crates/application/src/send_plan.rs::strict_and_lenient_have_exactly_one_verdict_each` 的 `"0x0xAB"` 一格 —— 今后任何人统一它，会先看到红测试，而不是只看到这张表。**连带事实（按可达性归因）**：`Workbench::validate_hex` 在 native 已经**没有任何调用者**，它的 `strict` 形参既没有 `true` 的调用者也没有测试 —— 原先那两个调用点整体不可达，现已随死子树删除而彻底不存在。 |

     > **错误文案（对先前记录的更正）**：Task 6 **改掉了** native 的非法 HEX 文案，此前写作
     > 「两平台错误文案逐字未变」是错的。本轮之前 native 的 dispatch 错误是
     > `AppError::Transport(TransportError::InvalidHex(..).to_string())`，渲染为
     > `transport: invalid hex input: <原因>`。任务内两步各有其责：Part A（`25d2c38`）把
     > `tool_transport::parse_hex*` 换成 `tool_core::*` 并用 `format!("HEX 解析失败：{原因}")`
     > 重新包装（`transport: ` 前缀之外的中文串就是那时换的），Part B（`74719a6`）又把这句话
     > 搬进 `SendPlanError: Display`。那时渲染为 `transport: HEX 解析失败：<原因>`
     > （`transport: ` 前缀来自 `AppError`）。判定不变，变的是披露与措辞。
     > **本轮发布的证据原文**（native 输入 `"AB C"` + `strict: true` 经 `dispatch` 的完整渲染，
     > 第 24 轮起如下）：
     > `传输错误：HEX 解析失败：严格模式: "C" 规范化后为 1 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式`
     > **这句有测试钉着**：`crates/application/tests/headless.rs::
     > send_routing_dispatches_by_port_kind_and_rejects_invalid_hex` 除断言变体外，还断言
     > 该输入的 `strict_error.to_string()` 逐字等于上面这整行 —— 改 `SendPlanError` 的
     > `Display`、改 `AppError::Transport` 的 `#[error("传输错误：{0}")]` 前缀、或改
     > `tool_core` 严格模式的文案，都会让该用例变红（两条变异均实测过，见 task-6 报告 M7/M7b；
     > 第 24 轮换掉前缀时该用例实测变红一次）。
     > **【第 24 轮更正】**上面那句里的分类前缀已从英文 `transport: ` 换成中文 `传输错误：`：
     > `application/src/error.rs` 七个 `#[error(...)]` 全部换成中文分类标签（`录制错误：`
     > `回放错误：` `存储错误：` `插件错误：` `当前状态不允许：` `命令不支持：`），因为它们是
     > 夹在 UI 那句中文框架与已中文化正文之间的最后一段英文。**换语言不等于闭合两端差异**：
     > `AppError` 被 `cfg(not(target_arch = "wasm32"))` 挡在 wasm 之外，web 侧仍然 `to_string()`
     > 出不带任何分类前缀的那句，"native 有一层分类框架、web 没有" 这条按设计仍在。
     > **本节原有的两处 hover 文案（`hex_precheck_hint`）已随死子树一起删除**：它当时
     > 仅有的两个应用点就在 `legacy_send_panel_body` 内（详见本文件第 8 条），所以 round-1
     > 记录的「用户在看板里读到带前缀的句子」那半条 UX 回归**从未到达过用户**，如今连
     > 载体都不存在了。留下的教训仍然成立：一个改动若只落在不可达站点上，四门全绿
     > 也证明不了它被用户看见过 —— 判可达性要用调用点形式核对，别用裸符号计数。
     > **仍然活着的**用户可见前缀站点只剩**红字标签**（不是状态栏）：`bottom_panel.rs:175`
     > 把 `dispatch` 的错误 `.to_string()` 写进 `self.send.error`（另两处 `:186`、`:191` 是
     > 本文件自己造的定长文案，不含 `AppError` 前缀），它经 `SendView.error` 由
     > `crates/panels/src/sender.rs:155` 的 `ui.colored_label(theme::red(), error)` 渲染，
     > 按设计**仍带 `传输错误：` 分类前缀**（`#[error("传输错误：{0}")]`，第 24 轮之前是 `transport: `）。
     > **状态栏与 HEX 无关**（round-2 把两者并列是错的）：活着的状态栏错误站点全是
     > DTR/RTS —— `sender.rs:417`、`:425` 产出 `SendAction::SetDtr` / `SetRts`，由
     > `bottom_panel.rs:140`、`:152` 消费后写状态栏，没有一条来自 HEX。本条目里原先记的
     > 另外两处（`send_signal_controls` 的 DTR/RTS 复选框）也在死子树里，已一并删除。
   - **下一轮候选（把漂移的门真正关上）：`plan_send` 收类型化目标，不收裸 `bool`** ——
     改签名为 `plan_send(cmd, SendTarget::{Serial, Network})`，由两平台**已经持有**的端口种类
     解析：`tool_platform::PortDescriptor::kind`（`crates/platform/src/lib.rs:112`，网络端口在
     `descriptor()` 里就带 `PortKind::Network`）与 native 侧 `SerialPortDescriptor.port_type`
     （注册于 `crates/application/src/workbench.rs:760`）。理由：`is_network: bool` 只能表达
     「不是网络就是串口」，于是「端口根本不存在」被折叠成「串口」；有了第三种取值才能把 unknown
     当场拒绝，也才让上面那张表的第一行不再靠命名约定对齐。
   - **wasm 侧发送路径的测试缺口（实测尺寸）**：Task 5 的字节契约是发送路径上唯一被**执行**的
     **端到端投递**守卫，而它只守 native 半区 —— `headless.rs` 按构造是 native-only（文件顶部直接
     `use tool_application::{Workbench, AppError, ..}`），`crates/app/src/web/`（拆分后的 `mod.rs` 与子模块）无 `mod tests`，
     `crates/application/src/web.rs` 亦无。**注意别把它读成「发送路径上只有这一条守卫」**：
     规则层另有 `send_plan.rs` 的 **6 条**单测在 native 测试目标里执行（M1 那次的零字节变异下
     红了其中 3 条），本轮又给 `headless.rs` 的非法 HEX 用例加了渲染句串断言 —— 被执行的守卫
     不止一条，只是**端到端投递**这一层仅一条。带测试的 `src` 文件也**不止三个**：
     `send_plan.rs` / `task.rs` / `transport.rs` / **`service/terminal_store.rs`**，核对：
     ```bash
     grep -rn '#\[test\]' crates/application/src/ | sed 's/:[0-9]*:.*//' | sort -u
     ```
     所以缺口的准确尺寸是：**`plan_send` 那个纯函数之下整个 wasm
     侧 —— application 与 presentation 两层都没有行为测试**。具体仍可全绿出厂：`is_network` 被
     硬编码（已实测）、`WebApplication::send()` 忽略 `plan.bytes`、两个 wasm `spawn` 分支里的
     `task_kind` 字面量被重新硬编码、`WebApplication::validate_hex` 的错误映射被翻转、
     以及整层 wasm 呈现 —— 含 `crates/app/src/web/mod.rs` 的 `web_hex_error`（按符号定位，行号每轮拆分都会漂）→
     `runtime.validate_hex`（Part A 正是在这里删掉手抄副本；未来一次编辑若丢掉 `serial.hex_strict`
     或不理会 runtime，没有任何测试会响）。成立的推论：判定逻辑已不再住在 `web.rs` 里，
     所以缺口的**内容**是投递与呈现，不再是规则本身 —— 但每往 `web.rs` 加一条判定，就是往这个
     零覆盖区里加一条。
   - **更新清单的 pinned 摘要只约束 native —— wasm 读同一份 `update.json` 却完全不看它**
     （实测，本轮**只登记不修**）：
     | | native | wasm |
     |---|---|---|
     | 清单 DTO | `tool_updater::update_info::UpdateInfo`（`crates/updater/src/update_info.rs:9-22`），`sha256` 是**必填**字段（无 `#[serde(default)]`），再过一道 `is_pinned_sha256`（同文件 `:25-27`）形状门 | `WebUpdateInfo`（`crates/application/src/web.rs:158-165`）只有 `version`/`date`/`download_url`/`changelog`，**没有 hash 字段** |
     | 结果 | 清单缺 `sha256` ⇒ 解析即失败 ⇒ 「检查更新」报错，绝不放行一次无法校验的更新 | `:868` 把它填进 `updater::UpdateInfoView`（`crates/application/src/updater.rs:4-9`，同样没有 hash 字段），`crates/app/src/web/mod.rs` 里两处把 `download_url` 直接交给 `open_web_url` 的调用点（按符号定位：搜 `open_web_url(&info.download_url)`，本树实测 2 处；此前写的 `web.rs:3738`/`:4396` 与 `web/mod.rs:2833`/`:3146` 都是拆分前的旧行号） |

     即：**同一份清单**在 native 被拒、在浏览器里却被当作"有可用更新"提示出来。本轮判为记录而非修改，
     四条理由（都不是"没时间"）：(a) wasm 侧**无从校验** —— 字节由浏览器在 GitHub 上取，本程序
     从不持有下载流，加一个必填 hash 字段的效果只是"把更新卡片藏掉"，不是把校验补上；
     (b) **当前已发布的 `update.json` 根本没有 `sha256` 字段**（仓库里那份即 v1.2.0 的产物），
     所以加上即把 wasm 从"显示 1.2.0"翻成"解析更新信息失败"，是一次用户可见的行为变更，
     该和 native 那条退化一起由发布说明承担，而不是混进修波；(c) 想让形状规则不重复实现就得从
     `crates/application` 调 `tool_updater::update_info::is_pinned_sha256`，而 `crates/application`
     **不依赖 `tool-updater`**（`crates/application/Cargo.toml` 实测：native-only 边只有
     extension/marketplace/lua_host/transport），新增该边既改写 `Cargo.lock`（本轮约束不变）、
     又把 `d58af5c` 刚收回去的"presentation/application 不直连更新栈"边界重新捅穿；
     (d) 该模块是 `crates/application/src/lib.rs:13-14` 的 `#[cfg(target_arch = "wasm32")] pub mod web`，
     宿主测试**编译不到**它（见上一条的覆盖缺口），改动会以"无测试的行为变更"出厂。
     后续若做，正确形状是：先发布一份带 `sha256` 的 `update.json`，再给 `WebUpdateInfo` 加必填字段，
     并在发布说明里写明 wasm 侧「检查更新」自此也会因缺字段而失败。
   - `tool-databus`：`app/mod.rs` 的 `DataBus::new()`、`bus.publish(..)` 与四个面板的 `::new(&bus)`；
     `web.rs`、`settings_panel.rs`、`perf.rs`、`web_perf.rs` 另有引用。
     所需 application 能力：一个返回共享总线句柄的 accessor（供 presentation 订阅），
     否则 app 无法构造面板订阅。
7. **`crates/panels` 内部仍自持 `DataBus` 订阅**（`Terminal/Log/Chart/Attitude/Gauge/Dynamic/data_table`
   各自 `subscribe_*`）。这属更大的 Presentation 数据流重构，本轮不动。
8. **`crates/app` 的死代码子树：已删除（本条原为「实测，未删」）**：
   `crates/app/src/ui/bottom_panel.rs` 的 `legacy_send_panel_body`（曾带 `#[allow(dead_code)]`、
   全工作区 0 调用）与被它拖成不可达的整族渲染函数（实测闭包 **14 个方法 + 2 个自由函数**；
   早期写"三个"、终审建议写"四个"，都低估），已随 `fix/panel-layout-and-dock-drag`
   （提交 `9b5e230`，自 `D:\game\serial_tool` 合入）**实际删除**：本文件 1467 → 242 行，
   发送器界面改由两平台共享的 `sender_ui` 渲染，并补了针对它的布局测试。
   `hex_precheck_hint`、`response_id_for_send_input`、旧历史弹窗、重复的本地 `SendLayout`
   枚举，以及 native 侧那两个 `Workbench::validate_hex` 调用点一并消失。
   （他们分支上的 CHANGELOG 记的是 1429 → 257：那是相对**他们**基线的数，本树以 1467 → 242 为准。）
   **连带事实（第 11 轮更新）**：native 的 `Workbench::validate_hex` 已删（它当时**一个调用点
   都没有**，`strict` 形参既没有 `true` 的调用者也没有测试）；现在全仓只剩 wasm 那份
   `WebApplication::validate_hex`，唯一的 `.validate_hex(` 调用点是 `crates/app/src/web/mod.rs`
   的 `web_hex_error` 内部那一处（按符号定位），而 `WebRuntime` 只是
   `WebApplication` 的别名（`crates/application/src/web.rs:37`），故它走 wasm 那份实现。
   **本条留下的方法论（这才是它值钱的部分）**：可达性只能用**调用点形式**核对，
   ```bash
   grep -rn '\.validate_hex(' crates/          # 恰好 1 行：web_hex_error 里那次调用
   grep -rn 'fn validate_hex' crates/          # 恰好 1 行：wasm 那份定义（native 的已删）
   grep -rn 'legacy_send_panel_body' crates/   # 0 命中：符号随代码一起没了
   ```
   裸符号计数**不是证据**：它随"提到这个符号的次数"增长，改一次注释就变一次 ——
   本条此前的版本曾把自己写的计数当成实测值。
9. **`crates/panels` 那组已被 application DTO 取代、但仍导出的死符号：第 11 轮已删**：
   原记的站点实测确认后才动手 —— `plugin_view.rs` **整文件删掉**（`InstalledPluginRow` /
   `PluginViewState` / 该文件的 `PluginUiCommand`，48 行），连同 `lib.rs` 的 `mod plugin_view;`
   与那条 `pub use`；`replay_view.rs` 只删 `ReplayView` 与它的 `Default`、`From<&ReplayStatusView>`
   两个 impl（106 → 26 行），**同文件的 `ReplayUiCommand` 是活的**（native `runtime/replay.rs`
   与 wasm `web/replay.rs` 都在消费），于是文件名 `replay_view.rs` 现在只剩一个命令枚举 ——
   刻意不顺手改名，改名是另一次搬运的账。
   > 为什么这类死符号是**这一轮才被发现**的：`dead_code` 看不见 `pub` 项，而本仓的死代码扫描按
   > `pub fn` 找，`pub struct` 根本不在它的形态里。这两条盲区叠在一起 = 四个符号在文档里记了
   > 一轮"实测未删"。扫描脚本要同时覆盖 `pub fn` / `pub struct` / `pub enum`。
   >
   > **第 23 轮又发现同一族的第三条盲区，而且更隐蔽**：那条扫描的正则是 `^pub (fn|struct|…)`，
   > **锚在零列** —— 于是它扫过的 481 个"pub 项"全是顶层条目，**`impl` 块里的方法一个都没看过**。
   > 触发点是 `LuaHost::run_script`（当时全仓只有它自己那一行，`dead_code` 因为它是 `pub` 而沉默）。
   > **【第 34 轮：这颗还了 —— 整座 `LuaHost` 死岛已删除，见本条末尾】**触发它的符号没了，
   > 但这条盲区结论**一个字都不作废**：漏看 `impl` 块是扫描器的性质，与某个符号还在不在无关。
   > 补的 `impl` 方法扫描（`target/` 下，不入库）在 731 个固有 `pub` 方法里查出候选，并且它自己
   > 一开始还带第二个盲区：**测试里的引用也算"用过"**，所以"只被自己那条单测调用"的方法会隐身
   > （`PanelManager::sync_tabs_from_dock` 就是这样混过第一遍的）。现在按
   > **生产引用 / 仅测试引用** 分桶：`DEAD 8` + `TEST_ONLY 25`。
   > **两个桶不能混为一谈**：`TEST_ONLY` 不等于可删 —— `TerminalExportJob::render_chunk` 与
   > `LogExportJob::render_chunk` 存在的意义就是被"分块导出 == 整块导出"那条等价用例调用，
   > 删掉是拆自己的测试能力。**【第 35 轮：这句话护的是那条等价用例，不是整座岛 —— 先把差分
   > 期望换成从活路径捕获、并用变异探针验过红/绿效力的字面量表，等价能力不再依赖快照 job，
   > 两份同形的快照导出域（terminal/log）连同这两条 `render_chunk` 一起删除。"岛内边看起来
   > 是生产"的盲区结论原样保留：判据是可达性，不是引用计数】**
   > 反过来 `DEAD 8` 那批里也有刻意留着的（`NotificationQueue::dismiss`
   > 带 `#[allow(dead_code)]` 与一条写明"Toast 得先带上 source"的理由 —— 那是暂停不是遗漏）。
   >
   > **第 23 轮删掉的（`tool-panels` 一个 crate，净 −95 行）**：
   > ① 终端书签那条**整支死支路** —— `TerminalPanel::bookmarked_entry_ids` 字段加它的 5 个操作点
   > 与两个访问器（`is_bookmarked`/`toggle_bookmark`）。可证零行为变化：唯一的写入者是
   > `toggle_bookmark`，而它零调用者，所以这个集合在每一个 `TerminalPanel` 的整个生命周期里
   > **恒为空**；`TerminalPanel` 没有任何 `#[derive]`（不是序列化形状）；该文件测试数 33 → 33
   > （这支特性本来就没被测过，与"从没接上过"互相印证）。
   > `crates/panels/src/replay.rs` 那套**活着的**书签（`bookmark_controls` + `status.bookmarks`）
   > 是另一个概念，没动。② `DockLayout::move_panel` + 随之变孤儿的 `stack_mut`/`reveal_area`
   > （`remove_from_all_areas` 仍被 `close_tab` 用着，留着并把已经变成假话的"跨区移动与关闭都必须
   > 先做这一步"改成只说关闭）。③ `DockStack::reorder`。④ `PanelManager::sync_tabs_from_dock`
   > 连它那条只测它自己的单测（它的存在理由就是"在 `dock.move_panel()` 之后同步"，而那个调用者
   > 本轮没了；删代码时把它留下的假 doc 一起删，别留个谎）。
   > **两个只记不删的疑问（下一轮的取证项）**：
   > (a) `PanelManager::select_center_panel` doc 自称是"左侧栏点击"的处理器，实际生产零调用 ——
   > 组合根 `crates/app/src/ui/tiles.rs:12` 直接写 `self.panels.active_tab = id.clone()` 绕过了它
   > （`app` 侧也完全没人调 `open_tab`）。于是"该收敛成字段还是该走 setter"是个方向问题，
   > 不是机械删除。**【第 35 轮：这颗还了，当时记的理由当场证伪】**那两处直写
   > （`ui/tiles.rs::tile_panel_body` 与 `web/mod.rs::render_panel`）发生在**绘制一块 tile 的
   > 过程中**，改走 `select_center_panel` 等于在布局途中执行 `ensure_tiles_layout().select_pane()`
   > —— 那是另一种操作，不是绕过；括号里"`app` 侧也完全没人调 `open_tab`"同样过期
   > （`runtime/plugin.rs` 与 `dynamic/ingest.rs` 都在调，`open_tab`/`open_plugin_tab`/
   > `close_tab` 三兄弟全活，本轮没动）。正确的记录是"**只测自己**的零生产调用 API"：
   > 方法与它唯一的单测一并删除，活的面板选择走 `TilesLayout::select_pane` 一族 handler。
   > (b) `Workbench::query_terminal_since` 有 **8 处测试引用、0 处生产引用** ——
   > 终端数据的活读路径是别的那一条（本轮没查是哪条），先记着。
   > **【第 46 轮：这颗还了，而且它比"零使用者"更糟 —— 整座 `TerminalService` 是 native-only 的
   > 第二条终端摄入管道】**活读路径查明了：`crates/panels/src/terminal.rs` 的 `TerminalPanel`
   > 自订阅 `transport.serial.*` 并持有自己那份 `TerminalStore`（native 每帧在
   > `crates/app/src/runtime/plugin.rs`、web 在 `crates/app/src/web/mod.rs` 调 `ingest_pending()`），
   > 终端数据从不跨 application 边界。本轮复核：引用是 **9 处调用 + 2 条用例，全部在
   > `crates/application/tests/headless.rs`**（当时记的 8 处少算了同一条 assert 里的第 9 个调用）。
   > 删的符号：`TerminalService`、`TerminalDelta`、`TerminalSeq`、`TerminalEntry`、
   > `Workbench::query_terminal_since`、`DEFAULT_MAX_ENTRIES`、整个 `model` 模块（它只装这份 DTO），
   > 以及 `tool-application` 的 `parking_lot` 依赖边（`git grep HEAD` 证实全 crate 唯一使用者就是
   > 被删那把 `RwLock<TerminalStore>`）；用例 `terminal_delta_is_incremental` 与
   > `terminal_bound_clamps_max_entries_and_pages_incrementally` 一并删除。**另两件记账级事实**：
   > ① native 的 `AppCommand::ClearTerminal`（命令面板/快捷键「清空终端」）原先只清那份没人读的
   > store，删除后该分支只剩 `Ok(Done)` —— 桌面端这条命令历史上就没有可见效果（wasm 侧靠
   > `notify(TerminalCleared)` 让组合根清面板），补不补这条回路是产品决定，代码里就地写了；
   > ② `crates/application/src/service/device.rs` 是一份**没被任何 `mod` 声明**的遗留文件
   > （`DeviceService` 全仓 0 引用，连编译都不参与），按"一轮一个域"本轮没动它。
   > **【2026-10 清理批：已删除】**该文件与 `TerminalItem::last_timestamp_ms()`（零调用）、
   > `ReplayManager::load`（仅测试调用；测试改走生产两步 `prepare_load` + `load_prepared`）
   > 一并删除。
   > 本轮**没**碰其余候选（`recorder` 的 backpressure 两件套、`core::advance`、`tool-testing` 的
   > 两个计数、`replay::publish_next_for_test`、`lua_host::run_script`），它们分属别的 crate，
   > 按"一轮一个域"排到后面。**【`lua_host::run_script` 那一颗由第 34 轮还掉，见本条末尾】**
   > **【第 26 轮把这批还了四件】**（710 → 710，**一条用例都没动过就是"确实没人引用"的最强证据**）：
   > 删 `JsonlRecorder::set_backpressure_config` + `backpressure_config`（**字段本身是活的**，
   > `tick_backpressure` 每帧读它判定软/硬背压，只有写侧没人做 —— 于是阈值实际固定在 `Default`；
   > 删之前 setter 是"可调用但无人调用"，删之后这条能力**不再可表达**，是否要开放给用户调是产品问题，
   > 已在类型上补一条注释免得下一个人当成漏接线重新加回）、`ReplayManager::publish_next_for_test`
   > （名字里的 `for_test` 是虚构的：全仓没有任何测试调它）、`FrozenClock` 的 `new`/`advance`/`set`
   > 三个构造与推进方法（本来就在 `#[cfg(test)] impl` 里），连带 `RecorderBackpressureConfig`
   > 那条**死的 re-export**（`tool-recorder` 之外 0 处提到这个类型名；类型仍在 `pub mod recorder` 里）。
   > **顺带查出扫描器的第 4 条盲区，值得记**：`DEAD 8` 是**少数**了 —— 判"用过"靠词边界匹配，
   > 而 `set` / `new` 这类动词名会撞上全仓无关代码（`new` 还刻意被跳过）。这条不是"再修修脚本"
   > 能解决的：动词名的死活**必须**结合接收者看，不能只数出现次数。
   > **两条新的后续结论 → 第 27 轮都收了**：① `FrozenClock`（删掉 `new` 后已无法构造）与
   > ② `tool_core::Clock` —— 后者在 `crates/core` 之外 **0 处引用**，且**没有任何多态使用者**
   > （全仓无 `dyn Clock`、无 `<C: Clock>`、无 `: Clock` 约束、没有任何函数接收过时钟），
   > 所以它文档里那句"时间源抽象，使依赖时间戳的代码可测试"**从未成立过**：没有注入点，
   > 谈不上接缝，`FrozenClock` 也从来没有任何用户。第 27 轮把 `Clock` / `SystemClock` /
   > `FrozenClock`（含两个 `impl`）整组删掉，`SystemClock::now_ms()` 的函数体**逐字搬进**
   > `now_timestamp_ms()`（两个 cfg 分支、wasm 那句"没有 OS 支撑的 SystemTime，改用浏览器挂钟"的
   > 英文理由注释、`.min(u64::MAX)` 钳制、以及 `UNIX_EPOCH` 之前 `log::warn!` 后**返回 `1`**
   > 的兜底，一个字没改；签名不动，60 来处调用点无感）。搬完后用"去掉空白后逐行比对"证明
   > 消失的实现行只剩脚手架（trait/struct/impl 头、`fn now_ms`、`#[cfg(test)]`、`Arc<AtomicU64>` 字段）。
   > **那条兜底臂仍然跑不到**（删之前也跑不到 —— 没有注入点就拨不动系统时钟），所以给它加了一条
   > **形状**守卫 `clock_before_epoch_fallback_is_pinned_to_a_nonzero_literal`：文档向调用方承诺
   > "时间戳非零"，而唯一能机器检查的办法就是钉实现文本。**它的限度写在注释里**：钉的是文本不是
   > 运行时行为，语义等价的改写（比如抽个常量）也会红，那时该连同文档一起重新审。
   > 一个真实的坑顺带记下：**本仓源文件里有 CRLF**，文本类守卫若按 `"}"` 判函数结束就会切片越界、
   > 把后文当成函数体（本条第一版就是这么悄悄红了又"看起来像变异生效"），**先归一化换行再切**。
   > **【第 34 轮：还掉 `lua_host` 那颗 —— 整座 `LuaHost` 死岛删除】**
   > `crates/lua_host/src/lib.rs` 删 `pub struct LuaHost` + 它的 `impl`（`new`/`run_script`/`stop`/
   > `state`/`reap_finished` 五个方法）+ `impl Drop for LuaHost` + 只有它一个用户的 `struct LuaWorker`，
   > 净 **−143 行**（2792 → 2649），**四道门 725 → 725 passed / 0 failed / 7 ignored**（一条用例都没丢，
   > 与第 26 轮同一式的最强证据）。判据是同形 grep，不是"没搜到"：
   > ```bash
   > grep -rnw 'LuaHost'  crates/   # 删前恰好 3 行：结构体 / impl / impl Drop，全在同一文件
   > grep -rnw 'LuaWorker' crates/  # 恰好 3 行：定义 + LuaHost 的字段与构造点
   > ls crates/lua_host/            # 无 tests/ 目录：本 crate 的单测只有 lib.rs 的 #[cfg(test)] mod
   > grep -rn 'LuaHost' plugins/ --include=*.lua   # 0 命中：插件侧不知道这个名字
   > ```
   > **`LuaHost::` 的调用点 repo-wide 为 0**（连它自己那条单测都没有）。活路径不在这座岛上：
   > 插件走 `run_plugin` / `LuaPluginRuntime`（`crates/extension/src/manager.rs:11`），测试走自由函数
   > `run_script_blocking` / `run_script_for_test` —— 三者都不是本轮的对象，全部保留。
   > 岛外唯一受影响的下游是 `LuaHostError::AlreadyRunning`：它随之**失去最后一个生产者**，
   > 是否连变体带 `ExtensionError::Lua` 一起收掉是**待用户拍板**的决定（判据是"`run_plugin`
   > 还要不要返回 `Result`"，不是死代码本身），本轮两条臂都没动。
   > 剩下的 DEAD 只有 `testing::failed_count` / `save_latest_json`
   > （这两个在**专门给测试用的 crate** 里，"仅测试引用"在那儿是设计而非死），
   > 以及刻意保留的 `app/state::dismiss`。
   > 另：删掉 `PermissionManager::missing_capabilities` 之后，它唯一的消费者
   > `PluginManifest::required_capabilities` 也随之删除，于是 **`tool-extension` 现在有一条
   > 可证明的死 Cargo 边 `tool-plugin-api`**（全 crate 0 处 `use`）。它和第 10 条
   > `egui_extras` 是同一类决定（要动 `Cargo.lock`），等用户拍板，本轮不动。
   > **【2026-10 清理批：两条死边一起删除】**`tool-extension/Cargo.toml` 的 `tool-plugin-api`
   > 边与 `egui_extras`（workspace 声明 + `crates/app` 依赖边）均已移除；`Cargo.lock` 重解析后
   > 净 −36 行（`egui_extras` 及两个仅供它使用的传递包 `enum-map`/`enum-map-derive` 消失，
   > 另有本条死边的一行）。
   注意 `PluginUiCommand` 存在**同名不同类型**（这条对删完的现状仍然成立）：
   `plugin_api::host::PluginUiCommand` 是活着的那组（在 `web_plugin_host.rs`、`mlua_engine.rs`、
   `web_lua.rs` 中大量使用）。核对：
   ```bash
   grep -rn 'tool_panels::PluginUiCommand' crates/   # 0 行（panels 那份已随文件删除）
   ```
   > 早期文档曾称这些文件为"已占位"（即待用），实测是"已被取代、可直接删除"，属相反结论。
   > **【第 35 轮：删掉被游标路径取代的整座快照导出域 —— 同一个域两份同形，一起走】**
   > `tool-panels` 净 **−306 行**（`terminal.rs` 3593 → 3441、`log.rs` 2400 → 2281、
   > `manager.rs` 1683 → 1663 是上面步骤 B、`table.rs` 1340 → 1325 是级联；`lib.rs` 只改
   > 两条 `pub use`），**四道门 725 → 720 passed / 0 failed / 7 ignored**。删的符号：
   > `TerminalExportJob`/`LogExportJob` 两份 `impl`（`render`/`render_chunk`）、两份
   > `export_job()`、两份 `export_visible_text/csv/json`，级联出 `LogPanel::collect_visible_entries`
   > 与 `MessageSearch::matches`（编译器只报了后一条 —— 前一条测试还在引用，靠人工按"活路径
   > 可达"判掉）。**少的 5 条用例全部点名**：两条 `chunked_export_preserves_*`、一条
   > `export_visible_*` 用例、`MessageSearch::matches` 那条自测、步骤 B 的自测 —— 前两条的
   > 独有字节（CSV 引号双写、HEX 列、TX 过滤）已先折进字面量表，不随删除丢钉。
   > **动手顺序是先换钉子再拆房**：差分用例原本拿 `export_job().render()` 当 oracle，先把
   > 期望换成从活路径捕获的字面量表（terminal 侧时间列经 `fmt_ts` 按本地时间插值，其余框架
   > 全字面量），再用两次变异探针（改活路径表头顺序、剥 `csv_cell` 引号）证明字面量真的会红，
   > 还原复绿**之后**才删 —— 用户下载的字节删除前后逐字相同。这类岛的取证陷阱也记在此：
   > **岛内边的调用者不在 `#[cfg(test)]` 区里，引用计数看起来是"生产"**，死活必须按可达性判。
   > 另：任务清单里挂着的开放计划"把 `terminal.rs` 导出域 ~390 行拆成兄弟模块"由本轮作废 ——
   > **能删的东西不拆**。本条上方第 23 轮那句"`render_chunk` 的存在意义是被等价用例调用"
   > 与疑问 (a) 的"绕过说"两处都已就地标注更正。
10. **`egui_extras` 是可证明的死依赖（实测；2026-10 清理批已删除，见本条第末）**：
   只在 `Cargo.toml:35`（workspace 声明）与 `crates/app/Cargo.toml:16` 出现；
   **`crates/app` 内 0 引用**，全 `crates/` 的 `.rs` 中 0 处代码引用
   （唯一命中是 `crates/app/tests/manifest_deps.rs` 的禁用语清单字符串字面量）。
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
   > **【2026-10 清理批：已按本节配方删除】**workspace 声明与 `crates/app` 的依赖边一起移除，
   > `Cargo.lock` 重解析后净 −36 行：本节预测的 3 个包 `egui_extras`/`enum-map`/`enum-map-derive`
   > 全部消失（多出的一行来自 `tool-extension → tool-plugin-api` 死边，见上文）。

11. **`crates/app` 的自动更新链路在 Linux 下留 9 处死代码诊断（实测，本轮只记录了缺口）**：
   Linux 用 `cfg(not(target_os = "linux"))` 关掉更新 UI 后，`UpdateState`/`CheckResult`
   及其读取方全部变成 never-used / never-read。逐条位置与复现方法见上方
   「Linux clippy 的 `-D warnings` 缺口」。在此之前，CI 的 Linux clippy 用
   `--exclude hardware-workbench-app` 上 `-D warnings`，app 仍跑不带该旗标的整包 lint。

12. **桌面端 / 网页端的体验差异（本轮闭合 6 条，其余如实标注）**：
    - **已闭合（本轮）**：
      ① web 的 `Ctrl+Enter` 是死绑定（键位处理器把 `CMD_SEND` 拦下 `return`，注释声称由面板处理，
      而 `crates/panels` 里发送键位处理是 0 处）—— 早退删掉后与 native 一样可达；
      ② 四个破坏性站点（native / web 的「恢复默认布局」与「恢复所有默认设置」）此前**全是单击即执行**，
      其中「恢复所有默认设置」会清掉串口配置、别名、分组、布局、上限、字体与主题且不可撤销 ——
      现在统一走 `design::confirm_armed` + `design::confirm_click`（与日志/终端「清空」同一份规则），
      确认态用等宽标签；
      ③ web 命令面板的过滤是 `to_lowercase().contains(...)`，不认 `re:`、也不显示非法正则的回退提示
      —— 现在与 native 同样走 `tool_panels::SearchQuery` + `hover_hint`；
      ④ web 成功动作静默（`WebNotificationLevel::Info` 零构造点）—— 恢复布局/恢复默认现在都推通知，
      文案与 native 逐字相同；
      ⑤ web 终端/日志的中文是豆腐块：`setup_web_fonts` 只把中文回退插进 **Proportional** 族，
      等宽族里只有没有 CJK 字形的 JetBrains Mono，而终端/日志按等宽渲染。native 的
      `bootstrap::setup_fonts` 一直是 `Monospace = ["jetbrains", "zh", "emoji", …]`，现在两端同序。
      **本条的验证方式要说清**：依据是两份字体族列表的逐项比对，**没有**截图证据 ——
      内置浏览器面板不可见时 `take_screenshot` 直接拒绝（`visibilityState=hidden`），
      而 kittest 跑不了 wasm 侧那个函数。
      > **本波把这条限制做实了**（免得下一轮再花时间去试）：用 `application/wasm` MIME 正确
      > 的本地静态服务器（`python -m http.server` 不注册 `.wasm`，会把"wasm 没跑起来"和
      > "字体没生效"混在一起）投 `dist-web/`，页面确实加载完成（`document.title` 已是
      > "Hardware Workbench"），截图仍被拒：`visible=false, attached=true, cdpAttached=true,
      > visibilityState=hidden`。再用 `evaluate_script` 查 DOM：`<canvas>` 仍是 **300x150 默认尺寸、
      > 无内联样式** —— 说明隐藏页里 `requestAnimationFrame` 被挂起，eframe 连一帧都没画过。
      > 所以这不是"重试一次就好"的问题：**像素级验证需要一个可见的浏览器表面**（用户打开内置
      > Browser 面板，或用 OS 级窗口控制驱动真实浏览器），而配置级验证（族序逐项比对、
      > 以及下面提到的可测部分）不需要。
      ⑥ 发送器「历史」弹层的排版退化：v1.2.0 删 `legacy_send_panel_body` 时，旧弹层的
      标题+条数/占满宽过滤框/定高行/固定删除列/两步清空**没有**搬进共享 `render_history`，
      留下的是一版"每条一个 `ui.button`"的弹层（按钮宽度=内容宽度，长中文条目把弹层拉到
      窗口宽）。现在共享实现恢复旧排版，两端同时生效，过滤框也接进 `SearchQuery`（从此支持
      `re:`）。守卫与它的边界都写在
      `crates/panels/src/sender.rs::history_popup_keeps_a_fixed_width_and_a_header` 的注释里：
      它钉住宽度上限（删掉 `set_max_width` 实测变红），**钉不住逐段截断**（手绘 galley 不参与
      `min_rect`，把 `galley_width` 换成 `f32::INFINITY` 仍绿）。
    - **仍留**：
      ① native 有浮层 `ToastOverlay`（`crates/app/src/ui/toast.rs`，由 `app/mod.rs` 里
      `self.toast_overlay.show(&ctx, &mut self.notifications)` 那一处驱动，按符号定位），
      web 只在底部状态栏渲染同一条通知。不收的原因：要把它的载体从 native 的 `NotificationQueue`
      泛化到 web 的 `Vec<WebNotification>`，是 UI 结构改动，且本轮无法在浏览器里做回归验证。
      ② wasm 插件沙箱与 native 引擎能力面不同（`load`/`require`/`package`/`debug`/`os.exit`），
      浏览器侧 `serial.close()` 不带参数会关掉所有已开端口 —— 需要产品裁决，不是机械收敛。
      ③ `TransportCapabilities` —— **原先记的"两侧都不被读取（能力位形同虚设）"是错的，本轮按调用点
      重测更正**。共享串口面板真的读它：`grep -n "capabilities\.\w\+" crates/panels/src/serial.rs`
      命中 7 处、覆盖 5 个能力位（`connect`/`disconnect` 在"连接按钮"与"端口列表项"里各一处，
      另有 `send`、`set_dtr`、`set_rts`；那几处都是 `add_enabled(...)` 与 DTR/RTS 的"变了才写"守卫）。
      **真实的不对称在取值来源**：web 把后端上报的能力交给面板（`crates/app/src/web/serial.rs` 里
      `transport_view…map(|view| view.capabilities)`，网络端口才走 `WEB_NETWORK`），
      native 却把交给面板的那份**写死成字面量**（`grep -rn "TransportCapabilities {" crates/app/src`
      命中 `ui/device_panel.rs` 的 `{ request_port: false, ..NATIVE_SERIAL }`，另有 `ui/top_bar.rs`
      的 `show_request_port: false`），所以 `NativeTransport::capabilities()` 的返回值对 native 的
      UI 不起作用；而 7 位里的 `list_known_ports`/`request_port` 两位全仓无人读（`NATIVE_SERIAL` 与
      `WEB_SERIAL` 两份常量还逐位相同）。结论：这条不是"死代码可删"，而是
      **"native 绕过了自己声明的能力"** —— 要让后端上报真正生效，得先决定 `request_port` 在桌面端
      是否该露出"申请端口"入口，那是 UI 行为改动，需要肉眼回归，不在机械收敛范围内。
      ④ 传输失败的文案 —— **原先记成"web 仍是英文、且没有 `transport: ` 前缀"，这个归因不准**。
      按调用点重测：中文映射 `tool_transport::translate_error` 全仓只有**一个**生产调用点
      （`application/src/workbench.rs` 的 `TransportEndpoint::send`，即周期发送那一行），
      而 `platform/src/native.rs` 的 `operation_error` 在能翻译之前就把强类型错误 `.to_string()` 了 ——
      所以**桌面端在同一批失败上同样是英文**，这不是平台差异，是一个调用点丢掉了变体。
      本波已修（有测试）：`tool_platform` 补了自己的 `translate_error`（6 个变体，两端都编译），
      web 侧接在 `application/src/web.rs` 的 `spawn` 缝上（赶在变体被丢掉之前，两个出口
      `snapshot.message` 与 `TaskFailed` 一起覆盖），`operation_error` 改为先翻译再嵌入，
      并去掉 `start_receive_with_disconnect` 那两处双重 `Operation(..)` 包装
      （此前用户会读到两句连在一起的 "…operation failed: …"）。
      两份映射之间"同一件事说同一句话"由 `crates/platform/tests/transport_error_wording.rs`
      机器钉住（实测：只改 `PortNotConnected` 一个字面量，3 条用例变红并打印左右原文）。
      **仍然英文**（本波未动，逐条核过）：`application/src/workbench.rs` 里三处
      `error.to_string()`（网络端口的打开/发送路径，以及 `:1271` 那处喂给 `"would block"`
      字符串重试判定的分支）、`platform/src/native.rs` 里 `"native transport worker stopped"`
      这个英文字面量；另外 `transport: ` 前缀来自 `application/src/error.rs` 的
      `#[error("transport: {0}")]`，而 `AppError` 被 cfg 挡在 wasm 之外，所以**前缀这一项
      仍然是两端不同**（native 有、web 无）。
      > **再更正一次（第 9 轮）**：上面这句里"重连那处喂字符串给重试判定"已经修掉了 ——
      > 判定改读类型化 `kind`（`reconnect_is_worth_retrying`），两条失败出口各自过
      > `translate_error`，不再是英文。同一轮还发现**"三处 `error.to_string()`"这个数是我自己数错的**：
      > `grep -n "error\.to_string()" crates/application/src/workbench.rs` 实际命中 18 处
      > （含 `AppError::Plugin` / `AppError::Transport` 这些包装），**其中哪些真的会到用户眼前、
      > 哪些只是内部转换，还没逐条核过** —— 这是下一轮的取证项，不要引用"三处"这个数。
      >
      > **第 10 轮把这条取证项做完了**：九处传输/平台错误出口（刷新端口、申请端口、两条关闭、
      > 两条连接、两条发送、DTR/RTS）已全部接上两个中文映射器，其中 6 处由 `headless.rs` 真造出
      > 失败并逐字钉住，3 处（`spawn_refresh_ports` / `spawn_request_port` / `open_network_serial`）
      > 在 native 驱动不出失败，改由 `crates/application/tests/transport_error_sites.rs` 钉形状。
      > 剩余的 `to_string()` 转换当时是 12 处、**第 11 轮删掉 native `Workbench::validate_hex`
      > 之后是 11 处**，按"会不会真的到用户眼前"分开算（按符号定位，
      > 这个文件的行号每轮拆分都会漂）：
      > - 正文本来就是中文，只是换个类型装着：`TransportEndpoint::send`、
      >   `dispatch_send`（两处都是 `SendPlanError`，它自己的 `Display` 就是中文句子；
      >   第三处 `Workbench::validate_hex` 已随第 11 轮的死代码清理删除）。
      > - 只是把错误塞进 `AppError` 的某个具名变体，用户读到的是那个变体的框架：
      >   `dispatch` 的两处插件分支与 `set_plugin_setting`（`Plugin`）、`dispatch_start_recording`
      >   （`Recording`）、`AppRuntime::dispatch`（浏览器 composition root；它给已中文化的正文
      >   加上 `AppError` 的英文 `transport: ` 前缀，是"前缀两端不同"那条的同一个根）。
      > - **仍然把英文原文直接摊给用户**：`dispatch_load_replay`（`io::Error`）、
      >   `spawn_reload_plugins` / `spawn_discover_plugins`（插件目录扫描）、
      >   `write_exported_content`（导出落盘）。
      > 最后这四处不是接线能解决的：`tool-extension` / `tool-recorder` / 回放与文件服务
      > 各自没有中文映射器（`tool-transport` 与 `tool-platform` 有）。下一轮要么给这几个 crate
      > 各补一份按变体分发的 `translate_error`，要么承认这些是"面向开发者的错误"并写下理由。
      > 第 9 轮那条教训的正确版本：**不能拿"已经翻译过的用户文案"再当判定的输入**（那才是
      > `"would block"` 那个死判定的成因）；映射器自己去读 OS 原始文案是另一回事
      > （`translate_error` 的 `Serial`/`Io` 两臂本来就这么做，因为它拿到的是 OS 的 `Display`；
      > **第 41 轮起这条只对 `Serial` 臂成立** —— `Io` 臂的权限判定改读 `kind()`，第 42 轮
      > 又让 worker 完成通道把 `io::Error` 本体（而不是它的文案）送到这一臂，见本条末尾的
      > 第 41/42 轮记录）。
      >
      > **第 9 轮留下的两个待拍板问题**（都不是机械收敛能定的）：
      > ① 重连那个 3 秒预算的循环，改成类型化判定之后**仍然没有任何生产者**
      > （串口的 kind 在 `platform/src/native.rs::operation_error` 就丢了；网络的
      > `open_network_serial` 连接失败时照样返回 `Ok`，失败以日志事件发布）。
      > 要么把 `WouldBlock` 生产者接到路上（需要 `tool_platform::TransportError` 携带 kind 的变体），
      > 要么承认这段循环是死代码并删掉、让重连失败即返回。
      > ② 是否把 `ErrorKind::TimedOut`（`close_port_blocking` 超时那条，网络端口真能走到）
      > 也算可重试 —— 那会让那种情况下重连真的试满 3 秒，是用户可见行为变化。
      > 另有一个子代理发现、本轮**没动**的不对称：**【第 12 轮已闭合】**
      > `start_reconnect` 只按 `display_name()` 匹配网络端口，所以 `network://host:port`
      > 形态的 id 会静默走串口分支（与 `is_network_port` 认两种形态不一致）。
      > 实测范围比记下的更宽：这类"这个名字是不是已注册网络端口"的判定在 `workbench.rs` 里
      > 有**六处**（注册 upsert、注销 retain、断开、连接、`is_network_port`、重连），
      > 用的是**三种**规则；第 12 轮把它们收成一条
      > （`network_config_matches` + 建在其上的 `find_network_port`），并规定
      > **写回与作键的名字一律是 `display_name()`**：`TransportManager` 的端口表与
      > `serial:{port}` 有序队列都以它为准，只统一判定不统一名字，会把"连不上"换成
      > "连上了但发不出"（`enqueue_command` 拿 `PortNotOpen`）。
      > 规范化点因此是**五处**（连接、发送、断开、重连、取消重连 —— 最后那处不在原清单里，
      > 是改的过程中发现的：`reconnect_tasks` 的键与取消查询不同源就会静默取消不掉）。
      > 行为差别：native 侧一律传 `display_name()`，**没有可观察变化**；wasm 的网络行以
      > `port_id()` 为键（`crates/app/src/web/serial.rs::add_web_network_port` 实测如此），
      > 所以浏览器里这一行的连接/重连/断开/发送四件事原先全都坏的，现在一起通了。
      > **仍未被任何用例钉住的一条**：`cancel_reconnect` 的规范化 —— 造不出一个"重连正在进行
      > 中"的可达状态（`WouldBlock` 那条没有生产者，见上面 ④），把它的规范化删掉整套用例仍绿。
      > 留在这里而不是假装测过。
      >
      > **第 21 轮：动手翻译之前先拆掉的一颗雷**。上面点名"仍然把英文原文摊给用户"的插件
      > 扫描那两处，取证时查到 **`ExtensionError` 的英文文案正在被一处判定读着**：
      > 组合根的插件重启交接（`crates/app/src/ui/tiles.rs`，`take_pending_restart` 那个循环）
      > 原先写 `if let Err(AppError::Plugin(message)) = dispatch(EnablePlugin) &&
      > message.contains("shutting down")`，判据来自 `ExtensionError::Stopping` 的 `Display`
      > （`plugin '{0}' is still shutting down`）。与第 9 轮那个死判定同形，**但这条是活的**：
      > `PluginManager::disable` 刻意只设 stop 标记、不 join（免卡 UI），Lua 线程要到
      > `lua_host` 事件循环顶部才看那个标记、而循环体默认在 `recv_timeout` 里最多睡 50 ms，
      > 于是「按 Restart → 下一帧 enable 被拒 → 塞回队列再等一帧」是**常态路径**。
      > 照原计划把 9 个变体翻成中文，这条分支立即永久恒假：用户点「重启」再没反应，
      > 而四道门逐字全绿 —— 因为判据在 egui 分支里，那部分没有任何用例经过。
      > 所以顺序反过来了：先换成类型化判定，翻译留给下一轮。
      > `PluginManager::is_stopping` 回答前照 `enable` 的样子先收割一次，`enable` 反过来调用它
      > —— 那条 `stopping_plugins.iter().any(...)` 比较从此只有一份；
      > `Workbench::plugin_is_stopping` 是最窄的转发（`plugin_manager` 字段仍私有）。
      > 判定本身由 `headless.rs::plugin_restart_handoff_relies_on_the_stopping_predicate`
      > 真驱动：运行中为 false → 禁用后窗口内为 true 且同窗口 enable 必被拒、被拒也不吞掉窗口
      > → 收割到线程真正退出后翻回 false 且能重新启用。把 `is_stopping` 改成恒 false 实测变红
      > （恒 true 由第一与第三条断言挡）。它断言的那个"窗口"没有轮询、只观察一次，
      > 所以另测了失败率：空闲连跑 30 次 0 红，整套 headless 并行连跑 5 次 0 红。
      > 文案独立性这条**没法用行为测**（那两行在渲染分支里，测试不跑 egui），改由
      > `crates/app/tests/plugin_restart_handoff.rs` 钉形状：循环体里必须恰好问一次
      > `plugin_is_stopping`、且派发在其后，`.contains(` / `to_string()` / `"shutting down"` /
      > `AppError::Plugin` 一个都不许出现。把旧写法原样贴回去，它的两条断言双双变红，恢复后全绿
      > ——「改文案顺手杀掉重启」从今往后由 CI 拦，不再靠人记得。
      > **本轮没做也不假装有**：web 侧那处重启交接保持原样（已核实 `AppCommand::EnablePlugin`
      > 在 wasm 的 `dispatch` 里只是入队并返回 `Ok`，`Stopping` 在浏览器侧根本不产生，
      > 所以它不是同一颗雷）；但留下一处**已核实、未修**的不对称：派发失败时 web 会把错误写进
      > 状态栏、native 直接丢弃，那是产品可见的取舍，不在机械收敛范围内。
      > 那 9 个变体的中文映射器仍在待办，另有 `dispatch_load_replay`、`write_exported_content`
      > 与 `AppRuntime::dispatch` 的 `transport: ` 前缀也未动。
      > **【后文对账（2026-10）：这四处都已闭合】**① 9 个变体：第 22 轮已译（见下）；
      > ② `write_exported_content`：接了 `tool_platform::storage::translate_error`
      > （`crates/application/src/workbench.rs:1353-1365`）；③ `dispatch_load_replay`：中文框架由
      > recorder 的三个 io 出口生产处补全（`crates/recorder/src/replay.rs`），残留差异只有不带
      > 分类前缀（web 侧同）；④ `AppRuntime::dispatch` 的 `transport: ` 前缀：第 24 轮随
      > `application/src/error.rs` 七个标签中文化一起闭合。
      >
      > **第 22 轮做了那个翻译**（685 → 698）：`tool_extension::translate_error` 按变体分发、
      > **不写 `_ =>`**（与 `tests/error_wording.rs` 的 `variant_name` 同一形状，两处都不写兜底 ⇒
      > 新增变体忘记配中文就编译不过，而不是悄悄把英文漏给用户），接上四处用户可见出口
      > （`Workbench` 的 Enable/Disable 派发、`spawn_reload_plugins`、`spawn_discover_plugins`）。
      > 同一轮删掉 `ExtensionError::UnsupportedApiVersion`：全仓**零构造点**，真实路径是
      > `scan_roots` 里 `api_version_supported()` 为假时推一条中文 `unsupported_api_version`
      > 诊断再 `continue`（与第 20 轮那两个零构造 `AppCommand` 同一类）。
      > 带外来细节的三臂（`Io`/`Manifest`/`Lua(mlua)`）只钉**中文框架**不钉细节 ——
      > 本机实测 `read_dir` 失败给的是「目录名称无效。 (os error 267)」，钉死它就把用例
      > 变成一台机器的快照（`tool_transport` 的 `Io` 臂同一取舍）。
      > **动手时更正三条我自己的前置判断**（都记下来，免得下轮再按错的推）：
      > ① `tool-extension` **不参与 wasm 编译** —— `crates/application/Cargo.toml` 把它放在
      > `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` 里，所以"因为它两端都编译所以
      > 不能复用 native-only 的 `tool_transport::translate_error`"这条理由是错的（真正的原因是
      > 领域不同 + `ExtensionError` 是另一个枚举；顺带一提 `tool-extension` 本来就依赖
      > `tool-transport`）。② `UnsupportedRuntime` 的分支活着但**输入进不来**：`spec::RUNTIMES`
      > 只有 `lua`，而 `scan_roots` 在登记成 record 之前先跑 `manifest.validate()`，
      > 非 lua 的 runtime 早被折成中文诊断，用户读不到这个变体。③ `ExtensionError::Lua`
      > **没有生产者**：`run_plugin` 一律返回 `Ok`（脚本错误走运行时状态与诊断），
      > 全仓唯一的 `Err(LuaHostError::…)` 是 `LuaHost::run_script` 里的 `AlreadyRunning`，
      > 而 `run_script` 零调用者。⇒ 这两臂是下一轮的死代码候选，别当成"已覆盖的用户路径"。
      > **【第 34 轮把这条判据收紧了：`AlreadyRunning` 现在全仓零生产者】**那座岛已随 `LuaHost`
      > 一起删除（见上面第 9 条末尾的延烧清单），于是 `grep -rn 'Err(LuaHostError' crates/` **0 命中**。
      > 两个变体的形状从此**不同，别混为一谈**：`Lua` 臂经 `#[from] mlua::Error` 隐式产生，落点仍有
      > **8 处 `?`**（`run_script_blocking` 4 + `replay::run_replay_analyzer_with_cancel` 4），是活的；
      > `AlreadyRunning` 只剩**读侧**三处 —— `crates/extension/src/lib.rs:103` 的中文映射臂，
      > 以及 `crates/extension/tests/error_wording.rs:99` 与 `:190` 两条**手工构造**的文案钉。
      > ⇒ 上面那句"别当成已覆盖的用户路径"如今更硬：这条臂连运行时入口都没有了。
      > **仍按未拍板处理**：删 `AlreadyRunning` 要不要连带 `ExtensionError::Lua`、`run_plugin`
      > 今后还返不返回 `Result`，是同一个决定的两面，本轮两条臂与其中文措辞一个字都没动。
      > **同一轮新查出的两处英文到用户眼前**（在 `scan_roots` 的诊断里，不在派发路径上，
      > 所以本轮的四条接线没覆盖它们）：`manager.rs` 里
      > `format!("manifest 解析失败: {error}")` 的 `error` **就是** `ExtensionError`，于是插件面板
      > 渲染成双重框架 `manifest 解析失败: manifest parse error: <serde 原文>`；
      > 紧挨着的那条 `permission_denied` 诊断正文干脆是裸 `error.to_string()`（纯英文），
      > 两端各自经 `ui/tiles.rs` 与 `web/plugins.rs` 的面板诊断列表渲染。两处都是一行
      > `translate_error(&error)` 的事，刻意留给下一轮（与 `plugin: ` 前缀同批）。
      > **现在用户点「启用」看到的整句**仍是 `启用插件 demo.x 失败：plugin: 未找到插件 demo.x
      > （请先重新扫描插件目录）` —— 正文已中文化，`AppError` 那句英文 `plugin: ` 前缀还在
      > （已核实全仓没有任何判定读 `AppError` 的 `Display`，所以换掉它是安全的，只是范围另开一轮）。
      > `dispatch_load_replay` 与 `write_exported_content` 仍未接映射器。
      > **【第 24 轮：上面三样一起闭合】**
      > ① `application/src/error.rs` 七个 `#[error(...)]` 的英文分类标签全部改成中文：
      > `传输错误：` / `录制错误：` / `回放错误：` / `存储错误：` / `插件错误：` /
      > `当前状态不允许：` / `命令不支持：`（变体名与载荷类型不动，那是对外 API）。
      > 用户点「启用」现在读到 `启用插件 demo.x 失败：插件错误：未找到插件 demo.x（请先重新扫描插件目录）`。
      > **但两端差异没有因此关掉**：`AppError` 被 `cfg(not(target_arch = "wasm32"))` 挡在 wasm
      > 之外，web 侧仍是**不带任何分类前缀**的那句 —— "native 有一层分类框架、web 没有"这条
      > 按设计仍在，别把本轮记成"两侧错误文案已一致"。动手前按第 21 轮的教训逐个查过
      > `.contains`/`.starts_with`/`==` 对七个旧前缀的读取：零命中（`replay:` / `plugin:` 的
      > 命中全在 DataBus 事件的 `source` 上，与 `AppError::Display` 无关）；唯一的逐字钉是
      > `headless.rs::send_routing_dispatches_by_port_kind_and_rejects_invalid_hex`，改前缀实测
      > 让它变红（`left: "串口错误：HEX …"` vs `right: "传输错误：HEX …"`）。
      > ② `scan_roots` 的 `manifest_parse_error` 诊断改走 `translate_error(&error)`：双重框架
      > `manifest 解析失败: manifest parse error: <serde 原文>` 没了，**而且标签本身此前在说谎** ——
      > `load_manifest` 会同时抛 `Io`（文件读不出来）与 `Manifest`（JSON 不合法），此前两种都被
      > 冠以「解析失败」；现在读不出来那句是 `插件文件访问失败：拒绝访问。 (os error 5)`（本机实测，
      > 细节跟随 OS）。③ `permission_denied` 诊断正文从裸 `error.to_string()` 改走同一个映射器，
      > 面板不再直接渲染 `permission 'filesystem' is not allowed for plugin 'x'`。两条各配用例
      > （`manager.rs` 的 `unreadable_manifest_is_diagnosed_as_io_not_as_parse_failure` /
      > `malformed_manifest_gets_exactly_one_chinese_frame` /
      > `undeclared_permission_diagnostic_is_the_chinese_sentence_not_the_display`；三处变异实测：
      > 退回内插形式 → 前两格红，把「解析失败」写成无条件 → 仍红且打印出重复框架，退回裸
      > `to_string()` → 第三格红）。诊断码 `manifest_parse_error` / `permission_denied` 一字未动。
      > **两条顺手查出来的、与上面记录不符的事实**：(a) 「plugin.json **缺失**」根本到不了这条
      > 诊断 —— `scan_roots` 在 `load_manifest` 之前先 `manifest_path.exists()` 判空并 `continue`，
      > 所以谎报只发生在「文件在、读不出来」这一格；(b) 这两条诊断实际**只有 native 看得到** ——
      > `tool-extension` 不参与 wasm 编译，web 侧插件由用户选文件装载、
      > `crates/app/src/web/plugins.rs` 给面板的 `diagnostics` 恒为 `Vec::new()`，上面写的
      > "两端各自经 `ui/tiles.rs` 与 `web/plugins.rs` 渲染"对 web 那一半是空的（渲染器在，输入永远没有）。
      > **第 25 轮把用户可见错误正文里的英文清完**（700 → 710）。新增
      > `tool_platform::storage::translate_error`（3 个变体、穷尽 `match` 不写 `_ =>`，
      > 逐字契约在 `crates/platform/tests/storage_error_wording.rs`，含"与 `TransportError`
      > 那句『此平台不支持该**传输**能力』不共用任何句子"的断言）。
      > **一个反直觉的取舍**：`StorageError::Operation(detail)` 在这里要套一层中文框架，
      > 而 `tool_transport` 那侧的 `Operation` 是**原样透出**的 —— 差别在载荷：transport 侧的
      > 载荷进门时已经翻译过，再套就成了「操作失败：串口错误：…」；storage 侧七个生产者
      > （`native storage worker stopped` / `std::io::Error` 原文 / `file id escapes storage root` /
      > `window unavailable` / JS 值原文 / `decode settings: ` / `serde_json` 原文）
      > **一个中文都不是**，所以不套就整句英文。清单与各自语言记在那份新用例的文件头里。
      > 接了三处：`write_exported_content`（导出失败 → 任务消息 → 状态栏，本机实测从
      > `storage operation failed: 系统找不到指定的路径。 (os error 3)` 变成
      > `存储操作失败：系统找不到指定的路径。 (os error 3)`）；`config.rs::read_native_file`
      > （**本轮新查到的活站点**：设置面板「最近工作区 → 打开」把 Err 正文
      > **原样** `set_status_force(Error, error)` 渲染，实测前值是
      > `storage operation failed: 拒绝访问。 (os error 5)`，把 `.map_err` 退回 `to_string()`
      > 该用例即红）；`write_native_json` 今天到不了用户眼前（`save_config` 十个调用点都只
      > `log::warn!`），统一它只为"同一个失败只有一种说法"，注释里写明了这点。
      > 回放那条改在**生产者**而不是接线处：`prepare_load` 里 `File::open(&path)?` 是唯一没框架
      > 的一处（邻居 `:241` 与 `oversize_file_error` 本来就是中文），在 `workbench` 那侧补会
      > 把已带框架的路径套成双重框架。**顺带纠一处可达性**：文件"不存在"根本走不到
      > `File::open`（`metadata` 先失败，用户看到的本就是中文那句），新框架管的是
      > 目录 / 被独占 / 权限。同形状但不同命令的一处（`create_recording_file` 的
      > `create_dir_all` / `File::create`）**第 40 轮补上了**（这一条当时只记不修）：两道出口
      > 各一句中文框架、各点名自己的路径 —— `创建录制目录 <父目录> 失败：<OS 原文>` 与
      > `创建录制文件 <目标> 失败：<OS 原文>`，形状取自 `tool_updater` 里同样两处操作。
      > 两处都按 `io::Error::new(err.kind(), ..)` 包，**不跟**回放那三条的 `io::Error::other(..)`
      > （它们把 kind 折成 `Other` 了，第 25 轮的取舍没写理由）：`kind()` 在本工作区有承重用途
      > （第 9/11 轮的重试判定、`tool_transport::translate_error` 的 `Io` 臂读的都是它）。
      > **第 43 轮把回放那三处的同一笔账平了**（`replay.rs` 的 `prepare_load` 取大小 /
      > `File::open` 与 `prepare_load_lines` 逐行读）：三条出口改成
      > `获取文件大小 <路径> 失败：<OS 原文>` / `打开回放文件 <路径> 失败：<OS 原文>` /
      > `读取回放文件 <路径> 失败：<OS 原文>`，框架里的冒号由半角 `: ` 换成与
      > `recorder.rs` 一致的全角 `：`，包装一律 `io::Error::new(e.kind(), ..)`（Display
      > 逐字不变、kind 却折成 `Other` 的写法有用例单独抓：`Display` 相同 ⇒ 文案用例全绿
      > 也照样是坏的，只有 `kind()` 看得见）。**这一族在本 crate 已闭合**：
      > `prepare_load_text` 那句 `io::Error::other("回放文本解析未完成")` **保持不动** ——
      > 它底下没有 OS 错误、没有 kind 可保，代码里就地写了这句话，别当漏网的顺手改掉。
      > **还剩两条同类小账，都在别的 crate**：`crates/app/src/runtime/autosave.rs` 里
      > `format!("录制失败: {e}")` 那一格仍是半角 `:`（同一文件的另一格与 `recorder.rs`
      > 全用全角 `：`）；本轮普查又抓到**这条回放路径上**的另一格 ——
      > `crates/application/src/workbench.rs` 里 `AppEvent::TaskFailed` 那句
      > `format!("后台任务失败: {error}")` 也是半角 `:`（它是所有后台任务失败进系统日志的
      > 公共出口，改它会影响导出/扫描插件等一整批句串，不是顺手改的量）。两处本轮都没动，
      > 下一轮的候选。
      > **再纠一处可达性**：这句到用户眼前的路是 native **状态栏**（`commands.rs` 的
      > `start_or_stop_recording` → `set_status_force(Error, error.to_string())` → `AppError::Recording`
      > 的 `录制错误：{0}`），**不是**录制面板那句 `录制错误：{error}`（它读
      > `RecorderStats::last_error`，只有 worker 的写/刷盘失败进得去，起录失败根本不写它），
      > 也不是 web 状态栏那个把原因挂在 hover 文本里的 `录制错误` 徽标（web 起录不落盘，
      > `create_recording_file` 在 wasm 侧结构上跑不到）。逐字契约在
      > `crates/recorder/src/recorder.rs` 的 `create_recording_file_frames_both_io_exits_in_chinese`。
      > **踩过的坑记下来**：`tool-platform` 现在有两个 `translate_error`
      > （根目录那份管 `TransportError`，`storage` 模块那份管 `StorageError`，各自与自己的
      > 错误类型同处），从根路径调 storage 那份会直接 E0308 —— 不是重名错误，但引用时要写全模块。
      > **第 41 轮：这条链上「别拿错误文案当判定」的又一处实例，修在了 `Io` 臂**。
      > `tool_transport::translate_error` 的 `Io` 臂此前先过 `is_permission_denied`（五个英文短语
      > 的子串）再 `match e.kind()` —— 它匹配的是 OS 的 `Display`，随系统语言本地化：本机（中文
      > Windows）权限失败实测「拒绝访问。 (os error 5)」，一个短语都不含，Windows 报的码也是 5
      > 不是 13，权限建议在本地化机器上永不出现（与第 9/11/21 轮同族：判定与翻译共读同一个
      > String）。修法照本族的规矩：**判定读类型，文案降级为细节** —— 权限格并进同一个
      > `match e.kind()`（`PermissionDenied => serial_permission_message(&message)`），OS 原话逐字
      > 留在冒号后。`raw_os_error()` 核实后**不需要**兜底：`from_raw_os_error` 造的错必已过 std
      > 码表（Windows `ERROR_ACCESS_DENIED`/`WSAEACCES`、POSIX `EACCES`/`EPERM` ⇒
      > `PermissionDenied`，std 1.92 `pal/unix` 与 `pal/windows` 的 `decode_error_kind`；Windows
      > 两格另有本机用例实测），`Error::new`/`other` 造的错又没有原始码 —— 补一份码表是永远
      > 多不出命中的死分支。反向红利：Windows 的 `os error 13` 是 `ERROR_INVALID_DATA`，旧
      > `"os error 13"` 那格（码抄自 POSIX EACCES）此前把它错包成串口权限建议，现已落回通用框架。
      > **英文 Windows / Linux 的带 kind 路径逐字不变**（有用例钉「修中文没坏英文」）。
      > **剩下的更难一半是 `Serial` 臂（可达性已核，本轮按要求未动）**：`sp::Error` 带类型化
      > `kind`（`ErrorKind::Io(io::ErrorKind)`），serialport 的 POSIX 后端把 EACCES 映射成
      > `Io(PermissionDenied)`，按 kind 可判定；但 **Windows 后端 `last_os_error` 把
      > `ERROR_ACCESS_DENIED` 折进 `ErrorKind::NoDevice`**（与 FILE/PATH_NOT_FOUND 共用），权限
      > 信息在依赖内部就已销毁 —— Windows 半边按 kind 分不出「端口被占用」与「设备不存在」，
      > 逐平台分叉要把 `serialport::Error` 的两套形状先写进用例，不是 `Io` 臂这一格的事。
      > **第 42 轮把这条链上的另一处同族病根闭合了：worker 完成通道不再抹 kind**。
      > `SerialCommand` 的 `completion` 载荷从 `Sender<Result<(), String>>` 换成
      > `Sender<io::Result<()>>`（`pub(crate)` 内部，不出 crate），`wait_for_completion`
      > 把收到的错误**原样**包进 `TransportError::Io` —— 旧写法发 `.to_string()`、等待方再
      > `Error::other(文本)`，kind 恒为 `Other`，第 41 轮那几臂对走过通道的失败全部不可达
      > （就是第 41 轮如实记下的那处收窄）。`io::Error` 不是 `Clone`，所以「先记日志、后交给
      > 通道」成了三个生产者（`complete_command`/`finish_signal_command`/
      > `report_completion`）的共同顺序；唯一例外是 Windows 的写臂：原件要跟着 `result?` 交给
      > `NativeWorker::run`（它读 `raw_os_error()` 抑制"关闭中被取消"的噪声日志），通道拿的是
      > `completion_copy` 造的 **同 kind 同 `Display`** 副本（1.92 实测：Win32 码 0..4000 里没有
      > 任何一格映射成 `InvalidData`/`WouldBlock`，故 kind+文案就是等待方读得到的全部）。
      > 本 crate 内普查：`Io(WouldBlock)` 的唯一生产者仍是 `open_serial` 的直接 return（正文
      > 已是中文「… 正在关闭中，请稍后重试」，不走通道），`Io(InvalidData)` **没有**生产者 ——
      > 两格臂留着、注释已改写实测结论，没编造用例。剩下的未闭合一半只有 `Serial` 臂。
      ⑤ 【本波已闭合】换行后缀表原有 3 份（native `state::LineEnding`、wasm `WebLineEnding`、
      共享面板 `SendLineEnding`），值一致但没有任何断言钉住，两端还各写两份 4 臂 **identity** `match`
      （类型不同才需要转）。现在收进 `tool_core::LineEnding`（`crates/core/src/lib.rs:454`），
      三处只留别名（`pub use ... as SendLineEnding` 保持面板对外名字可导入），四处转换随之删掉。
      持久化的裸变体名（`"None"`/`"Lf"`/`"Cr"`/`"Crlf"`，桌面 `workspace.json` 与浏览器设置都存它）
      由 `line_ending_persisted_names_are_bare_variant_strings` 按**字面量**钉住 —— 给合并后的枚举加
      `#[serde(rename_all = "lowercase")]` 实测打红 3 条、去掉即回绿，所以这条断言不是摆设；
      三份表本身由 `line_ending_label_and_suffix_tables` 与 `line_ending_default_is_none` 钉住
      （变体顺序、标签、后缀字节序、默认值）。
      **另有实机证据**：本波真开了一次桌面窗口（`target/debug` 的构建），发送器的换行下拉
      按 `ALL` 顺序渲染出「无 / LF / CR / CRLF」，而那次运行写回的
      `%APPDATA%\HardwareWorkbench\workspace.json` 里是 `"line_ending": "None"` ——
      持久化形状在真实读写路径上保持裸变体名，不只是单测里成立。
      ⑥ web 把 `webserial:session:N` 会话别名键一起持久化 —— **本轮把机制追出来了**：
      `platform/src/web_serial.rs::allocate_session_id` 每次页面载入都从 `next_session_id = 0`
      重新发号，而别名表 `serial.port_aliases` 是**按端口 id 存键**的
      （读写见 `web/mod.rs` 的 `settings.port_aliases ↔ serial.port_aliases` 与
      `state.rs:342` 的 `port_aliases.get(port)`）。于是给一个"已授权但没配对到持久描述符"的
      端口起的别名，下一次载入键名就变了：别名永远不生效，而旧条目在浏览器设置里**只增不减**。
      修法方向：保存时过滤掉无法稳定重算的键（或改用能跨载入稳定的身份做键）。
      这是持久化行为变化（会清掉用户已有的、本来就不生效的条目），需要配套测试与
      一条"重新载入后别名仍在"的断言，故未与本轮的机械改动混做。
      ⑦ 【第 24 轮查出来的】web 的插件面板**永远不会显示任何扫描诊断**：共享渲染器是接好的
      （`crates/app/src/web/plugins.rs` 把 `plugin_view.diagnostics` 交给面板），但喂进去的那份
      被写死成空 —— `set_plugins_view(PluginView { summaries:…, diagnostics: Vec::new() })`。
      于是同一件"插件清单坏了"的事：native 在面板里给一条中文诊断（本轮刚把它修成不说谎的
      `插件文件访问失败：…`），web **什么反馈都没有**，用户只会看到插件莫名其妙不在列表里。
      这不是文案问题也不是死代码，而是"渲染器在、输入永远为空"的半接线路。
      收它需要先回答一个产品问题：浏览器侧的插件来源与 native 不同（web 走 marketplace /
      `tool-plugin-runtime`，`tool-extension` 根本不参与 wasm 编译），所以"web 的扫描诊断该由谁产生、
      包含哪些码"是新增行为而不是将现有行为对齐 —— 需要用户定调，且改动会新增用户可见文案，
      不能算进机械收敛。
      ⑧ 【第 25 轮顺手查到的，与文案不是同一类】**配置读写的失败在两端都被丢掉**，
      用户看到的症状是"设置没保存上"而**没有任何一句话**：native 侧 `load_config()`
      （`crates/app/src/config.rs`）用 `if let Ok(..)` 吞掉读取错误后直接落到 legacy 路径，
      `save_config()` 的十个调用点全部只 `log::warn!`（`app/mod.rs`、`commands.rs`、
      `runtime/autosave.rs`、`runtime/keys.rs`、`runtime/update.rs`、`ui/device_panel.rs`、`ui/mod.rs`）；
      web 侧 `WebSettingsStore::from_window(...).ok()` 与 `let _ = store.save("settings.json", …)`
      两处同样丢弃。本轮只把**已经到达**状态栏的那条（`read_native_file` → 设置面板
      「最近工作区 → 打开」）换成中文，没动这些丢弃点 —— 动它等于新增用户可见反馈
      （"启动时配置读不出来要告诉用户吗、要不要退到默认值"），是产品裁决不是机械收敛，
      且与第 21 轮那条"native 丢弃 enable 失败、web 写状态栏"的不对称是同一个问题的两面。

### 本轮裁掉项

13. `crates/app/Cargo.toml` 删除 `crossbeam-channel`（本 crate 0 引用，工作区其它 crate 仍在用）；
    `crates/panels/Cargo.toml` 删除 `tool-marketplace`。两者都会改写 `Cargo.lock` 的依赖边
    （无版本变更、无新增行）。

