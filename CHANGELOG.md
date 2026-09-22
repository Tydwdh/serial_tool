# Changelog

本项目的重要变更记录于此。版本号遵循语义化版本。

## [Unreleased]

### 调整

- 拆掉工作区里最长的四个函数（纯提取，不改行为）：终端 `render_rows_view` 760 → 183 行、日志
  `render_log_rows` 664 → 348 行、native `Workbench::dispatch` 353 → 158 行、wasm
  `WebApplication::dispatch` 531 → 174 行，合计新增 74 个同文件私有函数（无新 trait/泛型/宏）。
  **等价性不是靠"测试全绿"说的**：终端与日志两个面板在 HEAD 与改动后各渲染同一批事件，
  把一帧的 shapes（含每条文本 galley 的内容、图层与裁剪矩形）逐行导出比对，SHA256 相同。
  该覆盖的局限也写清楚：这一组夹具只走"正常渲染"路径，行选中、悬停、右键菜单与字符拖选
  没有被比对。
  **本条的等价性后来按正确方法重测过一次**（下一批的记录里写了为什么需要重测：两棵树共用同一个
  target 目录时，worktree A/B 会静默退化成"同一个二进制跟自己比"）。重测基线取 v1.3.0 的发布提交
  `ad2091d`，两边各渲染终端 6 条 + 日志 7 条事件，两个面板的 shapes 摘要 SHA256 仍然逐字节相同
  （`63a0145d…`），所以这条结论没有被推翻。
- 浏览器端 composition root `crates/app/src/web.rs`（6029 行，全仓最大文件）拆成 `crates/app/src/web/`
  目录：`plugins.rs`（1280 行，插件安装/启停/Lua 宿主挂起请求/贡献点）与 `replay.rs`（538 行，
  JSONL 选取与 Web Replay Analyzer 分批调度）先搬出去，`web/mod.rs` 剩 4274 行。搬运保真是量过的：
  与拆分前整体比对，数字字面量 142 条完全一致、注释零丢失，唯一的文本变化是两处 `include_bytes!`
  的相对路径随层级加深（`../../../assets` → `../../../../assets`）。18 个跨模块项的可见性从私有
  提到 `pub(crate)`（这是本仓允许的唯一放宽，逐项列在提交信息里）。
- 第三批长函数：桌面端 Lua `process_tasks` 265 → 20、`create_task_api` 205 → 15、`create_ui_api`
  256 → 21（照 `serial.rs` 的 `register_*` 风格，Lua 侧键名与注册顺序逐条核对不变）；
  共享串口面板 `grouped_port_list_ui` 208 → 75；录制器 `start` 201 → 95（"先建文件、后订阅"的
  副作用顺序未变）。
- 摘掉 3 条已经失效的 lint 豁免（方法：把全仓 13 条一次摘光，让 clippy 逐轮点名哪些真的需要，
  只恢复点名的那些）。其中两条 `too_many_arguments` 的注释写着"故保留本豁免"，但函数现在
  恰好 7 个参数、在阈值之下，豁免早已空转 —— 注释改成说明这一点，"不打包上下文结构体"的理由保留。
- 第四批长函数与最后一个大文件（纯提取/搬移，不改行为）：日志面板 `render_log_rows` 348 → 103 行、
  同文件 `ui` 193 → 55 行（新增 21 个私有函数与 3 个私有结构体 `LogRowFrame`/`LogRowsInteraction`/
  `LogRenderedRow`，每个都被 ≥3 个函数共用，不是只为一个调用点传参）；插件面板 `plugin_row`
  191 → 42 行；桌面端 `apply_theme` 191 → 拆到最长 50 行；浏览器端的串口面板整域（`serial_panel_ui`
  177 行及其状态）搬进 `crates/app/src/web/serial.rs`（695 行），`web/mod.rs` 4274 → 3620 行，
  全仓最大文件由此从 4274 降到 3620（次大是 `terminal.rs` 3606）。
  **等价性证据与它的两次返工**：日志面板在 HEAD 的独立 worktree 与本树各渲染同一批 7 条事件
  （info/warn/error 三个级别、来源与消息都有悬殊宽窄的对照），把一帧 shapes（含每条文本 galley 的
  内容、图层、裁剪矩形）逐行导出比对，SHA256 一致。第一版这个比对是**假的**：worktree 与本树的
  单元指纹按**相对路径**计算，两边共用同一个 target 目录时命中同一份产物，第二遍"没有 Compiling
  任何 crate"却报告两边一致 —— 改成各自独立 target 目录、并核对编译日志里的 crate 路径之后才算真做。
  敏感性另外单独验过：只把列宽基准字号 `13.0` 改成 `12.0`，摘要立刻变（`40ce981c…`），改回又回到
  原值（`0ebb5f10…`），所以"一致"不是因为比对什么都不敏感。**未覆盖**：选中行、悬停、右键菜单与
  字符拖选这几条路径没有进夹具，它们的行为靠的是字面量审计与既有单测。
  字面量审计：`log.rs` 的 137 条字符串、173 条数字字面量多重集与 HEAD 完全一致，注释零丢失
  （只把 `//` 升为 `///`）；`web/serial.rs` 的整域搬移字面量增减均为 0；`bootstrap.rs` 与
  `plugins.rs` 除同样的 `//`→`///` 外各新写了 2~12 条解释性注释，一条注释都没有丢。
  ARCHITECTURE.md 里 5 处指向 `web/mod.rs` 的站点已重测：4 处改为按符号定位（行号每轮拆分都会漂），
  1 处（`send_web_current` 及其预检行）更新为新行号。
- 换行后缀表从 **3 份收成 1 份**：`tool_core::LineEnding` 持有 `ALL`/`label()`/`suffix()`，native
  （`crate::state::LineEnding`）、wasm（`WebLineEnding`）与共享面板（`tool_panels::SendLineEnding`）
  三处只留别名，面板对外名字仍然可导入（`pub use`）。**serde 形状一字未动** —— 桌面
  `workspace.json` 与浏览器设置里存的仍是裸变体名 `"None"/"Lf"/"Cr"/"Crlf"`，这一点由新用例
  `line_ending_persisted_names_are_bare_variant_strings` 按字面量钉住，并且**实测它会红**：给合并后的
  枚举加 `#[serde(rename_all = "lowercase")]` 立刻打红 3 条，去掉即回绿。顺带删掉四处只为"两个类型
  不同"而写的 4 臂 identity `match`（两端各一份 `line_ending ↔ SendLineEnding` 的来回转换），
  全仓净减 14 行、少两份表。发布给 Lua 插件的 `send.line_ending = {label, suffix}` 快照
  （`runtime/plugin.rs`、`ui/contributions.rs`、`web/plugins.rs`）字符串逐字未变。
- 桌面端 composition root `WorkbenchApp::new` 232 → 84 行（全仓最长的生产函数由此让位给
  浏览器端的 `poll_web_events` 215 行）：按原执行顺序拆出 9 个同文件私有函数
  （`startup_config`、`ensure_theme_dir`、`activate_startup_theme`、`startup_panel_manager`、
  `startup_ui_state`、`startup_workbench` + `application_config_from`、
  `apply_persisted_panel_settings`、`rewrite_config_if_migrated`），没重排任何副作用 ——
  主题与字体仍在前两条，往 bus 发配置损坏日志的三条分支仍在 `bus` 建好之后，
  回填面板设置 → `refresh_ports()` → 迁移回写配置的顺序原样保留。
  **启动顺序这件事以前没有任何东西守**（`new` 要 `eframe::CreationContext`，无头环境构造不出来），
  本波补了 `crates/app/tests/startup_order.rs`：把 `new` 体内 14 个步骤的先后与"各出现一次"按字面钉住，
  另加一条防空转的断言（步骤被整段搬出 `new` 时也会红）。实测它抓得住：把
  `ensure_theme_dir()` 提前到 `startup_config()` 之前，用例立刻报
  「启动步骤次序被改」，还原后回绿。字面量审计：`app/mod.rs` 的 16 条字符串、9 条数字字面量
  与 HEAD 多重集一致，两条注释只是从 `//` 升为 `///`。
- 第五批长函数：浏览器端每帧事件泵 `poll_web_events` 215 → 8 行（剩下的就是"克隆 runtime、
  排空事件、逐个交出去"），函数体逐字搬进 `handle_web_event`，再按 match 分支顺序拆出
  `finish_web_recording_export`、`dispatch_web_replay_load`、`apply_web_connected`、
  `apply_web_disconnected`、`apply_web_signals`、`take_web_lua_resolutions` 六个同文件私有函数。
  **两处刻意没拆**：30 个臂的 `match` 保持完整（`TextLoaded`/`FilesLoaded` 靠"带守卫的臂在前、
  兜底臂在后"的顺序，拆成嵌套子匹配可能改变命中的臂），以及 `handle_web_event` 末尾那五步
  （五个延迟临时量 + `drop(serial)` + 五步收尾）—— 拆它需要一个只被两三个函数用的五字段包，
  而 `drop` 与紧随其后的借用是防 panic 的相邻关系。浏览器端最长的函数由 215 降到 165 行；
  全仓最长的生产函数易主为 `crates/panels/src/sender.rs` 的 `history_popup`（200 行）。
  字面量审计：`web/mod.rs` 的 260 条字符串、95 条数字字面量、102 条注释多重集与 HEAD 一致，
  零丢失、零新增字面量，另新增 47 条解释性注释。
- 两处"参数墙"收拢，**退掉 2 条 lint 豁免**（全仓 `#[allow(clippy::…)]` 由 9 条降到 7 条）：
  `render_rows_view` 20 → 7 个参数、`render_log_rows` 14 → 6 个。每帧的只读输入收进
  `RowsViewInputs<'a>`（13 个命名字段，4 个消费者）与 `LogRowsInputs`（8 个字段，3 个消费者），
  三个可变借用（`selection` / `text_selection_rows` / `virtual_rows`）仍作独立参数。
  **真正的收获不是参数个数，而是调用点从位置变成名字**：此前 `stick_to_bottom` 与
  `force_scroll_to_bottom`、`show_hex` 与 `show_raw` 这些相邻 bool 在调用点上无法区分，
  换错两个编译不报错、渲染却变；现在这种错误在写法上就表达不出来。
  **等价性证据**：HEAD（`c279bb2`）的独立 worktree 与本树各渲染同一批事件，
  终端 / 日志各自的**空态与有数据态**四段一帧 shapes 共 176 个（含每条 galley 文本、图层、
  裁剪矩形）逐行比对，SHA256 一致（`619de6e0…`）。摘要夹具这一轮专门补了空态一路 ——
  原来的夹具永远有行，而本轮改动同时动到了两个面板的"无数据"分支，不补就会比出一条死分支。
  敏感性另外验：把 `measure_rows` 的 `text_padding` 由 `4.0` 改成 `5.0`，摘要立刻变
  （`0ed60754…`），改回即回到原值。
  **探针同时暴露了一个比对盲区（如实记下）**：把空态的高度下限 `max(40.0)` 改成 `41.0`，
  摘要**不变**（确认过是重新编译过的，不是旧二进制）。也就是说 A/B 证明不了这一行；
  它靠的是逐行读用法 —— 老版里 `height` 只被空态分支用掉（非空路径用的是 `total_height`
  与 `full_width`），所以把 clamp 移进 `render_empty_rows_view` 等价。
  字面量审计：`terminal.rs` 337 条字符串 / 376 条数字、`log.rs` 137 / 173 与 HEAD 多重集一致，
  零丢失、零新增，各多 6 条解释性注释；没有只换比较运算符的行。
- 更正一条已经与代码不符的架构结论（文档，不改行为）：`docs/ARCHITECTURE.md` 第 12 条「仍留 ③」
  原写 `TransportCapabilities` "两侧都不被读取"。实测共享串口面板读它 7 处、覆盖 5 个能力位；
  真实的不对称是 native 把交给面板的那份能力写成字面量，而后端上报的 `capabilities()` 在 native
  不参与 UI。同轮顺手核过的另两条里，`webserial:session:N` 仍被生成这条确认成立；
  而"web 仍显示英文 `TransportError` 文案"那条**在同一轮晚些时候也被推翻**（归因错了：
  桌面端在同一批失败上同样是英文），见「修复」小节与下一条。改后的表述一律写成可复核的
  grep 形式。
- 新增一条**豁免清单守卫** `crates/app/tests/lint_exemptions.rs`：扫描 `crates/**/*.rs`，
  把每条 `#[allow(...)]` / `#[cfg_attr(..., allow(...))]`（clippy 与 `dead_code`）折成
  「文件 + lint → 条数」与清单逐字比对。多一条红（要求改动者显式承认并写理由），
  少一条也红（提醒把清单收紧）。此前"还剩几条豁免"只能靠人记，第 6 轮一次摘光才发现
  3 条早已空转；这条守卫把这件事变成机器断言。它**第一次运行就抓出了我自己数的清单**：
  我用一条只匹配 `clippy::` 的 grep 得出"7 条"，实际是 **16 条**（漏掉 9 条写在
  `cfg_attr` 里的 `dead_code`，对应 Linux / wasm 死代码那张表），断言直接把差集打在失败输出里。
  清单按**次数**比而不是按 (文件, lint) 去重 —— 否则在已登记文件里再加一条会溜过去；
  这一点也单独探过（往 `gauge.rs` 再加一条 `dead_code` → 红，报"多了 1 条"）。
- 事件 → `PluginValue` 的转换从两份收成一份：`crates/app/src/web/plugins.rs` 的
  `web_event_to_plugin_value` 与 `crates/app/src/web_plugin_host.rs` 的 `event_to_plugin_value`
  是同一个 33 行函数的两份实现，逐行 diff **只差签名那一行**。两份分别喂插件事件派发与
  回放分析器，于是"新增一种 `Payload` 或一个事件字段"时只改一份就有一条静默漏掉它，
  而且两条路径不会同时出问题。保留宿主侧那份并设为 `pub(crate)`，删掉副本与别名
  （不留"一个东西两个名字"，那是下一次复制的起点）。
  **判据是 wasm 侧**：这两个文件 native 根本不编译，所以用
  `cargo check -p hardware-workbench-app --target wasm32-unknown-unknown`（exit 0、零告警）。
- 终端与日志各有一份**逐字节相同**的"自动跟随时把滚动钉到末尾"助手
  （`scroll_rows_to_bottom_if_requested` / `scroll_log_rows_to_bottom_if_requested`，
  只差函数名），收进 `design::scroll_to_bottom_if_requested` 一份。
  可见性只到 `pub(crate)`（两个调用点都在 `tool-panels` 内），不扩公开面。
  这两份分叉的症状是"一个面板跟随、另一个不跟随"，而两处代码本来就没理由不同。

### 修复

- 修复重连循环里一个**从来没生效过**的判定：`workbench.rs` 用
  `network.is_some() && error.to_ascii_lowercase().contains("would block")` 决定"要不要再试一次"。
  换成读类型化错误的 `kind`（`reconnect_is_worth_retrying`，只认 `Io(ErrorKind::WouldBlock)`），
  翻译只发生在交给用户那一处。**这个分支此前恒假**，两个方向都假：
  `io::Error::new(kind, msg)` 的 `Display` 只打印 `msg`、不含 kind 名，而全仓唯一造出
  `WouldBlock` 的 `TransportManager::open_serial` 给的 msg 本身就是中文「… 正在关闭中，请稍后重试」；
  再加上 `network.is_some()` 把它限定在网络端口，而 `open_network_serial` 没有这条错误路径。
  **必须说清的限度（不要读成"重连现在会重试了"）**：改成 kind 判定之后，
  **没有任何一种失败从"立即放弃"变成"重试"** —— 两个分支都没有 `WouldBlock` 生产者：
  串口那条的 kind 在 `platform/src/native.rs::operation_error` 就已折成字符串（要修得动
  `tool-platform`，超出本轮范围），网络那条 `open_network_serial` 在 TCP/WS 连接失败时仍返回 `Ok`
  （失败由 worker 线程以日志事件发布）。本轮净效果：判定从"看起来在判、其实恒假"变成
  "语义正确、当前无人触发"，并有一条注释把这件事写死。
  **用户可见的变化只有文案**：串口重连硬失败不再带英文 `transport operation failed:` 框架，
  网络重连失败从 `io error: …` 变成 `操作超时：…`（与 `web.rs` 同一边界已有的翻译一致）。
  守卫：`workbench.rs` 的 `mod tests` 钉住 kind→重试的对应关系（`WouldBlock` 才重试；
  **文字长得像"正在关闭中"的硬失败不重试**），`headless.rs` 钉住公开面上真走得通的三支
  （硬失败立刻放弃、失败文案是中文、网络端口确实走网络分支）。子代理报了 6 次变异，
  其中一条**诚实记为没抓到**：把成功返回值 `Reconnected` 换成 `TransportConnected` 时三条用例全绿，
  于是它改断言为"成功必须携带 `poll_tasks` 会应用的结果"（该变异随即变红）。我另外在 16 路
  CPU 负载下把那条依赖墙钟的用例连跑 8 次全部通过（0.01–0.11s），不是运气过的计时测试。
  两个待你拍板的问题记在 `docs/ARCHITECTURE.md` 第 12 条 ④。
- 修复**连接/断开/发送失败时用户读到英文句子**（两端都是，此前只记在网页端名下）。根因不是
  平台差异，而是一个调用点把强类型错误提前压成了字符串：`platform/src/native.rs` 的
  `operation_error` 用 `error.to_string()` 折进 `TransportError::Operation(..)`，变体信息就此丢失；
  全仓唯一的中文映射 `tool_transport::translate_error` 只剩周期发送那一行能享受。
  现在：`tool_platform` 有了自己的 `translate_error`（6 个变体，native 与 wasm 都编译），
  web 侧接在 `application/src/web.rs` 的 `spawn` 缝上（赶在变体丢失之前，一次覆盖
  `snapshot.message` 与 `TaskFailed` 两个出口），`operation_error` 改为先翻译再嵌入，
  并去掉 `start_receive_with_disconnect` 的两处双重 `Operation(..)` 包装 ——
  **用户可见的措辞变化**：以前是 `transport operation failed: port is not connected: COM3`
  （网页端还会看到两句连在一起的 `…operation failed: …`），现在是
  「串口 COM3 未打开（可能已断开，请重连）」，两端同一句话。
  守卫是 `crates/platform/tests/transport_error_wording.rs`：每个变体一条精确字符串断言、
  一条"没有臂落到通用兜底"的覆盖断言、以及一条**跨两份映射的一致性断言**
  （同一件事两边必须逐字相同）。实测它抓得住：只把 `PortNotConnected` 的字面量改成
  「串口 … 没打开」，3 条用例变红并打印左右原文。
  **仍然英文**（本波未动，逐条核过并在 ARCHITECTURE 里记名）：`application/src/workbench.rs`
  网络端口路径的三处 `error.to_string()`、`platform/src/native.rs` 的
  `"native transport worker stopped"` 字面量；`transport: ` 前缀仍只有 native 有
  （`AppError` 被 cfg 挡在 wasm 之外）。
- 修复测试随机红：主题调色板是**进程级全局**（`ACTIVE_THEME`），而 cargo 在同一个测试二进制里
  默认多线程跑用例，于是"切换主题后断言取色"的用例与"先解析颜色、之后才和 `theme::*()` 比对"的
  用例（仪表盘的 zone 颜色、`value_color`）会互相踩。实测同一个测试二进制以 16 线程连跑 40 次：
  加串行锁前红 **6** 次，加锁后 **0** 次；空闲单跑很难复现（15 次 0 次），所以以前它只在 CI
  机器紧张时露一次脸——上一次发布就出现过"同一批代码先红后绿"。现在碰主题状态的用例共用一把
  测试专用锁，锁的注释里写着这组数字与失效模式。

## [1.3.0] - 2026-09-22

### 新增

- 浏览器 Lua VM 改为全平台编译，新增 14 个契约测试在宿主平台上直接运行同一份实现：共享 `LuaEngine` 协议、沙箱（`io`/`os.execute`/`dofile` 拒绝）、权限门禁、`ctx.bus`/`ctx.session` 的宿主请求映射、指令上限，以及直接执行仓库内 `template.serial-chart` 回放插件并校验派生事件。
- `tool-platform` 能力层测试：网络端口身份前缀、序列化契约、`TransportCapabilities` 信号位、RX/TX 事件形状，以及 native 存储的覆盖写入、幂等删除和路径逃逸拒绝。

### 调整

- 收敛插件市场的依赖边界：插件面板不再直接依赖市场与更新栈，市场索引改由应用层持有，界面只消费只读视图。
- 补充无界面契约测试：串口调试核心的发送路由、终端上限、回放生命周期、插件启停与事件发布五类行为，用例由 2 条扩到 7 条；并修复架构测试在清单文件读取失败时会静默通过的问题。
- CI 的 Windows 作业改为对整个工作区执行 clippy 与测试（此前受默认成员设置限制只覆盖主程序），使契约测试与其他 crate 的测试都纳入检查。
- 自动更新改为只认发布时**外部固定**的 SHA256：`update.json` 必须携带 `sha256`，缺失或不合法时直接拒绝并报错，不再"下载完之后自己算一遍再跟自己比"（那种自校验在更新包被替换时不起作用）。**由此产生一处需要知道的退化**：如果清单里没有这个字段（例如 1.2.0 发布时生成的那一份 `update.json`），桌面端点「检查更新」会得到一条以「解析更新信息失败：」开头的报错。这是刻意"宁可失败也不装无法校验的包"，不代表安装损坏，无需重装或清理配置。影响范围是**用本分支自编译的版本、或清单尚未带上该字段的那段时间**；发布流水线现在会在生成 `update.json` 时写入这个摘要，因此安装下一个正式发布的用户不会碰到它。浏览器版不读这一项，不受影响。
- 非法 HEX 输入的提示统一成一句中文：桌面端原来显示 `transport: invalid hex input: <原因>`，现在显示 `transport: HEX 解析失败：<原因>`，桌面端**哪些输入合法没有变化**。浏览器端除措辞之外，判定边界也向桌面端对齐了：`0x0xAB` 这类重复 `0x` 前缀、`0A,BB` 这类逗号/分号分隔的写法，现在两端一致（此前浏览器端有的路径收、有的路径拒）。
- Lua 插件的运行环境收回了三个可直接读盘的全局函数：`dofile`、`loadfile`、`load` 现在在插件里为 `nil`，`require` 也不再按 `package.path` 去加载磁盘上的 `.lua`（只能用宿主预置的模块，如 `require("hw.codec")`，这部分不变）。插件 main.lua 自身与它被允许的模块照常加载；此前靠 `dofile` 互引的插件写法需要改为 `require` 预置模块。
- CI 的 `test` job 增加压力门禁（`tool-panels`/`tool-databus` 的结构性断言），`crates/app` 中依赖安静调度的计时测试继续保持 `#[ignore]`。
- 文档明确 macOS 不受支持（无专用代码、CI 与发布包），`RELEASE.md` 的 crate 结构说明同步到当前 15 个 crate。
- 删除 `crates/app/src/ui/bottom_panel.rs` 中已被共享实现取代的旧发送器路径：`legacy_send_panel_body` 及其 12 个辅助函数、旧历史弹窗与重复的本地 `SendLayout` 枚举（本树实测该文件 1467 → 242 行），并为其补上针对共享 `sender_ui` 的布局测试。
- 设备响应匹配统一：`ctx.serial.expect`、`expect_from` 与 `request` 的 `expect` 选项现在走 `write_line_and_expect` 的 patterns 那一套三形态规则（`re:<regex>`、`^行首锚定`、无前缀子串）。**行为变化**：这三条路径此前是纯子串比较，`expect("^ok")` 会去字面量找 `^ok` 这三个字符、几乎必然超时；现在它表示行首锚定（并同样先跳过行首可选的 `(数字)` 时间戳前缀）。**不含 `re:`/`^` 前缀的普通子串模式行为一字未变**。两端也由此对齐：浏览器侧本来就只有一条共享扫描，此前是桌面侧的异类。
- 「恢复默认布局」与「恢复所有默认设置」改为**两步确认**，四个站点（桌面端、浏览器端各两条）统一走日志/终端「清空」那同一份确认规则：第一次点击只把标签换成确认态（等宽），3 秒内再点一次才真正执行。此前它们都是单击即执行，其中「恢复所有默认设置」会清掉串口参数、端口别名与分组、布局、数据上限、字体和主题，且没有任何撤销入口。桌面端布局行原来那个独立的「取消」按钮不再出现 —— 取消由 3 秒超时、Esc 或点击别处承担（与「清空」一致，也避免窄面板里多一个按钮把整组控件挤到下一行）。
- 浏览器端命令面板的搜索现在与桌面端一样支持 `re:` 正则，非法写法的「已按字面量搜索」悬停提示也补齐（此前是固定的小写子串匹配）；恢复布局/恢复默认这类成功动作在浏览器端不再静默，通知文案与桌面端逐字相同。
- 搜索框的 `re:` 提示补齐到每个入口（插件市场、命令面板此前没有；终端/日志/表格一直有），并且**非法正则回退现在会直接说明**：`re:` 后面写法不合法时按字面量搜索，鼠标悬停会看到"正则表达式非法，已按字面量搜索"，不再只记一条用户看不见的 warning。**发送器的历史过滤框**在同一版里也接进了这套规则（见下方「历史」弹层那条），至此搜索入口不再有例外。

### 修复

- 修复 CI 在最低支持的 Rust 1.92.0 上 clippy 报错导致作业失败的问题（两个 lint 触发点在 1.98 不报）；Windows 作业与 Web 目标作业现已恢复通过，且未使用任何 lint 豁免。
- 修复 Dock 拖拽时上下拆分**只对左侧生效**：1.1.1 关闭上下拆分用的是投放建议层的开关，它管不到已经存在的垂直容器（默认布局的左侧那一列本来就是上下结构），于是左侧仍有「下方拆分」、右侧日志和终端完全拿不到。现在上下投放区统一收窄成贴着面板上下边缘的细条（与线性容器自己的投放条同一定宽），两侧一致；要向下拆分某个面板，把标签拖到它上/下边缘附近即可，面板中部仍然只用于合并标签组，防误触的初衷不变。
- 修复浏览器 Lua VM 中 `ctx.session.get(key, default)` 的默认值不生效：宿主对未写入的键返回 `null` 时也会回退到默认值，而不只在宿主报错时回退。
- 修复卸载不干净。安装器此前漏了三处：eframe 的窗口/egui memory 持久化目录 `%APPDATA%\hardware-workbench\`（目录名来自 `main.rs` 的 `with_app_id`，**与它一直在删的 `HardwareWorkbench` 是两个不同目录**，任何用户首启即产生）；被隔离的损坏配置副本 `*.corrupt-*.backup`（旧清单只有精确文件名与 `*.tmp`，匹配不到，留一份就让目录非空、`dirifempty` 失效）；以及更新器写在安装目录里的 `*.exe.bak`、`.hw_update_probe_*` 和 `copy_updated_resources` 从更新包复制进来的 `assets`/`docs`/`licenses`/`examples` —— Inno 只删它记录过的文件、且只在目录已空时才移除目录，所以**只要自动更新过一次，整个安装目录就会留在 `%LOCALAPPDATA%\Programs\HardwareWorkbench`**。现在 `[UninstallDelete]` 补齐并加了末尾兜底；`crates/app/tests/installer_uninstall_scope.rs` 把清单与代码钉在一起（改了 `with_app_id` 而忘同步 `.iss` 会直接变红，两种失配都实测过）。Ubuntu 侧行为未变，但已在 `docs/INSTALLER.md` 写清：`.deb` 不含 `prerm`/`postrm`/`conffiles`，`dpkg -r` 与 `-P` 都不删用户数据，是否跟随 purge 惯例另作决定。**随后在真机上跑完整周期（装→启动一次→伪造更新器残留→卸载）又抓出两处读代码没发现的遗留**：早期版本把 `themes\` 与 `workspace-iced.json(+.backup)` 直接写在 `%APPDATA%\HardwareWorkbench` 下，卸载后那里仍剩 12 个文件、目录非空于是 `dirifempty` 不生效；补上这三条后四个位置（安装目录、两个漫游目录、HKCU 卸载项）实测全部消失。
- 修复终端与日志面板「清空」的二次确认在窄面板下把工具栏挤成两行：确认态只把标签换成等宽的「确认」并在悬停提示里说明，不再插入额外的「取消」按钮；取消改由 3 秒超时、Esc 或点击别处承担。
- 修复发送器按钮行「宽面板横向滚动、窄面板换行」两套机制并存导致插件按钮（G文件、暂停/恢复、取消等）被裁到视野外：三处行容器统一为换行，操作区预留高度改为跟随实测高度，换行后最后一行仍在面板内。
- 修复 `egui::ComboBox` 在换行行里不参与换行的问题：剩余宽度不足时它会向右溢出，并撑大所属 Ui 的换行边界，使同一面板后续所有行也不再换行。新增 `design::combo_slot` 按已知宽度占位，用于发送器的端口/换行符下拉与终端、日志的过滤下拉。
- 修复桌面端**周期发送**绕开发送判定：它此前走 `tool_transport::send_impl_to` 这份独立实现，于是同一段输入「点一次发送」和「开着周期发送」结果不同 —— 多行 HEX 被按行拆成一帧一行（点发送是完整的一帧，对端固件看到的帧边界因此不同）；纯空白文本被静默当成发送成功（什么都不投，状态却显示正常）；非法 HEX 报的是另一套措辞「无效HEX：…」而不是「HEX 解析失败：…」。现在周期发送与四个手动发送站点共用同一份命令构造与解码，那份独立实现随之删除。**需要知道的行为变化只有一处**：多行 HEX 的周期发送现在每周期投**一帧**（与点一次发送一致），不再每行一帧。
- 修复浏览器端 `Ctrl+Enter` 发送是死键位：键位处理器把发送命令拦下来直接返回，注释说"面板会处理"，而共享面板里根本没有发送键位的处理，只有那句「Ctrl+Enter 发送」的提示。现在浏览器端与桌面端一样，按 `Ctrl+Enter` 走的是「发送」按钮那条完全相同的路径（含输入校验与换行后缀）。
- 修复浏览器端**终端与日志里的中文变成豆腐块**（`□□□`）：网页端注册字体时只给比例字体族加了中文回退，等宽族里只有 JetBrains Mono —— 而终端/日志正是按等宽渲染的，缺字形就只剩占位框。桌面端一直是 `["jetbrains", "zh", …]`，现在两端同一份族序。**需要知道的限制**：这条改动我没能用眼睛确认（内置浏览器面板不可见，截图取不到），依据是两端字体族列表的逐项比对。
- 修复发送器「历史」弹层的排版：v1.2.0 删掉旧发送器子树时，旧弹层那套设计（标题 + 条数、占满宽度的过滤框、定高行 + 超宽丢弃、右侧固定删除列、行间细线、两步「清空全部」）没有跟着搬进共享实现，剩下的是一版"每条历史画一个按钮"的粗糙弹层 —— 按钮宽度等于内容宽度，一条长中文历史能把弹层拉到窗口那么宽，行与行之间全是空白格。现在共享实现恢复成旧排版，两端（桌面 + 浏览器）一起生效；顺带把过滤框接进共用的 `SearchQuery`，那里从此支持 `re:`（此前是小写子串），悬停提示与其它搜索入口一致。

### Dock 拖拽

- 修复无法用拖拽还原默认布局：上下拆分的落点从"整个上半/下半"收窄为目标区域的**上/下边缘窄带**。整半边会让面板中部也命中、一拖就误拆，v1.1.1 因此把上下拆分整个禁用，代价是"主区在上、发送器在下"这种默认结构再也拖不出来；现在贴边拖即可拆分，中部照旧是"合并为标签组"。
- 修复拖拽产生的嵌套容器在标签栏显示枚举 Debug 名（`Horizontal`/`Vertical`）：改为「左右拆分」「上下拆分」「标签组」「网格」。
- 修复把底部（或右侧）区域的最后一个面板拖走后，该区域的开关变成空操作：区域容器被 `prune_empty_tabs` 剪掉后 `bottom_tabs`/`right_tabs` 悬空，`set_bottom_visible(true)` 静默无效，设置页勾选也会立刻弹回。现在 `TilesLayout` 记录区域成员（`bottom_panes`/`right_panes`，`show_dock` 每帧刷新），再次要求显示时按记录重建容器并把原面板搬回去；重建出的底部区域挂在主标签区所在列上，与默认布局同构。`main_tabs` 本身悬空时会先用 `ensure_main_tabs` 修好，不会把不存在的 tile 塞进树里。
- 布局的根容器若已损坏（配置被写坏或容器被剪空），`ensure_tiles_layout` 直接回退到默认布局，不再渲染出空白 Dock。
- 修复拖拽经过标签栏会切换该组当前显示的面板：上游在光标落在标签上时改写 `active`（注释为 "Expand this tab"），但那是最终状态而非预览，拖动横穿窗口会让途经的每个标签组翻页并被持久化。落点判定不依赖 `active`，"把面板丢进标签组"不受影响。
- 修复在窗口空白处（顶栏、状态栏、预留边距）松手也会移动面板：上游用"最近候选"选择落点且没有距离上限，现在要求光标距离候选区域不超过 16px，远离任何候选即等价于放弃移动。
- 修复按 Esc 取消拖拽后残留平滑预览矩形，导致下一次拖同一块面板时从头一处位置开始插值（看起来是跳动）。
- vendored `egui_tiles` 的本地补丁清单记入 `docs/ARCHITECTURE.md`，升级上游时需逐条重新应用。

## [1.2.0] - 2026-08-27

### 新增

- 提供浏览器在线模式，复用桌面端的 egui、Dock、Terminal、Log、Chart、发送器和设置界面。
- 接入 Web Serial：授权设备刷新、用户主动添加设备、串口连接、断开、TEXT/HEX 收发以及 DTR/RTS 控制。
- 增加网络串口连接能力，并将 Native/Web 的串口、快捷键、插件设置和状态栏组件统一为共享 UI。
- Web 端支持 Lua 插件、插件命令、动态面板、工具栏按钮、市场索引和文件选择等浏览器等价能力。
- 增加 WebAssembly 时间源适配、性能诊断和发布构建检查，避免浏览器调用未实现的系统时间接口。

### 调整

- 收拢 Native/Web 的 Application 边界，串口异步任务统一使用 TaskId 和状态事件。
- 恢复并统一桌面端与 Web 端的默认 Dock 布局，修复发送器窄宽度下按钮换行和操作按钮消失问题。
- 串口列表进入页面和再次打开串口面板时自动刷新，过长端口名称使用省略显示并保留完整名称悬浮提示。
- Web 端发布构建使用 release 优化，并支持 GitHub Pages 子路径部署。

### 修复

- 修复 `wasm32-unknown-unknown` 下 `SystemTime`/`Instant` 不可用导致的浏览器崩溃。
- 修复 Native 手动关闭串口后被自动重连，以及重连时串口 worker 尚未退出导致的竞态。
- 修复插件工具栏按钮在发送器中不可见或点击无效的问题。
- 修复 Web 串口授权列表显示不完整、端口名称和分组状态显示异常的问题。

## [1.1.2] - 2026-08-25

### 新增

- G-code Sender 支持跳过 M110 行号同步，并可配置设备“运行中”匹配规则。
- G-code Sender 支持配置暂停和取消时直接发送的原始串口指令。
- 插件设置在插件被禁用时仍可编辑，重新启用后直接生效。

### 调整

- G-code Sender 默认在作业末尾追加 M2，并将 ACK 超时和设备重传处理调整为超时即停止，避免重复执行。
- 插件管理页的启用、禁用操作改为明确的状态事件并持久化配置。
- Windows 便携包使用系统自带 tar 生成 ZIP，避免压缩步骤在 CI 中卡住。

### 修复

- 修改插件或主题后，Windows 构建会重新同步运行时资源，避免安装包携带旧内容。

## [1.1.1] - 2026-08-25

### 修复

- 稳定终端虚拟列表滚动和增量布局更新。
- 仅在 transport 确认端口真正打开后显示连接成功提示，连接超时则显示失败提示。
- 修复 Ubuntu `.deb` 打包时 `/usr/bin/hardware-workbench-app` 符号链接目录不存在的问题。

### 调整

- 优化无配置文件时的默认工作区布局，并将终端作为默认活动面板。
- Dock 拖拽时关闭容易误触发的上下拆分目标，保留左右拆分和标签合并。
- 收紧通知、配置路径和设备面板的布局与显示时长。

## [1.1.0] - 2026-08-25

### 新增

- 提供 Ubuntu 22.04+ x86_64 `.deb` 安装包，安装后可从应用菜单以普通用户运行。
- Linux 用户数据写入用户目录，并为串口权限不足提供 `dialout` 引导；应用本身不要求使用 `sudo` 启动。
- 在 CI 中加入 Ubuntu 构建、Debian 包元数据检查和 SHA-256 校验文件生成。

### 调整

- 建立 UI 无关的 application core 和 DTO 驱动的界面边界，降低前端与业务实现的耦合。
- 重构终端流存储，虚拟化行布局并延迟生成终端行，降低大数据量场景的渲染开销。
- Linux 发行版不显示面向 Windows 便携包的应用内更新入口，改由 `.deb` 发布包升级。

## [1.0.0] - 2026-08-02

### 新增

- 将插件源码、市场索引和发布产物统一合并到主仓库维护。
- 完善 Lua 插件生命周期、权限、命令、任务、配置和动态面板能力。
- 增加可停靠工作区、JSON 主题系统、数据表格与插件场景测试。
- 设置页显示可点击的项目仓库地址。

### 调整

- 恢复 Rust + egui 为唯一桌面客户端，移除 Flutter 和小窗模式遗留。
- 重构插件管理页为响应式紧凑卡片，并统一界面层级、间距和对比度。
- 简化界面提示文字和次要操作，减少视觉噪声。

### 修复

- 修复接收区和日志区自动跟随无法通过向上滚轮暂停的问题。
- 支持左键拖选文本或跨行选择时继续使用滚轮滚动。
- 修复设置导航、工具栏按钮和插件统计切换时的布局抖动。

[Unreleased]: https://github.com/Tydwdh/serial_tool/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/Tydwdh/serial_tool/compare/v1.1.2...v1.2.0
[1.1.2]: https://github.com/Tydwdh/serial_tool/compare/v1.1.1...v1.1.2
[1.1.1]: https://github.com/Tydwdh/serial_tool/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/Tydwdh/serial_tool/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Tydwdh/serial_tool/compare/v0.7.3...v1.0.0
