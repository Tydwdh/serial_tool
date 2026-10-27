# 架构

面向维护者。事实以工作区代码为准；**已知差异、构建/测试缺口与未闭合工作**登记在 [known-gaps.md](known-gaps.md)。

## 1. 分层与依赖边界

工作区成员 crate 一览（`Cargo.toml`）：

| crate | 职责 | 关键位置 |
| --- | --- | --- |
| `tool-core` | `Event`/`Payload`/`LogLevel`、topic 常量、hex 解析、原子写、时间助手 | `crates/core/src/lib.rs` |
| `tool-databus` | 唯一事件总线：订阅模式、历史、性能计数 | `crates/databus/src/lib.rs` |
| `tool-platform` | 平台抽象：存储、native 端点、Web Serial / WebSocket / fetch | `crates/platform/src/lib.rs` |
| `tool-transport` | 端口 worker：本地串口 / 网络串口 / 虚拟串口 | `crates/transport/src/lib.rs` |
| `tool-recorder` | JSONL 录制与回放（零耦合的两个 manager） | `crates/recorder/src/{recorder,replay}.rs` |
| `tool-application` | UI 无关的应用核心：`Workbench`（native）/ `WebApplication`（web）、命令、DTO、任务 | `crates/application/src/workbench.rs`、`web.rs` |
| `tool-panels` | egui 面板（View 返回 Action）+ dock 布局模型 | `crates/panels/src/lib.rs` |
| `tool-extension` | 插件发现/清单校验/权限/生命周期 | `crates/extension/src/manager.rs` |
| `tool-lua-host` | native Lua 运行时（mlua）与 `ctx.*` API | `crates/lua_host/src/lib.rs` |
| `tool-plugin-api` | 插件契约：`PluginValue` / `LuaEngine` / `PluginHostApi` | `crates/plugin_api/src/lib.rs` |
| `tool-plugin-runtime` | web Lua 运行时（omniLua）与双引擎差分测试 | `crates/plugin_runtime/src/web_lua.rs` |
| `tool-marketplace` | 市场索引、SHA256 校验安装 | `crates/marketplace/src/lib.rs` |
| `tool-updater` | 自动更新（检查/下载/校验/提权 helper/回滚） | `crates/updater/src/lib.rs` |
| `tool-testing` | 测试报告模型 + 场景 DSL | `crates/testing/src/lib.rs` |
| `hardware-workbench-app` | eframe 壳：组合根、dock、快捷键、主题、配置 | `crates/app/src/app/mod.rs` |
| `vendor/egui_tiles` | 打补丁的 dock 布局库（§12） | `vendor/egui_tiles/src/lib.rs` |

依赖方向（粗）：

```text
crates/app ──────────> crates/panels ─────────> crates/application
     │                                              │
     └──────────────────────────────────────────────┤
                                                    ├─> transport（本地/网络/虚拟串口）
                                                    ├─> recorder（录制/回放）
                                                    ├─> extension + lua_host（插件）
                                                    ├─> marketplace（市场安装）
                                                    └─> updater（自动更新）
基础层（被广泛依赖）：core / databus / platform
```

边界规则由测试守卫（改 `Cargo.toml` 之前先看它们）：

- 非 presentation crate 不得依赖 UI/窗口库（egui/eframe/egui_tiles/rfd 等），也不得反向依赖
  presentation 层；presentation = `tool-panels` + `hardware-workbench-app`
  （`crates/application/tests/architecture.rs`）。判定基于**解析后的真实依赖名**，覆盖
  `{ package = ... }` 重命名、`[target.*]` 段与根清单 `alias.workspace = true` 继承的写法。
- `tool-panels` 不得直连领域 crate（transport/recorder/extension/lua_host/marketplace/updater 等），
  必须经 `tool-application` 的 DTO；`hardware-workbench-app` 不得直连 `tool-recorder`/`tool-extension`/`tool-testing`
  （`crates/app/tests/architecture.rs`）。两份守卫 `#[path]` 引同一份判定实现（`crates/app/tests/manifest_deps.rs`），
  修一处漏洞另一处不会仍可绕过。
- 还有一类**源码形状守卫**钉住 wasm 半边（native 测试目标编译不到 `src/web`），例如命令面板
  「使用顺序」两端必须按命令 id 记录、native 端口条目投影只准有一份——见同一测试文件。

## 2. 控制面：AppCommand / 后台任务 / 查询

- UI 动作 → `AppCommand`（`crates/application/src/command/mod.rs`）→
  `command::execute`（唯一匹配）→ 平台 `CommandEffects` 适配 →
  `CommandOutcome::Done | Pending { task_id, .. }`。任务种类用共享 `TaskKind`，两端不再各拼字符串。
- 后台任务由 `TaskManager`（`crates/application/src/task.rs`）执行：`spawn` 并发；
  `spawn_ordered` 按端口键 FIFO——保证同端口的 `Write → SetDtr` 顺序与多段发送顺序。
- 完成经 `AppEvent`（`TaskStateChanged`/`TaskCompleted`/`TaskFailed`/`TaskCancelled`）→
  `Workbench::poll_tasks`（每帧）→ 结果落到各 manager；UI 在下一帧通过查询读取。
- 读侧统一走 `AppRuntime::query_transport / query_recording / query_replay / query_plugins /
  task_snapshots`（`crates/application/src/runtime.rs`）；`TransportView` 只有一份
  （`crates/application/src/transport.rs`），native 填 `statuses/open_ports/config/auto_reconnect`，
  web 填 `ports/connected/connecting/settings/status`。
- 平台契约：UI 侧只依赖 `AppRuntime`（`capabilities / query_* / tick / dispatch`，
  `crates/application/src/runtime.rs`）——native `Workbench` 与 web `WebApplication` 各自实现。

## 3. 数据面：DataBus（唯一事件总线）

- `DataBus`（`crates/databus/src/lib.rs`）：发布时分配单调 id、进有界 history（默认 20000）、
  向匹配订阅者分发 `Arc<Event>`；`TopicFilter` 支持 `All/Exact/Prefix/And/MetadataEq`。
- 订阅模式 = 显式背压策略：

| 模式 | 语义 | 使用方 |
| --- | --- | --- |
| lossless（无界） | 不丢；背压由订阅方自己消化 | recorder（可靠性路径）、web 录制 |
| lossy_bounded（丢新） | 满则丢新并计数 | UI 面板（chart/gauge/attitude 等）、插件、`UiEventSubscriptions` |
| ring_bounded（丢旧） | 环形覆盖最旧 | Terminal / Log |

- 高频路径两个常数（`crates/panels/src/lib.rs`）：每帧最多消化 `MAX_INGEST_PER_FRAME = 500`；
  Terminal/Log 事件缓冲 `MESSAGE_EVENT_BUFFER_CAPACITY = 65536`。
- `RepaintWaker`（`crates/transport/src/lib.rs`）：worker 发布后唤醒 egui 重绘。
- 全仓只有这一个总线；任务/串口命令的一次性完成通道不算第二总线。

## 4. 线程与并发模型

- 每个打开的串口 = 一个 worker 线程 + 有界写队列 + `SerialCommand` FIFO（Write/SetDtr/SetRts 同队列，
  完成回执携带 `io::Error` kind）；`AliveGuard` 保证 panic 时清存活标志，`reap_closing` / `reap_dead_ports`
  回收僵尸。`TransportManager` 刻意无 `Drop`——唯一安全关闭点是 `WorkbenchApp::drop → close_serial`。
- 后台任务跑在 `TaskManager` 的 worker 线程；重活按端口键在 `spawn_ordered` 队列里串行。
- 插件：native 每个插件一个线程 + mlua（§7）；web 单线程协作式。
- 录制 writer 线程（BufWriter 周期 flush）；回放由帧驱动（`tick_replay`），不开独立时钟线程。
- 主线程是 egui：UI 状态只在帧内读写，后台只能通过总线/任务回执交接。
- wasm 没有线程：传输与任务都是异步适配（`crates/platform/src/` 的 web 模块）。

## 5. 传输层

- 三种端口：**本地串口**（serialport；Windows 走 overlapped IO + 可取消唤醒，其他平台 portable worker 循环）、
  **网络串口**（WebSocket + JSON-RPC 2.0 接 Moonraker/Klipper：`server.connection.identify` 握手、
  TX = `printer.gcode.script`、RX = `notify_gcode_response`；无 DTR/RTS；端口名 `host:port`）、
  **虚拟串口**（内存实现，供插件场景测试与 CI）。
- 错误翻译：`translate_error` 产出可操作中文（判定读 kind、文案降级为细节）；`Serial` 臂的残留见
  [known-gaps.md](known-gaps.md)。
- `TransportManager` 本身实现 `tool_platform::TransportBackend`；`Workbench` 不再另持
  `NativeTransportBackend`，同一端口状态只有一个接口。命令执行只能经 `CommandEffects` 的
  `connect/disconnect/send/set_signal`，平台差异留在适配器里。
- `NetworkSerialConfig` 只有一份（`tool_core`），`tool_platform` / `tool_transport` 都再导出；
  平台事件构造器的 source 不再出现 `serial:serial:` 双前缀。
- RX/TX 以 `tool_core::Event` 发布到 `transport.serial.default.rx/tx` 等 topic。
- 设备存活：Windows worker 的空闲等待有 1s 上限，到点调 `probe_alive()`；连续 2 次失败判定
  设备消失，并把 `reason` 写进 `transport.serial.closed`（`device_lost` / `user_close`）。
  `reap_dead_ports` 是「用户关闭 vs 设备失效」的唯一判据，UI 因此能在拔线后立刻变灰，
  而不是等下一次 I/O 失败。
- 自动重连是 app 侧状态机 `ReconnectTracker`（`crates/app/src/reconnect.rs`；纯逻辑，`now`
  由调用方给）：按端口记账、退避 `0.5·2ⁿ` 封顶 8s、最多 10 次；触发点是 `device_lost` 或
  端口从枚举列表消失，到点派发 `AppCommand::Connect`（仍走端口有序队列），取消统一走
  `AppCommand::CancelReconnect`。native 与浏览器共用同一套退避与上限。
  `Workbench::start_reconnect` 只是一次尝试、失败即返回中文原因——原先那个「3 秒预算 +
  10ms 轮询」的循环判据（`Io(WouldBlock)`）没有生产者，已删除。

## 6. 录制与回放

- 录制：JSONL 行格式；`RecordMode::RawSerial | StandardReplay` 过滤（`crates/recorder/src/format.rs`；
  回放来源事件不录）；`JsonlRecorder` 以**无损订阅**接全总线，worker 线程写 BufWriter；
  支持暂停/书签/会话摘要；背压阈值（软 100k 事件 / 256MB / 10s，硬 1M / 512MB / 60s）由 `Workbench::tick` 驱动检查。
- 回放：两步加载（`prepare_load` + `load_prepared`；浏览器用协作式 `ReplayTextLoader`）；
  稀疏时间索引（步长 200）支持 seek；策略 `AutoPreferRecorded | ExactRecorded | ReparseRaw`；
  重放事件在**同一个总线**上发布并打标（`replay=true`、`origin=replay`、source 前缀 `replay:`）。
- recorder 与 replay 刻意零耦合（`crates/recorder/src/lib.rs` 文档）。

## 7. 插件体系

- 契约：`crates/plugin_api`（`PluginValue` / `LuaEngine` / `PluginHostApi` / capability）——
  无 mlua、无 egui、无原生 fs；浏览器异步操作经 pending/completion 队列表达。
- 引擎（双实现）：
  - native `crates/lua_host`：mlua（vendored Lua 5.4）、每插件线程、沙箱（去 `dofile`/`loadfile`/`load`、
    `package.searchers` 只留 preload、10k 指令中断钩子）；`ctx.*` 按权限装配；
    协程任务每 tick 最多 resume 50 个。
  - web `crates/plugin_runtime`：纯 Rust omniLua（指令/内存限制）；模块**刻意不 cfg**，
    由 `cargo test` 在宿主上执行，并与 native 引擎做差分（`crates/plugin_runtime/tests/lua_api_parity.rs`）。
- `ctx.*` 的纯规则（HEX、topic、串口 pattern + 有界正则缓存）与 domain→permission 表在
  `crates/plugin_api/src/ctx.rs`，两引擎共用；没有生产调用者的 `MluaEngine` 与死掉的
  `LuaEngine::resume` 路径已删除。
- 管理：`crates/extension`——扫描 `<dir>/<plugin>/plugin.json`、校验 api_version（当前 `"0.1"`）/
  runtime（`lua`）/权限/contributions、enable/disable 生命周期、事件泵
  （`ui.*`/`log.*`/`plugin.command.*` 恒转发）、诊断中文化。
- UI 集成：`contributes`（commands / ui slots / panels / settings）声明式 + `ui.*` 总线事件
  （动态面板由 `crates/panels/src/dynamic` 消费）；市场安装见 `crates/marketplace`。
- 清单与 API 参考见 [plugins/](plugins/README.md)。

## 8. Native / Web 双组合根

- native：`crates/application/src/workbench.rs` + `crates/app`（eframe 壳）。
- web：`crates/application/src/web.rs` + `crates/app/src/web/`；`send_plan`
  （`crates/application/src/send_plan.rs`）刻意不 cfg，两端共用判定与编码。
- 能力矩阵（`crates/application/src/capability.rs`；面板注册按它门控）：

| capability | native | web |
| --- | --- | --- |
| Serial | ✓ | ✓（Web Serial） |
| RequestPort | ✗ | ✓（仅用户手势） |
| FileSystem | ✓ | ✗ |
| Recorder / Replay | ✓ | ✓（内存录制 / Blob / 浏览器文件选择） |
| Plugins / Marketplace / Updater / NetworkSerial | ✓ | ✓（实现不同，见 [web.md](web.md)） |

- 平台抽象：`crates/platform`（native 端点包 `TransportManager`；web 侧 `web_serial` / `web_network` /
  `web_fetch` / `storage`）。
- 壳两端共用：`crates/app/src/shared_shell.rs` + `vendor/egui_tiles`；面板注册表
  `crates/app/src/panel_registry.rs`（8 个内置面板，按 capability 可用性过滤）。

## 9. 状态与持久化

- `workspace.json`（Windows：`%APPDATA%\HardwareWorkbench\`）：schema_version v1、原子写
  （临时文件 + rename）、旧文件 `.backup`、损坏隔离 `.corrupt-*.backup`（`crates/app/src/config.rs`）。
- eframe persistence 的 app_id 是 `hardware-workbench`，与 `HardwareWorkbench` 是**两个不同目录**；
  安装器卸载范围两者都覆盖（`crates/app/tests/installer_uninstall_scope.rs` 钉住）。
- 归属：`Workbench` 拥有 transport/recorder/replay/plugins；`WorkbenchApp` 持 UI 状态与配置；
  `PanelManager`/`TilesLayout` 持久化 dock 布局与区域记忆（`crates/panels/src/manager.rs`）；
  panels 持各自的订阅与渲染游标。
- 运行时资源从 exe 旁 `assets/` 读；用户插件在 `<数据目录>/plugins`
  （Linux：`~/.local/share/HardwareWorkbench/`）。

## 10. 如何新增能力 / 面板

- **新增能力（命令）**：`AppCommand` 加变体 → 在 `command::execute` 里写唯一决策 →
  平台差异实现为 `CommandEffects` 方法（native `Workbench` / web `WebCommandEffects`）→
  需要异步结果就加 `AppEvent` 分支并在 `poll_tasks` 落账 → 读侧加 `AppRuntime::query_*` DTO →
  app 层 handler（native `src/ui` / `src/runtime`，web `src/web`）接线 → 浏览器需要等价物时更新
  `capability.rs` 与 `web.rs`。
- **新增内置面板**：在 `crates/panels` 实现 View（返回 Action，不依赖 Workbench）→
  定义 `PanelId` / `PANEL_*` 常量（`crates/panels/src/manager.rs`）→ 在
  `crates/app/src/panel_registry.rs` 注册 `PanelDef`（可挂 capability 门控）→
  共享面板加进 `shared_shell::SharedPanels`（由 `render_panel` 统一分发），平台专属面板
  实现 `DockHost::render_platform_panel`。动态面板走插件路径，不进注册表。

## 11. 测试与验证策略

- 门槛：`cargo fmt --all --check`；`cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo test --workspace --all-targets`；压力门禁见 [performance.md](performance.md)；
  wasm 侧两条（`check` + `clippy`，排除 5 个 native-only crate，见 `.github/workflows/ci.yml`）。
- 用例类型：
  - **架构**：依赖边界 + 源码形状（§1）。
  - **契约**：`crates/application/tests/headless.rs`（send routing / replay / 插件启停 / 事件发布）、
    安装器卸载范围、命令面板键空间等。
  - **差分**：同一份 Lua 脚本喂 mlua 与 omniLua 两引擎（`crates/plugin_runtime/tests/lua_api_parity.rs`）。
  - **场景**：虚拟串口 + 真实插件运行时（`crates/extension` 的 scenario runner；模板
    `plugins/template.serial-chart`）。
  - **豁免清单**：`crates/app/tests/lint_exemptions.rs`（见 [known-gaps.md](known-gaps.md)）。
- 两类判据纪律（都是踩过的坑）：
  - **先证明断言会红再交付**——放松一个判定，用例必须红在预期的那条上，不许留下恒绿判据。
  - **不要把「等待 N 毫秒后断言」当同步手段**——跨线程派发的实测最坏延迟可达数秒，
    这类用例会随机红且不携带信息（实例与关闭代价见 known-gaps 的 `ExpectNoTx` 条目）。

## 12. vendored 依赖：egui_tiles

- 基线：crates.io `egui_tiles 0.16.0`，以 fork 拷贝进 `vendor/egui_tiles`（2026-07-31，`cd3196c`），
  workspace 用 `[patch.crates-io]` 指过来。
- 导入后叠加的**本地补丁**（升级上游时逐条重打）：
  1. `b68adb8`（v1.1.1）：新增 `Behavior::is_container_kind_allowed` 钩子，按容器种类过滤**新提议**的
     投放目标（不动已存在的容器结构）。
  2. `ad55450`：抽出 `DROP_ZONE_THICKNESS`（12px，`container/linear.rs` 与 `lib.rs` 共用同一常数，
     防止两条投放路径分叉）；垂直拆分目标从"半块面板"改成边缘条，避免与标签组目标重叠误触。
  3. `91901ae`：拖拽经过标签栏不再切换该组的激活页（上游行为会把途经的每个标签组翻页并被持久化）；
     拖拽结束（松手或 Esc）清理平滑预览矩形，避免下次拖动从残留位置插值；投放候选加距离上限
     （`MAX_DROP_DISTANCE`，远离任何候选显式放弃，而不是吸附到最近位置）。
- 与上游的完整 diff 需要拿 crates.io 0.16.0 源码对照 `vendor/`；仓库内以提交史（`git log -- vendor/egui_tiles`）
  作为补丁清单。升级上游后逐条重打，并跑全工作区测试（dock 布局用例分布在 `crates/panels` 与 `crates/app`）。
