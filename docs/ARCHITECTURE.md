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
| Application (`tool-application`) | 行为与状态 | `Workbench::dispatch(AppCommand)`, `send_plan::plan_send`（send 命令 → (任务种类, 字节) 的唯一决策点，无 `cfg` 门控、wasm 侧同调）, `TerminalService`, `ApplicationConfig` |
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
2. 在 `workbench.rs` 实现 `dispatch` 分支 + `query`/`TerminalService` 扩展；
   **`dispatch` 有两套实现**（native `workbench.rs` / wasm `web.rs`，变体数已实测不同：
   仅 native 有 `ExportLog`/`ExportTerminal`/`LoadReplay`，仅 web 有
   `InstallMarketplacePlugin`/`LoadReplayText`），只改一侧就是埋下「同一命令在两端行为不同」
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
  test 为 **585 passed / 0 failed / 7 ignored**（22 targets；= Task 6 落 `send_plan` 前的
  578 + `send_plan` 的 6 条单测，该模块无 `cfg` 门控所以跑在 native 测试目标里
  + Task 6 复审轮的 1 条 `headless.rs` 端口命名形态用例）。
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
   - **HEX 判定的残留重复（实测，刻意未收敛）**：判定的唯一真相已在 `tool_core::{parse_hex,
     parse_hex_strict, hex_preview}`（native 与 wasm 共用），但仍有 3 处各自实现：
     | 位置 | 为什么留 |
     |---|---|
     | `crates/panels/src/sender.rs:517` `hex_preview` | 与 `tool_core::hex_preview` **渲染不同**：空输入返回 `""` 而非 `"—"`、无 `|ASCII|` 列、无 32 字节截断、报错带具体原因。二者谁都不该被悄悄替换 —— 那会改变界面输出。 |
     | `crates/panels/src/sender.rs:491` `hex_error` | 同上：它只判「能不能发」并渲染红色标签文案（`"HEX 中包含无效字符：{token}"`），文案本身是渲染结果。它是**校验器**而非解码器，不被 `grep parse_hex` 命中，容易被漏数。**测试覆盖是本轮补的**：此前该文件 `#[test]` 数为 0，把严格档的 `len() != 2` 放松成 `len() > 2` 全套 585 条仍全绿；现在 `sender.rs` 的 `mod tests` 钉着整张真值表（`gate_has_exactly_one_verdict_per_input_and_mode`、`strict_mode_rejects_every_length_except_two`）与那个分裂格（`gate_and_decoder_disagree_on_repeated_0x_prefix`），上述变异实测打红 2 条。 |
     | `crates/plugin_runtime/src/web_lua.rs:2508` `parse_hex` | wasm 插件 Lua API `ctx.serial.send_hex_to` 的后端，语义与 `tool_core::parse_hex` 不同（要求偶数长度、不认 `0x` 前缀），收敛即改动已发布文档（`docs/lua-plugin-api.md`）承诺的插件行为；且 `tool-plugin-runtime` 目前不依赖 `tool-core`，新增该边会改写 `Cargo.lock`（本轮约束为锁文件不变）。 |
     另：`crates/lua_host/src/codec.rs` 的 `from_hex` 是**有意**独立的 Lua 侧解析器，其归一化规则与
     `tool_core::parse_hex` 对齐，由该文件的 `#25` 用例用同一段输入双向比对钉住。
   - **发送路径的残留差异（实测，刻意未收敛）**：`send_plan::plan_send` 收敛的是**判定**，
     不是判定的**输入**。任务种类现在只有一个决策点，但它的唯一路由输入 `is_network: bool`
     仍由各平台自己算：
     | 差异 | native | web | 为什么留 / 后果 |
     |---|---|---|---|
     | `is_network` 的**计算规则** | `Workbench::is_network_port` 比对 `app_config.network_ports` 的字符串；本轮起同时认 `display_name()`（`host:port`）与 `port_id()`（`network://host:port`）两种形态 | `WebApplication::is_network_port` 查 `network_ports` 的键，键只在 `port_id()` 形态下写入（`crates/application/src/web.rs` 的 `RegisterNetworkPort`） | 两种形态都**被持久化**（native 的 `workspace.json` 存 `network_ports`、web 存浏览器设置），统一命名会打断已存工作区，故**不统一命名**。跨平台误分类是**响亮**的：native 形态的 id 到了 web 会走到 `WebSerialTransport::enqueue_command` → `TransportError::PortNotConnected`（`crates/platform/src/web_serial.rs:504`），字节发不出去而不是发往错误设备。native 此前**内部不自洽**（`RemoveNetworkPort` 认两种形态、`is_network_port` 只认一种），本轮闭合，由 `crates/application/tests/headless.rs::network_port_ids_are_recognized_in_both_naming_forms` 双向钉住（两形态→`send_network`，普通串口名→`send_serial`）。 |
     | HEX **预检的规则来源**（round-1 把这一行记成「预检档位」的两端分歧，**那个分歧不成立** —— 依据的站点是死代码，见本行末与下方更正） | **活着的**按钮门禁是共享面板 `crates/panels/src/sender.rs:304-309`：它传 `*view.hex_strict`（默认 `true`，`crates/app/src/state.rs:307`），「严格」勾选框也是活的（`sender.rs:183-185`）。本平台自己的 `Workbench::validate_hex` **没有任何活调用点** —— 仅存的两个（`bottom_panel.rs:658`、`:752`，都传 `false`）在 `legacy_send_panel_body` 的死子树里（`bottom_panel.rs:279-340`，本文件第 8 条已记它全工作区 0 调用） | 按钮门禁是**同一个** `sender.rs:304-309`（`crates/app/src/web.rs:4947` 也调 `tool_panels::sender_ui`）；`web_hex_error` → `WebApplication::validate_hex` → `tool_core` 只出现在 `send_web_current`（定义 `crates/app/src/web.rs:3231`，预检站点 `:3249`）这一条路上，而它的**可达手势是命令面板、不是键位**：web 的键位处理器在 `:3136-3138` 就把 `CMD_SEND` 拦下直接 `return`（注释称"发送键位由面板自己处理"，可 `crates/panels` 内 `Key::Enter` 的发送处理是 **0 处**，面板提示却写着「Ctrl+Enter 发送」（`sender.rs:226`）—— 这条 web 键位疑似死绑定先于本分支，未在本轮改动），于是 `execute_web_command` 的 `CMD_SEND` 分支（`:3144`）只由命令面板的确认路径到达（`:4063`） | 「`"abc"` 在 native 判可发（按钮亮着）、在 web 判不可发」这句 round-1 的结论是**错的**：两端按钮都由 panels 那份校验器按 `hex_strict` 判，默认严格档下 `"abc"` 在**两端都被拒**（`sender.rs:507`：规范化长度 3 ≠ 2）。真正的残留是**规则来源**、不是档位：活的门禁用 `crates/panels/src/sender.rs:491` 那第三份 `hex_error`（只剥**一层** `0x`），而发送与 web 那条面板外发送路（`send_web_current`）的预检用 `tool_core`（`normalize_hex_token` 的 `trim_start_matches` 反复剥），于是默认严格档下 `"0x0xAB"` 一边判非法（规范化后 `0xAB`，长度 4 ≠ 2）、一边判合法（`AB`）：**两端**都会把这串的发送按钮灰掉，而 `dispatch` 其实接受它 —— 且这条路**用户能走到**：native 的 Ctrl+Enter 走 `crates/app/src/commands.rs:212` 的 `cmd_send_if_ready`（只查「端口已打开 + 输入非空」，**不做 HEX 预检**）→ `do_send` → `dispatch`，web 走 `send_web_current`（命令面板确认那条路，预检用 `tool_core`，同样放行），两边都会把这串真发出去。行为保持的收敛不动它（改了会改变渲染出的 UI）。**本波已把这个分裂两侧的实际行为都变成机器断言**：门禁侧 `crates/panels/src/sender.rs::gate_and_decoder_disagree_on_repeated_0x_prefix`（连同该文件新增的真值表），解码侧 `crates/application/src/send_plan.rs::strict_and_lenient_have_exactly_one_verdict_each` 的 `"0x0xAB"` 一格 —— 今后任何人统一它，会先看到红测试，而不是只看到这张表。**连带事实（按可达性归因）**：`Workbench::validate_hex` 的 `strict` 形参在 native 既没有 `true` 的调用者、也没有活的调用者，更没有测试 —— 因为那两个调用点整体不可达。 |

     > **错误文案（对先前记录的更正）**：Task 6 **改掉了** native 的非法 HEX 文案，此前写作
     > 「两平台错误文案逐字未变」是错的。本轮之前 native 的 dispatch 错误是
     > `AppError::Transport(TransportError::InvalidHex(..).to_string())`，渲染为
     > `transport: invalid hex input: <原因>`。任务内两步各有其责：Part A（`25d2c38`）把
     > `tool_transport::parse_hex*` 换成 `tool_core::*` 并用 `format!("HEX 解析失败：{原因}")`
     > 重新包装（`transport: ` 前缀之外的中文串就是那时换的），Part B（`74719a6`）又把这句话
     > 搬进 `SendPlanError: Display`。今天渲染为 `transport: HEX 解析失败：<原因>`
     > （`transport: ` 前缀来自 `AppError`）。判定不变，变的是披露与措辞。
     > **本轮发布的证据原文**（native 输入 `"AB C"` + `strict: true` 经 `dispatch` 的完整渲染）：
     > `transport: HEX 解析失败：严格模式: "C" 规范化后为 1 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式`
     > **这句有测试钉着**：`crates/application/tests/headless.rs::
     > send_routing_dispatches_by_port_kind_and_rejects_invalid_hex` 除断言变体外，还断言
     > 该输入的 `strict_error.to_string()` 逐字等于上面这整行 —— 改 `SendPlanError` 的
     > `Display`、改 `AppError::Transport` 的 `#[error("transport: {0}")]` 前缀、或改
     > `tool_core` 严格模式的文案，都会让该用例变红（两条变异均实测过，见 task-6 报告 M7/M7b）。
     > round-1 给两处 hover 加的 `hex_precheck_hint`（应用点 `crates/app/src/ui/bottom_panel.rs:661`、
     > `:754`）**位于不可达的子树**：那两个站点属于 `legacy_send_panel_body`（`:279-340`，
     > `#[allow(dead_code)]`，全工作区 **0 调用**，见本文件第 8 条）。所以 round-1 写成
     > 「用户在看板里读到带前缀的句子」的那半条 **UX 回归从未到达过用户** —— `1a96efb` 起
     > 这两个函数就没有调用者，Task 6 只是换了死代码里的文案写法。**活的**用户可见改动是
     > **红字标签**（不是状态栏）：`bottom_panel.rs:219`、`:234`、`:843` 三处 `.to_string()`
     > 都写进 `self.send.error`，它经 `SendView.error`（`bottom_panel.rs:170` 处装配）由
     > `crates/panels/src/sender.rs` 的 `ui.colored_label(theme::red(), error)` 渲染成红字，
     > 按设计**仍带 `transport: ` 分类前缀**（`#[error("transport: {0}")]`）。
     > **状态栏与 HEX 无关**（round-2 把两者并列是错的）：该文件里活着的状态栏错误站点是
     > `:245`、`:256`（panels 的 `SendAction::SetDtr` / `SetRts`）与 `:791`、`:807`
     > （`send_signal_controls` 的 DTR/RTS 复选框），四条全是 DTR/RTS，没有一条来自 HEX。仍然没有测试守着的是那两处 hover
     > 字符串本身：把 `hex_precheck_hint` 还原成 `error.to_string()` 后四门全绿
     > （585 passed / exit 0）—— 该文件的 `mod tests` 只测 egui 布局、从不渲染
     > `WorkbenchApp`，站点又不可达，故没有观察点。
     > 可达性核对（本轮在当前树上重跑，非继承自上轮结论）：
     > ```bash
     > grep -rn 'legacy_send_panel_body' crates/
     > # 5 处命中，其中只有 1 处是函数体定义（`app/src/ui/bottom_panel.rs:280`），
     > # 另 4 处全是注释/文档提及：`bottom_panel.rs:30`、`:273`、`:644` 与
     > # `application/src/workbench.rs:1077`。—— 上一版把期望输出写成"只命中定义处
     > # 1 行"是**错的**（错在把"提及"当"调用"，且计数发布它的那次提交就已经不成立）。
     > # 这条字符串计数因此**不能**用来判可达性，判可达性看下面这条：
     > grep -rn 'self\.render_send_actions' crates/
     > # 1 处命中：`bottom_panel.rs:327`，位于 legacy 体内（即唯一调用点整体不可达）
     > ```
   - **下一轮候选（把漂移的门真正关上）：`plan_send` 收类型化目标，不收裸 `bool`** ——
     改签名为 `plan_send(cmd, SendTarget::{Serial, Network})`，由两平台**已经持有**的端口种类
     解析：`tool_platform::PortDescriptor::kind`（`crates/platform/src/lib.rs:112`，网络端口在
     `descriptor()` 里就带 `PortKind::Network`）与 native 侧 `SerialPortDescriptor.port_type`
     （注册于 `crates/application/src/workbench.rs:293`）。理由：`is_network: bool` 只能表达
     「不是网络就是串口」，于是「端口根本不存在」被折叠成「串口」；有了第三种取值才能把 unknown
     当场拒绝，也才让上面那张表的第一行不再靠命名约定对齐。
   - **wasm 侧发送路径的测试缺口（实测尺寸）**：Task 5 的字节契约是发送路径上唯一被**执行**的
     **端到端投递**守卫，而它只守 native 半区 —— `headless.rs` 按构造是 native-only（文件顶部直接
     `use tool_application::{Workbench, AppError, ..}`），`crates/app/src/web.rs` 无 `mod tests`，
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
     以及整层 wasm 呈现 —— 含 `crates/app/src/web.rs:371-375` 的 `web_hex_error` →
     `runtime.validate_hex`（Part A 正是在这里删掉手抄副本；未来一次编辑若丢掉 `serial.hex_strict`
     或不理会 runtime，没有任何测试会响）。成立的推论：判定逻辑已不再住在 `web.rs` 里，
     所以缺口的**内容**是投递与呈现，不再是规则本身 —— 但每往 `web.rs` 加一条判定，就是往这个
     零覆盖区里加一条。
   - **更新清单的 pinned 摘要只约束 native —— wasm 读同一份 `update.json` 却完全不看它**
     （实测，本轮**只登记不修**）：
     | | native | wasm |
     |---|---|---|
     | 清单 DTO | `tool_updater::update_info::UpdateInfo`（`crates/updater/src/update_info.rs:9-22`），`sha256` 是**必填**字段（无 `#[serde(default)]`），再过一道 `is_pinned_sha256`（同文件 `:25-27`）形状门 | `WebUpdateInfo`（`crates/application/src/web.rs:158-165`）只有 `version`/`date`/`download_url`/`changelog`，**没有 hash 字段** |
     | 结果 | 清单缺 `sha256` ⇒ 解析即失败 ⇒ 「检查更新」报错，绝不放行一次无法校验的更新 | `:868` 把它填进 `updater::UpdateInfoView`（`crates/application/src/updater.rs:4-9`，同样没有 hash 字段），`crates/app/src/web.rs:3738` 与 `:4396` 把 `download_url` 直接交给 `open_web_url` |

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
8. **`crates/app` 存在死代码子树（实测，未删）**：
   `crates/app/src/ui/bottom_panel.rs:279-340` 的 `legacy_send_panel_body` 带
   `#[allow(dead_code)]`（`:279`），全工作区 **0 调用**，块长 **62 行**（含 `#[allow]` 行，
   279..340；早期估为"约 350 行"，已按实测更正。行号会随上方插入漂移：`249-310` →
   round-1 加 `hex_precheck_hint` 后 `265-326` → round-2 加注释后 `279-340`；**本文件的行号按
   本轮提交实测**，符号名才是稳定锚点）。
   **它把整族渲染函数拖成不可达 —— 实测 13 个，不是"三个"**（此前写"三个"，终审的更正
   建议写"四个"（补 `render_send_error`），两者都仍低估；下面是逐条实测的闭包）：
   死体 `legacy_send_panel_body`（`:280`）直接调 `render_send_options`（`:286`）、
   `render_send_input`（`:324`）、`render_send_actions`（`:327`）、`render_send_error`（`:328`），
   而 `render_send_actions`（`:576-622`）又调 `render_send_and_clear_buttons`（`:589`/`:611`）、
   `send_history_combo`（`:590`/`:612`）、`render_periodic_controls`（`:601`/`:617`）、
   `send_signal_controls`（`:603`/`:619`）、`render_hex_preview`（`:604`/`:620`），
   `render_send_options` 再调 `render_send_target_options`（`:346`）、`render_hex_toggle`（`:392`）、
   `render_line_ending_combo`（`:393`），后者又调 `render_send_target_options_row`（`:362`/`:372`）。
   这 13 个函数的**每一个**调用点都落在这 13 个函数体内部，族外 0 引用（核对命令见下）；
   唯一的例外是 `do_send`（`:678` 虽在死族里调它，但 `crates/app/src/commands.rs:214`
   的 Ctrl+Enter 路径活着，故 `do_send` 本身可达）。
   编译侧只报 `legacy_send_panel_body` 一处，是因为 rustc 的 `dead_code` 见调用边即算"用过"、
   不沿死调用者向下传递，所以那 12 个函数各自都不报警 —— **别把"没报警"读成"可达"**。
   连带后果：**native 的 `Workbench::validate_hex` 仅有的两个调用点（`:658`、`:752`）就在这一族里**，
   所以那个函数在本平台没有任何活的调用点。核对（本轮在当前树上重跑）：
   ```bash
   grep -rn 'legacy_send_panel_body' crates/
   # 5 处命中：函数体定义 1 处（`bottom_panel.rs:280`）+ 注释/文档提及 4 处
   # （`bottom_panel.rs:30`、`:273`、`:644`、`application/src/workbench.rs:1077`）。
   # 这条串计数判不了可达性（"被提到"≠"被调用"），可达性看下面三条：
   grep -rn 'self\.render_send_actions' crates/   # 1 处命中：`bottom_panel.rs:327`，在死体内
   grep -rn 'self\.render_send_error' crates/     # 1 处命中：`bottom_panel.rs:328`，在死体内
   grep -rn 'validate_hex(' crates/app/src/       # 恰好 3 行：native 两处（都在死子树）+ wasm 的 :373
   ```
   删除它是**真**改进（连带清掉两份 `hex_precheck_hint` 调用点），但属另一个任务的裁决，
   Task 6 按约束**不删**。
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

11. **`crates/app` 的自动更新链路在 Linux 下留 9 处死代码诊断（实测，本轮只记录了缺口）**：
   Linux 用 `cfg(not(target_os = "linux"))` 关掉更新 UI 后，`UpdateState`/`CheckResult`
   及其读取方全部变成 never-used / never-read。逐条位置与复现方法见上方
   「Linux clippy 的 `-D warnings` 缺口」。在此之前，CI 的 Linux clippy 用
   `--exclude hardware-workbench-app` 上 `-D warnings`，app 仍跑不带该旗标的整包 lint。

### 本轮裁掉项

12. `crates/app/Cargo.toml` 删除 `crossbeam-channel`（本 crate 0 引用，工作区其它 crate 仍在用）；
    `crates/panels/Cargo.toml` 删除 `tool-marketplace`。两者都会改写 `Cargo.lock` 的依赖边
    （无版本变更、无新增行）。

