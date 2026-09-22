# Changelog

本项目的重要变更记录于此。版本号遵循语义化版本。

## [Unreleased]

### 调整

- 重查这 10 条 lint 豁免是否空转 —— **负向结论：10 条全部复发，一条没删**（四道门 711 / 0 / 7，
  与基线逐字相同；本轮只改了守卫测试自己的文档注释，零生产代码变动）。做法沿用第 17 轮：
  一次摘光、native 与 wasm **分别**跑（只跑一条会得出相反结论）。逐站点的新证据：
  `transport/network.rs` 两处 8 参、`lua_host` `plugin_event_loop` 9 参、`gauge.rs::draw_arc` 9 参、
  `panels/lib.rs::next_auto_scroll_state` 8 参、`serial.rs::render_port_editor` 8 参、
  `config.rs` 的 `ConfigLoadResult` 两臂体积差 ≥1089B vs ≥88B、`shared_keymap.rs` 两条是
  `cfg_attr(not(wasm32), allow(dead_code))`（只有 wasm 的 `web` 模块读它们）、`state.rs` 那条是
  刻意暂停的 `NotificationQueue::dismiss`（全仓 grep 只命中定义行）。
  **三处把我自己的前提纠正了，记下来免得下轮再引**：① "每个被豁免文件自第 17 轮以来都改了"
  是**错的** —— 我用 `git log --grep=… | tail -1` 取基线，结果抓到的是一个更早的、正文里提到"豁免"
  的旧提交（`d58af5c`，9-19），于是把 round 17 之前的历史也算进来了。按真正的第 17 轮提交
  `a6dfb3a` 数：**只有 `config.rs` 变了（3 次）**，其余 7 个文件 0 次 —— 也就是说它们根本没有
  漂移的机会，这也正好和"10 条全部照旧必要"的结果对上。教训：取"自某轮以来"的计数要用
  `git rev-list --count <该轮的提交>..HEAD`，别用 `--grep | tail -1` 猜基线。
  ② 第 17 轮记的"最少 9 参"**不是它记的**（它只记了 `draw_arc` 9 参）；本轮实测最小是 **8 参**
  （四处），仍高于阈值 7，所以不影响结论，但别再把"最低 9"传下去。
  ③ **我写的审计步骤本身有个危险的坑**：`-D warnings` 下一个 crate 编译失败，依赖它的 crate
  **就不会被编译**，于是"摘光全部豁免"只报出最前面几个站点就收工（本轮 native 停在
  `tool-transport`，`crates/app` 那 4 条完全没被看到）。照字面执行会得出"那 7 条没复发 = 空转"，
  正是第 17 轮那个"只跑 native"错误的镜像。要收集全部告警必须**去掉 `-D warnings` 再各跑一遍**。
  这条已经写进守卫测试的文档注释（连同 `dead_code` 那 3 条真实的 2+1 形状区分 —— 原文案说
  三条都在 `cfg_attr` 里，其实 `state.rs` 那条是普通 `#[allow]`）。
  棘轮仍然咬得住：探针换成了**在已登记文件里再加一条**（`panels/lib.rs` 上给一个 3 参函数加
  `too_many_arguments` 豁免，两条 clippy 都不会报它，只有守卫能看见），实测报
  "清单 10 条，实际 11 条"后移除 —— 比第 17 轮"重新加一整行"的探针更强，因为它压的是
  按 (位置, lint) → 次数 的比较而不是集合。
- 删掉一个从未被注入过的抽象：`tool_core` 的 `Clock` trait、`SystemClock`、`FrozenClock`
  （含两个 `impl`）整组移除（净 −20 行；四道门 711 passed / 0 failed / 7 ignored，基线 710 → 711
  —— 多的那条是新的形状守卫）。**它文档写着"时间源抽象，使依赖时间戳的代码可测试"，而这句话
  从未成立**：全仓没有任何函数接收过时钟 —— 没有 `dyn Clock`、没有 `<C: Clock>`、没有 `: Clock`
  约束，trait 在 `crates/core` 之外 0 处引用，`FrozenClock` 也从来没有任何用户（第 26 轮删掉它的
  `new` 之后就彻底无法构造了）。没有注入点就谈不上接缝，留着只会让下一个人以为可测性是接好的。
  `SystemClock::now_ms()` 的函数体**逐字搬进** `now_timestamp_ms()`：两个 cfg 分支、wasm 侧
  "没有 OS 支撑的 `SystemTime`，改用浏览器挂钟"那条英文理由注释、`.min(u64::MAX)` 钳制、
  以及系统时钟落在 `UNIX_EPOCH` 之前时 `log::warn!` 后**返回 `1`** 的兜底（刻意不是 `0`），
  签名不动、60 来处调用点无感。搬运是机器核的而不是目测：去掉空白逐行比对后，消失的实现行
  只剩脚手架（trait/struct/impl 头、`fn now_ms`、`#[cfg(test)]`、`Arc<AtomicU64>` 字段）；
  `now_timestamp_ms_is_plausible` **一字未改**继续绿，wasm clippy 重新对 wasm32 类型检查过这个
  crate，是那条 cfg 分支两边都编译得过的实证。
  **仍然跑不到的那条兜底臂**（删之前也跑不到）配了一条形状守卫
  `clock_before_epoch_fallback_is_pinned_to_a_nonzero_literal`：函数文档向调用方承诺"时间戳非零"，
  而没有接缝时唯一能机器检查的办法就是钉实现文本。**限度写在注释里**：钉的是文本不是运行时行为，
  语义等价的改写也会红（那时应连同文档一起重新审）。变异实测：把 `1` 改成 `0` 用例即红并打印出
  被切的函数体，改回即绿。
  踩到的坑也记下来，属于所有文本类守卫的通用问题：**本仓源文件含 CRLF**，按 `"}"` 判函数结束会
  切片越界、把后文当成函数体（第一版就是这么"看着像变异生效"地红了）—— 先归一化换行再切。
- 删掉 `tool-recorder` / `tool-core` 里没人引用的 API（净 −31 行；四道门 710 passed / 0 failed /
  7 ignored —— **用例数一条没动，这正是"确实无人引用"的最强证据**：整条链上任何一处漏查都会有
  编译错误或某条测试变红）。删的东西：`JsonlRecorder::set_backpressure_config` +
  `backpressure_config`、`ReplayManager::publish_next_for_test`、`FrozenClock` 的
  `new`/`advance`/`set`（本来就在 `#[cfg(test)] impl` 里），以及随之变死的
  `pub use RecorderBackpressureConfig`（`tool-recorder` 之外 0 处提到这个类型名；类型仍在
  `pub mod recorder` 里，没有对外破坏）。**一条重要的不对称**：背压字段本身是活的
  （`tick_backpressure` 每帧读它判软/硬阈值，而 `workbench.rs:201` 每帧调它），只有**写侧**从来没人做
  —— 删之前 setter 是"可调用但无人调用"，删之后这条能力**不再可表达**，阈值固定在 `Default`。
  这不是漏接线（没有任何设置/面板/UI 提到过背压，字符串形状也查过：Lua API 表、web host、JSON 键全无），
  要不要开放给用户调是产品决定；已在类型上写一条注释，免得下一个人把它当成没接完的线重新加回。
  `publish_next_for_test` 名字里的 `for_test` 是虚构的：全仓没有任何测试调它。
  **顺带查出扫描器的第 4 条盲区**：`DEAD 8` 少数了 —— 判"用过"靠词边界匹配，而 `set`/`new`
  这类动词名会撞上全仓无关代码（`new` 还被刻意跳过）。**动词名的死活必须结合接收者看**，
  光数出现次数会同时产生假阴与假阳。
  **两条新的后续结论（第 27 轮的靶子）**：① 删掉 `new` 之后 `FrozenClock` 是个**无法构造的 `pub` 类型**
  （字段私有，只剩 `Clone` 与 `impl Clock`），`dead_code` 与两个扫描脚本都看不见它，证据只有全仓
  词边界 grep；② `tool_core::Clock` 这个 trait **在 `crates/core` 之外 0 处引用**，且
  **没有任何多态使用者**（全仓没有 `dyn Clock`、没有 `<C: Clock>`、没有 `: Clock` 约束），
  两个 impl 之外的唯一活用户是 `now_timestamp_ms` 里那句对具体类型的 `SystemClock.now_ms()`
  —— 也就是一个"为实现而实现"的抽象。**代价要说清**：删掉它就删掉了"给测试注入假时钟"的接缝
  （而 64 处调用点全都走全局自由函数，这个接缝至今一个用户也没有），所以这一步该不该走值得先问一句。
- 把用户可见错误正文里最后一段英文清掉（四道门 710 passed / 0 failed / 7 ignored，基线 700 → 710；
  新增 10 条用例：`storage_error_wording` 6 + 回放/导出/配置三条活路径 + recorder 单元 1）。
  新增 `tool_platform::storage::translate_error`（3 个变体，穷尽 `match` 不写 `_ =>`，
  与 `TransportError` 那句"此平台不支持该传输能力"**不共用任何句子**，由新用例断言钉住）。
  **一个刻意与 transport 相反的取舍**：`StorageError::Operation(detail)` 在这里套一层中文框架，
  而 transport 那份是原样透出 —— 因为 transport 的载荷进门时已翻译过（再套就是
  「操作失败：串口错误：…」），而 storage 的七个生产者（worker 退出、`std::io::Error` 原文、
  `file id escapes storage root`、`window unavailable`、JS 值原文、`decode settings:`、
  `serde_json` 原文）**一个中文都不是**。
  接三处并逐字实测（本机 zh-CN Windows）：导出失败 `storage operation failed: 系统找不到指定的路径。 (os error 3)`
  → `存储操作失败：系统找不到指定的路径。 (os error 3)`；**本轮新查到的活站点** —— 设置面板
  「最近工作区 → 打开」把 `read_native_file` 的 Err 原样写进状态栏，
  `storage operation failed: 拒绝访问。 (os error 5)` → `存储操作失败：拒绝访问。 (os error 5)`
  （把 `.map_err` 退回 `to_string()` 该用例即红，实测过）；`write_native_json` 到不了用户眼前
  （`save_config` 十个调用点都只 `log::warn!`），统一只为"同一个失败一种说法"，注释里写明。
  回放那条改在**生产者**：`ReplayManager::prepare_load` 里 `File::open(&path)?` 是唯一没中文框架的
  一处（邻居与 `oversize_file_error` 本来就是中文），在 `workbench` 侧补会把已带框架的路径套成双重框架；
  行读取 `line?` 同一处理，由注入失败迭代器的 recorder 用例驱动。**顺带纠一处可达性**：文件
  "不存在"走不到 `File::open`（`metadata` 先失败，用户看到的本就是中文），新框架管目录/独占/权限。
  **只记不修**（都写进 `docs/ARCHITECTURE.md`）：`create_recording_file` 的 `create_dir_all` /
  `File::create` 同样未框架化（`录制错误：<OS 原文>`）；配置读写失败在两端被丢弃（native
  `if let Ok(..)` + 十个 `log::warn!`，web `.ok()` + `let _ =`），症状是"设置没保存上"而零反馈 ——
  那类改动等于新增用户可见反馈，是产品裁决（第 12 条⑧）。
- 去掉夹在中英文之间的 `AppError` 分类前缀，并修好两条插件诊断文案（四道门 700 passed /
  0 failed / 7 ignored，基线 697 → 700；新增三条用例都是建/删临时目录的 fs 用例，故另测失败率：
  `tool-extension` 里匹配 `manifest_` 的那 10 条连跑 12 次 0 红）。
  `crates/application/src/error.rs` 七个 `#[error(...)]` 的英文分类标签换成中文：`传输错误：`
  `录制错误：` `回放错误：` `存储错误：` `插件错误：` `当前状态不允许：` `命令不支持：`
  （变体名与载荷类型不动，那是对外 API；标点跟三个既有映射器同一形状 —— 全角冒号）。
  桌面用户点「启用」一个不存在的插件，从前读
  `启用插件 demo.x 失败：plugin: 未找到插件 demo.x（请先重新扫描插件目录）`，现在那句英文前缀没了。
  **没有因此宣称两端一致**：`pub mod error` 被 `cfg(not(target_arch = "wasm32"))` 挡在 wasm 之外
  （`lib.rs:19-20`），web 侧 `to_string()` 出来仍是**不带任何分类前缀**的那句，"native 有一层框架、
  web 没有" 这条按设计仍在。动手前按第 21 轮的教训逐个查了七个旧前缀有没有被判定读取
  （`.contains`/`.starts_with`/`==` 全仓扫）：**零命中** —— 命中的 `replay:` / `plugin:` 全是 DataBus
  事件的 `source` 前缀（`panels/src/dynamic/ingest.rs`、`recorder/src/replay.rs`），与 `Display` 无关。
  逐字钉只有 `headless.rs:277` 那一格，它原本就写着"改 `transport: ` 前缀会让本断言变红"，
  本轮把期望值与那句注释一起换成中文（另两处引用该字面量的注释 —— `send_plan.rs` 的双前缀
  说明、`crates/extension/tests/error_wording.rs` —— 同步换掉，理由保留）。
  `scan_roots` 的两条诊断改走 `tool_extension::translate_error`：`manifest_parse_error` 此前内插整个
  `ExtensionError`，插件面板渲染成 `manifest 解析失败: manifest parse error: <serde 原文>` 双重框架；
  `permission_denied` 正文干脆是裸 `error.to_string()`（整句英文）。**顺带纠了一处谎报**：
  `load_manifest` 既抛 `Manifest` 也抛 `Io`，"解析失败"那个标签对读不出来的文件根本不成立 ——
  现在这种情形用户读到的是 `插件文件访问失败：<OS 原文>`（本机实测 `插件文件访问失败：拒绝访问。 (os error 5)`）。
  三个新用例（同名目录造 `Io`、畸形 JSON 造 `Manifest`、白名单外权限造 `PermissionDenied`）
  只钉框架与"不许出现的英文"，第三方细节跟随系统语言不钉；`PermissionDenied` 整句是自己写的，
  逐字钉，并钉它不与 `浏览器拒绝了串口授权请求` 撞（两个概念）。诊断码
  `manifest_parse_error` / `permission_denied` 一字未动（查过：面板只显示它，`command_not_found` 那条才被匹配）。
  **四道变异实测**（都在本机跑过、看过原文再改回）：① 改 `error.rs` 的一个前缀 ⇒
  `headless.rs` 那格红（`left: "串口错误：HEX …"` / `right: "传输错误：HEX …"`）；② 把诊断退回
  `format!("manifest 解析失败: {error}")` ⇒ 新两格红，其中读不出来那格打印出
  `"manifest 解析失败: io error: 拒绝访问。 (os error 5)"`，正是谎报 + 双重框架的现场；
  ③ 只把「解析失败」写成无条件（`format!("manifest 解析失败：{}", translate_error(&error))`）⇒
  仍红且报出重复框架 `manifest 解析失败：manifest 解析失败：…`，证明 ② 的功劳不只是"少了英文"；
  ④ 退回裸 `to_string()` ⇒ 第三格红（`left: "permission 'filesystem' is not allowed for plugin 'greedy.plugin'"`）。
  **两处与第 22 轮记录不符、按本轮实测更正**：(a)「缺失的 plugin.json 被报成解析失败」只对了
  一半 —— `scan_roots` 在 `load_manifest` 之前先 `manifest_path.exists()` 判空并 `continue`，
  缺失那格压根不产生诊断，谎报只在"文件在、读不出来"；(b) 这两条诊断实际**只有 native 看得到** ——
  `tool-extension` 不参与 wasm 编译，而 `crates/app/src/web/plugins.rs:43` 给面板的 `diagnostics`
  恒为 `Vec::new()`（渲染器在，输入永远没有），上一轮记的"两端各自经 tiles.rs 与 web/plugins.rs 渲染"
  对 web 那一半是空的。
- 删掉 `tool-panels` 里没人调用的面板 API（净 −95 行、7 个方法；四道门 697 passed / 0 failed /
  7 ignored，基线 698 → 697 —— 少的那一条是"只测自己"的用例，见下）。
  **起点是我自己的结论错了**：先前记的"死代码已见底（481 个 pub 项零引用者为 0）"其实只覆盖到
  **顶层条目** —— 那条扫描的正则 `^pub (fn|struct|…)` 锚在零列，**`impl` 里的方法一个都没看过**；
  `dead_code` lint 又因为它们是 `pub` 而沉默。触发点是 `LuaHost::run_script`（全仓只有它自己一行）。
  补的 `impl` 方法扫描在 731 个固有 `pub` 方法里查候选，且它第一版还带第二条盲区：
  **测试里的引用也算"用过"**，于是"只被自己那条单测调用"的方法会隐身（`sync_tabs_from_dock` 就这么混过）；
  改成按生产引用/仅测试引用分桶后是 `DEAD 8` + `TEST_ONLY 25`。**两桶不能混**：
  `render_chunk` 那两姊妹存在的意义就是被"分块导出 == 整块导出"等价用例调用，删它是拆自己的测试能力；
  `NotificationQueue::dismiss` 虽在 DEAD 桶里，但它带着 `#[allow(dead_code)]` 和一条写明
  "Toast 得先携带 source"的理由，那是刻意暂停，保留。
  本轮删的四组：① 终端书签整支死支路（`TerminalPanel::bookmarked_entry_ids` 字段 + 5 个操作点 +
  `is_bookmarked`/`toggle_bookmark`）—— **恒为空**是可证的：唯一的写入者 `toggle_bookmark` 零调用者，
  其余四处都是 clear/remove；`TerminalPanel` 无任何 `#[derive]`（不是 serde 形状）；该文件测试数
  33 → 33（这支特性本来就没被测过，与"从没接上过"互相印证）。`replay.rs` 那套**活着的**书签是另一个
  概念，没动。② `DockLayout::move_panel` 与随之变孤儿的 `stack_mut`/`reveal_area`
  （`remove_from_all_areas` 仍被 `close_tab` 用着；它 doc 里"跨区移动与关闭都必须先做这一步"已经变成
  假话，改成只说关闭）。③ `DockStack::reorder`。④ `PanelManager::sync_tabs_from_dock` 连同它那条形同
  虚设的单测 —— 它存在的理由写在 doc 里："在 `dock.move_panel()` 调用后同步 active_tab"，而那个调用者
  本轮没了，删代码就把它留下的假 doc 一起删。
  漂移审计确认**一个用户可见字符串都没消失**（manager.rs 91 个、terminal.rs 337 个逐字保留），
  消失的只有一个数字字面量 `1`（在被删的 `reorder` 体内）与 6 条随代码一起走的注释。
  两个查到但**只记不删**的疑问排到下一轮：`PanelManager::select_center_panel` 的 doc 自称是左侧栏
  点击的处理器却生产零调用（组合根 `ui/tiles.rs:12` 直接赋 `active_tab` 字段绕过了它），
  以及 `Workbench::query_terminal_since` 有 8 处测试引用、0 处生产引用。
- 插件错误到用户眼前改走中文映射器（四道门 698 passed / 0 failed / 7 ignored，基线 685 → 698）：
  新增 `tool_extension::translate_error(&ExtensionError) -> String`，按**变体**分发且**不写 `_ =>`**
  （`crates/extension/tests/error_wording.rs` 的 `variant_name` 同样不写兜底 ⇒ 以后加变体忘了配中文
  就编译不过，而不是悄悄把英文漏给用户），接上四处用户可见出口 —— `Workbench` 的 Enable/Disable
  派发与 `spawn_reload_plugins` / `spawn_discover_plugins` 两条任务失败消息（后者落
  `TaskSnapshot::message`，即第 10 轮已经中文化的同一条状态栏）。桌面用户此前点「启用」一个不存在
  的插件，状态栏读到的是 `启用插件 x 失败：plugin: plugin 'x' was not found`（中文框架套英文前缀
  套英文正文），现在正文那半是中文了。
  顺带删掉 `ExtensionError::UnsupportedApiVersion`：**全仓零构造点**，真实路径是 `scan_roots` 里
  `api_version_supported()` 为假时推一条中文 `unsupported_api_version` 诊断再 `continue`
  （与第 20 轮那两个零构造 `AppCommand` 同一类；`publish = false` 且该枚举不序列化，不算对外破坏）。
  用例分两层：`NotFound` 整句逐字钉（自己写的句子、且正是状态栏那句），带 OS 细节的三臂
  （`Io`/`Manifest`/`Lua(mlua)`）只钉中文框架 + 断言英文 `Display` 没漏进来 ——
  本机实测 `read_dir` 失败给的是「目录名称无效。 (os error 267)」，钉死细节就把用例变成一台机器的快照。
  三处变异实测：把一处接线改回 `.to_string()`、改一个中文字面量、给枚举加第 9 个变体，
  分别让逐字用例 / 文案用例 / 两处穷尽 `match` 编译失败变红。
  **三条被推翻的前置判断（记下来免得下轮重推）**：① `tool-extension` 其实**不参与 wasm 编译**
  （`crates/application/Cargo.toml` 把它放在 `cfg(not(target_arch = "wasm32"))` 下）；
  ② `UnsupportedRuntime` 的分支活着但输入进不来（`spec::RUNTIMES` 只有 `lua`，`scan_roots` 先跑
  `manifest.validate()`，非法 runtime 早被折成中文诊断）；③ `ExtensionError::Lua` **没有生产者**
  （`run_plugin` 一律返回 `Ok`；全仓唯一那个 `Err(LuaHostError::AlreadyRunning)` 在零调用者的
  `LuaHost::run_script` 里）。②③ 两臂是下一轮的死代码候选。
  **同一轮新查出两处英文到用户眼前**、刻意留到下轮与 `plugin: ` 前缀同批：`scan_roots` 的
  `manifest 解析失败: {error}` 内插的正是 `ExtensionError`（渲染成 `manifest 解析失败: manifest
  parse error: …` 双重框架），紧邻的 `permission_denied` 诊断正文是裸 `error.to_string()`。
  `dispatch_load_replay` 与 `write_exported_content` 仍未接映射器。
- 插件「重启」交接的判据从**英文错误子串**换成类型化状态查询（四道门 685 passed / 0 failed /
  7 ignored，基线 682 → 685）：`crates/app/src/ui/tiles.rs` 的 `take_pending_restart` 循环原先写
  `if let Err(AppError::Plugin(message)) = dispatch(EnablePlugin) && message.contains("shutting down")`，
  读的是 `ExtensionError::Stopping` 的 `Display`。
  **和第 9 轮那个恒假判定同形，但这一条是活的**：`PluginManager::disable` 只设 stop 标记、不 join，
  Lua 线程要到 `plugin_event_loop` 循环顶才看那个标记、而每轮最多睡 50 ms（`wait_duration` 实测为
  `.unwrap_or(50ms).min(50ms)`），所以「点重启 → 下一帧 enable 被拒 → 塞回队列再等一帧」是常态路径。
  下一轮本要把 `ExtensionError` 的 9 个变体翻成中文 —— 照旧写法，那句 `contains` 会从那一刻起永久恒假：
  用户点「重启」再没任何反应，而四道门逐字全绿（判据写在 egui 渲染分支里，没有任何用例经过那里）。
  所以先拆雷再翻译：新增 `PluginManager::is_stopping`（回答前照 `enable` 一样先收割一次，`enable`
  反过来调用它 ⇒ `stopping_plugins.iter().any(...)` 那条比较只剩一份）、
  `Workbench::plugin_is_stopping` 做最窄转发（`plugin_manager` 字段仍私有），组合根从此问状态不问文案。
  **用户可观察行为零变化**（两条分支的取舍与旧代码逐一对应：仍在关闭就留到下一帧、其余失败照旧丢弃）。
  证据分两层，各证一半：
  - 判定本身可驱动 ⇒ `headless.rs::plugin_restart_handoff_relies_on_the_stopping_predicate` 用真实
    Lua 探针插件走 false → 停机窗口内 true 且同窗口 enable 必被拒 → 收割后翻回 false 并重新启用；
    把 `is_stopping` 改成恒 false 实测变红。它只观察一次那个停机窗口（不轮询），所以另量了失败率：
    空闲连跑 30 次 0 红、整套 headless 并行连跑 5 次 0 红。
  - 文案独立性不可驱动（那两行在渲染分支）⇒ `crates/app/tests/plugin_restart_handoff.rs` 钉形状：
    循环体必须恰好问一次 `plugin_is_stopping` 且派发排在其后，`.contains(` / `to_string()` /
    `"shutting down"` / `AppError::Plugin` 一律不许出现。把改动前的写法原样贴回去，两条断言双双变红，
    恢复后全绿；另实测把 `Stopping` 文案换成中文后，受影响 4 个 crate 的 362 passed 前后逐字相同。
  web 侧同名循环**刻意没动**（已核实 wasm 的 `dispatch(EnablePlugin)` 只是入队并返回 `Ok`，
  `Stopping` 在浏览器侧根本不产生，不是同一颗雷）；顺带记下一处已核实、未修的不对称：enable 失败时
  web 把错误写进状态栏、native 直接丢弃，那是产品取舍。`ExtensionError` 九个变体的中文映射器
  因此排到下一轮。
- 把"46 个 `AppCommand` 里，两端各自处理了哪些"对照了一遍，查出**两个从来没人构造的变体**并删掉
  它们与整条只被它们用到的链（4 个文件、净 −107 行）：
  - `AppCommand::ExportTerminal` 的处理器 `Workbench::dispatch_export_terminal` 是终端导出的
    **第二份实现**——和活路径一样落到 `write_exported_content`，于是删掉它又连带摘出
    `ServiceTerminal::export_job` → `TerminalExportJob` → `render_export` → `csv_escape`/
    `format_dir`（每一样的"没有别人用"都是词边界 grep 查的；私有函数那几样另有 clippy 的
    `dead_code` 兜底，它们若真成孤儿，`-D warnings` 门就会红）。
  - `AppCommand::ExportLog` 的处理器是一个**注释里明写"占位"**的 no-op（忽略两个字段直接返回 Done）。
  两个变体都带 `#[cfg(not(target_arch = "wasm32"))]`，所以 wasm 侧那份穷尽 `match` 从来没见过它们
  —— 删掉后 wasm 的变体集逐字不变，不存在"别的命令换了分支"的可能（由 wasm 门证实）。
  可达性也核过：全仓（含测试）零构造点，且 `lua_host`/`plugin_runtime`/`extension` 里对
  `AppCommand` 是 0 引用，插件（本仓全是 Lua）没有任何途径构造命令；`AppCommand` 只 derive
  `Debug, Clone`，**不序列化**，所以这不是 serde 形状改动。
  占位虽然删了，它背后那条设计约束留在 ARCHITECTURE 里：**日志导出归 `LogPanel`（presentation），
  application 层不碰 recorder**；终端导出的活路径也只有一条
  （`commands.rs::export_terminal_data` → `Workbench::spawn_file_export` → `write_exported_content`），
  并写明"别再在 `dispatch` 里加第三个入口"。
  字面量审计单独看过消失的用户可见文案：`Native 导出需要路径型文件句柄` 与任务种类
  `export_terminal` 都还在活路径上用的那份里（各少一处重复）；只有 `正在导出终端数据`
  整串消失，而它**只可能由那条不可达命令产生**，用户从来没见过它。
  四道门 exit 0（682 passed / 0 failed / 7 ignored —— 与基线逐字相同，一条用例都没删）。
- "持久化 JSON 里哪个键声明了 schema 版本"这条键名规则（`schema_version` 优先、历史键名
  `version` 兜底、非整数当作没声明）此前**在三个 crate 里逐字写四遍**：
  `crates/app/src/config.rs` 两遍（`declared_schema_version` 与 `parse_persisted_config`）、
  `crates/lua_host/src/config.rs` 一遍、`crates/panels/src/theme.rs` 一遍。
  一致靠的是 app 里那段注释（"这里与 `parse_persisted_config` 都必须同时接受两个键，
  否则这类配置会被读成 v0：既可能重复执行 v0 迁移，也可能漏掉「版本高于当前程序」的判定"）
  —— **没有任何机器检查**。现在收成 `tool_core::config::declared_schema_version(&Map) ->
  Option<u32>`（core 本来就持有 `CURRENT_SCHEMA_VERSION`，三个 crate 本来就依赖它，
  没加任何依赖边），四处全走它；**各家盖在它上面的策略刻意原地不动**：app 保持
  `Option<u32>` 与 `.unwrap_or(0)` 两种读法、lua_host 保持 `contains_key` 守卫、
  theme 保持 `if version == 0` 的 v0 迁移分支。**生产行为零变化**（理由逐条核：
  `unwrap_or(0)` 只在 `None` 时生效，与旧的 `.unwrap_or(0) as u32` 同值）。
  `contains_key(...)` 与 `declared_schema_version(...).is_some()` **不等价**，所以守卫没动：
  `{"version":"1"}` 这种"键在、值不是整数"的文档，前者仍走带 `values` 的分支，
  后者会掉进 v0 分支把整个根对象当配置读 —— 这一点写成了代码注释。
  键名规则拿到它的**第一个机器检查**：core 三条新用例（两个键都认、`schema_version` 优先、
  非整数/缺失为 `None`，含"先命中的键决定结果，不会回落到另一个键"这条现状语义），
  app 再加一条跨reader一致性用例 `legacy_version_key_is_a_version_for_both_config_readers`
  —— 它断言的是**完整错误文本**而不是 `is_err()`，因为消息里的版本号才是"parse 侧到底认出了
  哪个键"的唯一可观察量。变异验证四次：删掉历史键 → core 与 app 两条同时红；颠倒优先级 →
  core 红；把非整数读成 `Some(0)` → core 红。
  **本轮顺带查出一个既有缺陷（没动，已开单）**：`parse_persisted_config` 会往 map 里
  **插入** `schema_version` 却没有删掉 `version`，而配置结构体上有 serde `alias = "version"`，
  于是一份只声明了历史键、版本又不高于当前的配置反序列化时直接报
  「配置字段无效：duplicate field `schema_version`」；`theme.rs` 正是用
  `object.remove("version")` 避开了这个坑。上面那条 app 用例之所以钉的是"未来版本"的
  历史键文档，就是因为它在插入之前就被拒绝了、绕不到这个 bug —— 这是第 19 轮的靶子。
  四道门 exit 0（681 passed / 0 failed / 7 ignored = 677 + 4 条新用例）。
- 再跑一次第 9 轮那套"把豁免一次摘光、让 clippy 逐条点名"的实验，**判出 6 条早已空转的
  `dead_code` 豁免并删掉**（`crates/app/src/panel_registry.rs` 的 `PanelKind` 一条，
  `crates/panels/src/gauge.rs` 的 `ZoneKind` / `from_config` / `parse_zones` /
  `parse_zone_color` / `parse_zone_kind` 五条）。它们全是
  `#[cfg_attr(target_arch = "wasm32", allow(dead_code))]`，而护着的东西在 wasm 上早就是活引用：
  `PanelKind` 在 `crates/app/src/web/mod.rs` 里既被构造也被 `match`；gauge 那五个由
  `crates/panels/src/dynamic/ingest.rs` 在总线驱动下调用（`DynamicPanels` 在 wasm 侧是活的）。
  **这 6 条只有 wasm 那半边测得出来**：只跑 native clippy 会得出相反结论 —— 被
  `cfg_attr(wasm32)` 护着的东西在 native 构建里本来就活着，native 永远不会抱怨它。
  实验过程里差点多做一件错事：`gauge.rs` 同文件还有一条 `too_many_arguments`，它在 wasm 侧
  **仍被点名**（`draw_arc` 9 参），跟着那 5 条一起删就会让 wasm 门直接红；按目标分别核过才留下。
  于是豁免棘轮从 16 条收紧到 **10 条（7 clippy + 3 dead_code）**，`lint_exemptions.rs` 的
  字面量清单同步收紧，并把"分别跑两条 clippy 再判空转"的方法写进那份守卫与 ARCHITECTURE。
  收紧后的守卫自己也被证明会红：把其中一条豁免原样加回去，测试报
  「清单 10 条，实际 11 条 / 新增 crates/panels/src/gauge.rs dead_code 多了 1 条」。
  **生产行为零变化** —— 空转的 `allow` 只是让告警静默，删掉它不改变任何代码路径；
  四道门 exit 0（677 passed / 0 failed / 7 ignored，一条用例没动）。
- 清掉最后 **3 个零引用的 `pub` 项**，于是这一类取证到底了：把扫描形态铺开到
  `pub fn` / `struct` / `enum` / `trait` / `const` / `static` / `type` / `union` 之后重跑，
  **481 个候选里 0 个还有"只出现在自己定义行"的项**（第 11 轮是 28 个，全部 `pub fn`）。
  删的是：`tool_application::NativeRuntime`（`cfg(not(wasm32))` 下与 wasm 侧 `WebRuntime`
  对称的别名；`Workbench` 本身已由上面那条 `pub use` 导出，原生调用方也一律直呼 `Workbench`，
  这个别名什么都没带上）、
  `tool_testing::TestManager`（`= TestReportStore` 的同义别名，零引用）、
  以及 `tool_platform::TransportEvent`（25 行、5 个变体的"后端向上传事件"枚举）。
  `TransportEvent` 不是随手漏的小函数，而是**同一件事的第二套模型**：它的文档写着
  "backend can surface to Application/DataBus"，但全仓没有一个构造点、没有一个匹配点 ——
  真正在跑的通道是 DataBus 上的 `tool_core::Event`（同文件上面那组 `serial_*_event` 构造器）。
  留着它的代价不是 25 行，而是读者要花时间猜哪一套是活的；删它时在原位留了一条**普通注释**
  （不是 `///`：这里与下一个条目之间隔着空行和另一段 `///`，写成文档注释会被并进
  `TransportBackend` 的 rustdoc 里）。
  判据与第 11 轮同一套：词边界计数、注释行不算引用、`plugins/` 与 `plugin-marketplace/`
  是 11 个 Lua 文件 0 个 `.rs`（Lua 只能经宿主注册名触达 Rust，那会表现为一次标识符引用）。
  本轮 3 项在删前逐一复核过动态可达性（`wasm_bindgen` 属性、JS 侧、serde 持久化形状、
  文档承诺）：三者都不是 `#[wasm_bindgen]` 导出、`.md` 文档里 0 次提及、也没有任何
  `pub use` 把它们再导出去 —— 所以删它们不改任何外部可见形状。
- 卸载插件时算暂存目录名的 `ensure_unique_dir` 也有两份**逐字节相同**的实现（各 14 行）：
  `tool-marketplace` 里一份私有的，`crates/app/src/runtime/marketplace.rs` 里一份带 `&self`
  （形参根本没用上）的私有副本。现在只剩 `tool_marketplace::ensure_unique_dir`（`pub`），
  app 侧调用它。
  **这次重复是有理由的，而且理由写在代码里** —— app 那份的注释写着"与
  `marketplace::ensure_unique_dir` 同语义，避免跨 crate 暴露"。本轮没有悄悄推翻它，而是把这段
  理由连同"为什么不成立了"一起写进函数自己的文档：本仓 `publish = false`、工作区外没有任何 Rust
  消费者（`plugins/` 全是 Lua），而 `crates/app` 本来就依赖 `tool-marketplace`，所以这里的 `pub`
  只意味着"工作区内可链接"，正是想要的共享。同时留下重复的真实代价：后缀用尽时这个函数返回的是
  **仍然冲突**的原路径，两份副本改一份忘一份，就会去覆盖一个已存在的目录。
  **这条函数此前两份都是零覆盖**（`grep` 全仓没有任何测试引用它）。新增
  `stash_dir_name_avoids_existing_directories_without_creating_one` 钉住三件事：不冲突时原样返回
  且**不顺手创建目录**、被占用时加 `.1`、`.1` 也在时走到 `.2`（同父目录、后缀从 1 起算）。
  变异验证：让不冲突分支也创建目录 → 红在「只该算名字，不该创建目录」；后缀忘了编号
  （`{stem}.{i}` → `{stem}`）→ 红在第 ② 条（`left` 就是那个仍被占用的原路径）。均还原复跑回绿。
  **仍未覆盖的一支**：`1..1000` 后缀全部被占用后的兜底（返回冲突的原路径）。要钉它得先造出 999 个
  目录，本轮没做，也没有假装做过 —— 这个兜底本身是不是好选择（覆盖 vs 报错）留作下一次判断。
  app 侧被这次删除腾出两个空 import（`Path`/`PathBuf`），一并去掉。
- `parse_gcode_response` 原有**两份逐字节相同的实现**（各 12 行、370 字节，按函数体 diff 确认相同）：
  native 在 `tool-transport::network`、wasm 在 `tool-platform::web_network`。合成一份落到
  `tool_core::parse_gcode_response`（`crates/core/src/lib.rs:600`），两个调用点改用它。
  **为什么这组重复是结构性的、不是偷懒**：`crates/platform/Cargo.toml` 把 `tool-transport` 放在
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` 下 —— wasm 构建里 platform 根本链接不到
  transport，所以那份代码除了再抄一遍没有别处可去。`tool-core` 是两端都无条件依赖的唯一点，
  这个理由写进了函数自己的文档，免得下一个人"顺手改成引用 `tool_transport`"直接把 wasm 构建弄坏。
  与第 13 轮同一类漂移：native 那份带着文档（"只处理 `notify_gcode_response`，其余 notify 一律忽略"），
  wasm 那份一个字都没有。唯一的公开面扩大是必需的：跨 crate 共享只能 `pub`（两份旧实现都是私有），
  没有新增依赖，`Cargo.toml`/`Cargo.lock` 一字未动。
  **两处测试合并时更正了一个错误判断**（主代理原本的描述是错的，子代理用对照运行证伪）：
  wasm 副本的 `parses_only_gcode_notifications` 里"`notify_status_update` + 空 `params`"那条
  并不比其他用例多走一个分支 —— method 守卫就在前面返回，`.first()?` 根本没执行；transport 那
  三条里的对象 `params` 同理。所以"照搬合并"会让删除 `.first()?` 或删掉 method 守卫这两个变异
  **全都无人变红**（对照运行实测：exit 0、27/26 passed）。最终：wasm 那条用例的两条断言按形状并入
  迁移后的用例（它本来不计进原生测试数 —— `web_network` 整个模块被 `#[cfg(target_arch = "wasm32")]`
  挡着），并净新增 `ignores_gcode_notification_with_no_text_params` 钉住守卫放行之后的
  `.first()?`（空数组）与 `.as_str()`（首元素非字符串）两行，外加一条"非 gcode 推送但首元素是
  字符串"来钉 method 守卫本身。**这是本轮唯一改变测试语义的地方，方向是变严。**
  变异验证（各改生产代码一行）：删掉 method 守卫 → 红在 `ignores_status_update`
  （`Some("printing")` vs `None`）；把 `.first()?` 改坏 → 红在
  `ignores_gcode_notification_with_no_text_params`（`Some("")` vs `None`）。均还原复跑回绿。
  四道门 exit 0（676 passed / 0 failed / 7 ignored；等式 = 675 + core 新增 4 条 − transport 迁走 3 条）。
- 任务快照的保留规则原有**两份逐字节相同的实现**（各 536 字节）：native 在 `task.rs::prune_finished`、
  wasm 在 `web.rs::prune_finished`。现在合成一份 —— `task_model::prune_finished_snapshots`，
  两个结构体（`TaskManager` / `WebTaskRegistry`）各自留一行委托。选 `task_model` 作宿主是因为它是
  这条规则唯一能活的地方：`task.rs` 是 `cfg(not(wasm32))`、`web.rs` 是 `cfg(wasm32)`，
  **互斥编译的两份代码没有任何一次构建能同时看见**，所以它们分叉时编译器不会有任何反应；
  事实也是如此 —— wasm 那一份**连文档注释都没有**，"只回收终态快照、否则 UI 按 kind 查活跃任务会查空"
  这个理由只写在 native 那份上。**行为变化为零**：两份函数体字节级相同、常量同值 256，
  合并后类型仍由编译器保证（`BTreeMap<TaskId, TaskSnapshot>` 是 `task_model` 自己的类型）。
  顺带补上这条规则**此前完全没有的测试覆盖**（改动前 `grep -rn prune_finished crates/` 在所有
  `tests/` 目录下 0 命中 —— 没有任何一条断言直接钉过它）：
  `retention_never_touches_live_snapshots_and_stops_exactly_at_the_bound` 两条断言分别钉住
  文档里那两句话。变异验证：去掉终态过滤 → 红在「全是未结束快照时不得回收」（`left: 256`）；
  上限判定 `>` 改 `>=` → 红在「回收必须停在上限」（`left: 255`）。两次都改了生产代码的
  一行、都还原并复跑回绿。
- 删掉 **36 个编译器永远看不见的死公共项**（5 行增、396 行删，12 个文件；本仓第一次以删除为主的轮次）：
  `dead_code` lint 只看 crate 内可达性，`pub mod` 里的 `pub` 项永远算"可达"，所以这些一条警告都不会发；
  而"零引用 = 不可达"这个推断成立的前提也核实了 —— workspace `publish = false`，且**没有任何工作区外的
  Rust 消费者**（`plugins/` 与 `plugin-marketplace/` 是 11 个 Lua 文件、0 个 `.rs`，Lua 只能经宿主注册的
  名字触达 Rust，而那会表现为一次标识符引用）。判据是**词边界**的整仓 token 计数，注释行不算引用：
  子串搜索会把 `widget_active` 与活着的 `widget_active_weak` 混为一谈。
  - `panels/theme.rs` 的 22 个配色垫片（1090 → 1010 行；这个文件 25% 的公开面从来没人读过）。
  - 8 个零调用方法：`SerialPanel::top_bar_ui`（4 行死包装，两个活调用点自己做了 `horizontal_wrapped`）、
    `web_serial::start_receive`（一行死重载）、`DockStack::add_inactive`、`DockLayout::insert_panel_at`、
    `JsonlRecorder::clear_completed_path`（body 本身也近乎无效：只在 `worker.is_none()` 时清一次）、
    `PermissionManager::missing_capabilities`、`Workbench::query_transport_capability`、
    native `Workbench::validate_hex`。
  - 连带删除（每一样的"没有别人"都单独查过）：`PluginManifest::required_capabilities`（唯一消费者就是
    上面那条）、3 条随之失效的 `use`、`tool_core::hex_preview` 连同它自己的 3 条用例（675 → 672）。
  - ARCHITECTURE 第 9 条记了一轮的那批 `pub struct` 死导出：`plugin_view.rs` 整文件、`ReplayView`
    及其两个 impl（同文件活着的 `ReplayUiCommand` 保留）。**这批为什么漏了一轮**：死代码扫描按
    `pub fn` 找，`pub struct`/`pub enum` 不在它的形态里 —— 工具已补上，重跑后全仓只剩 3 项待看。
  两条**与代码不符的文档陈述**随条目消失（`query_transport_capability` 自称"used by shared
  presentation code"、`required_capabilities` 自称"used by Native/Web hosts"，实测都是零调用）；
  `recorder.rs` 那句"调用者需在读取后调用 `clear_completed_path()`"改写成现状描述，并按
  `replay_error`/`stop`/`start` 的实际行为逐条核对过（出错时 `reap_error` 已就地清、否则由下一次
  `start()` 覆盖或被 `stop()` 取走）。
  **这一轮的"证明会红"不需要变异测试**：删除的判据是编译期事实 —— 任何一个还有引用，native 或 wasm
  构建立刻 E0425/E0433，两道 clippy 门（含 wasm 目标）本身就是检查。四道门 exit 0
  （672 passed / 0 failed / 7 ignored）。
  **新发现（等拍板，本轮不动）**：删掉 `missing_capabilities` 后 `tool-extension → tool-plugin-api`
  成为可证明的死 Cargo 边（该 crate 已 0 处 `use`），与第 10 条 `egui_extras` 同类 —— 要动 `Cargo.lock`。
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

- **修掉一个"从旧版本升级上来、第一次开机就丢设置"的缺陷**：`schema_version` 改名之前写的
  `workspace.json` 只用历史键 `version` 声明版本，这样的文件在本版本读不出来 ——
  `parse_persisted_config` 会无条件往 map 里 `insert("schema_version", …)`，而
  `PersistedConfig.schema_version` 上写的是 `#[serde(default, alias = "version")]`，
  旧键又没被删掉，于是 serde 在同一字段上看到两个键，报
  「配置字段无效：duplicate field `schema_version`」。真正严重的是后果：`load_config`
  把解析失败当作"配置损坏"，下一步就是 `quarantine_corrupt_file(primary)` **把用户的工作台
  配置挪走**并回落到默认值 —— 面板布局、选中端口、波特率、主题全部回到初始状态。
  修法与 `theme.rs` 里已有的做法一致：归一化时 `insert` 之后补一句 `object.remove("version")`。
  **同一形状都查过**：`crates/panels/src/theme.rs` 早就这样处理（所以它没这个坑）；
  `crates/lua_host/src/config.rs` 的插件配置文档同样带 `alias = "version"`，但它是手工读字段、
  再从 Rust 值构造文档，不反序列化被改过的 map，因此不受影响 —— 这条差异也写进了注释，
  免得后来人以为"三处都改过"。
  **证据是先红后绿**：新用例 `legacy_version_only_config_still_loads` 在改之前红在
  `只用历史键声明版本的配置必须能读出来，实际: 配置字段无效：duplicate field `schema_version``，
  改之后连同该模块全部 14 条用例一起绿。它也与第 18 轮的
  `legacy_version_key_is_a_version_for_both_config_readers` 配成对：那条只能钉"未来版本"的
  历史键文档（它在插入之前就被拒绝、绕不到这个坑），这条钉 `<= 当前版本` 的那一半。
  本轮不动 serde 形状：磁盘上仍接受两个键、写出的仍是 `schema_version`。
  四道门 exit 0（682 passed / 0 failed / 7 ignored，+1 条用例）。
- **浏览器端「网络串口」那一行原先四件事全坏**（连接、重连、断开、发送），桌面端不受影响。
  根因是一个端口名有两种写法（`display_name()` = `host:port`，`port_id()` =
  `network://host:port`），而 `workbench.rs` 里"这个名字是不是已注册网络端口"的判定散在
  **六处**（注册 upsert、注销 retain、断开、连接、`is_network_port`、重连），用的是**三种规则**：
  只有发送路径与注销认两种，其余只认一种。wasm 侧的网络端口行偏偏以 `port_id()` 为键
  （`crates/app/src/web/serial.rs::add_web_network_port` 实测如此），于是浏览器里点这一行：
  连接/重连/断开静默走串口后端（`COM` 枚举根本不认这名字），发送虽然路由到网络分支、也会在
  端口表查不到同名键而拿到 `PortNotOpen`。
  **更正一个先前的错误归因**：本轮取证时我把它记成"发得出、连不上"，实测两头都坏，只是坏法不同。
  收敛成**一条判定**（`network_config_matches` 与建在其上的 `find_network_port`）加**一个规范形**：
  凡写回应用状态、作队列键 `serial:{port}`、交给 transport 的名字一律 `display_name()`。
  规范化是五处（连接、发送、断开、重连、取消重连；最后一处不在原清单里，是改的过程中发现
  `reconnect_tasks` 的键会不同源才补的）。只统一判定不统一名字是不够的 —— 那会把"连不上"
  换成"连上了但发不出"，这一点由用例钉着。**native 无任何可观察变化**（它一直传 `display_name()`）。
  **先红后绿的证据**：新用例 `network_ports_connect_and_reconnect_in_both_naming_forms` 改前红在
  `left: Some("connect_serial") / right: Some("connect_network")`，改后转绿。另一条新用例
  `network_port_commands_in_the_id_form_share_the_canonical_port` 连的是真在跑的 loopback WebSocket，
  并且**不认任务 `Completed` 就算数**（`send_to` 是 fire-and-forget），它问对端有没有真的收到那帧
  `G28`。变异验证明白：把发送侧的规范化删掉，用例红在
  `串口 network://127.0.0.1:64341 未打开（可能已断开，请重连）`（主代理独立复跑过一次，不信报告）；
  把共用判定退回只认 `display_name()`，3 条用例红。**一条诚实的空白**：`cancel_reconnect` 的规范化
  没有任何用例钉得住 —— 造不出"重连正在进行中"的可达状态（`WouldBlock` 那条没有生产者，见第 9 轮），
  删掉它整套用例仍绿；这条写进 `docs/ARCHITECTURE.md` 第 12 条 ④ 而不是假装测过。
  四道门 exit 0（674 passed / 0 failed / 7 ignored，+2 条用例）。
- 桌面端最后 **9 处**把传输错误直接 `to_string()` 的后台任务出口接上中文映射器
  （`workbench.rs` 的刷新端口、申请端口、两条关闭、两条连接、两条发送、DTR/RTS 一条）：
  此前用户在工作台状态栏与红色任务标签上看到的是错误枚举的 `Display` ——
  `transport operation failed: 串口 X 未打开…`（平台英文外框套中文正文）或整句英文的
  `port '…' is not open`。现在同一个失败在两条投递入口上是逐字相同的一句中文。
  **这是纯接线**：`workbench.rs` 与 HEAD 的字面量、数字、注释多重集三项全等（各 84/25/130 条，
  增减都是 0），改动只落在 9 个表达式上。
  **覆盖是分两层的，别读成"9 处都有测试"**：6 处能在无头测试里真造出失败并逐字钉住文案
  （新增 5 条用例 + 更新 1 条），关键驱动手段是让网络 worker 停进一个"收下 WebSocket 握手请求、
  永不回话"的 loopback 对端 —— 那次握手读没有超时，`close_port_blocking` 就必定超时，于是
  「关闭」两条分支的正文也变成应用自己写的那句（`{端口} 正在关闭中`）而不依赖 OS 语言。
  剩下 3 处（刷新端口、申请端口、`open_network_serial`）在 native 上驱动不出失败，理由逐条写在
  用例抬头里，它们改由新的形状守卫 `crates/application/tests/transport_error_sites.rs`
  按函数体数映射器。**变异验证明白**：9 处逐处退回 `.to_string()`，6 处各自点亮对应的行为用例，
  3 处（794/810/1080）行为套件全绿、只有形状守卫变红 —— 那三处就是这份守卫存在的全部理由；
  还原后整包复跑 `675 passed / 0 failed / 7 ignored`，四道门全绿。
  两处诚实记录：`refreshing_and_requesting_ports_…` 那条在**有串口的机器上是空跑的**
  （两个任务都 Completed，用例自己的注释也这么说），真正钉住逐字中文的是
  `crates/platform/tests/transport_error_wording.rs`；串口打开失败的正文由 OS 给
  （本机 `IO 错误：系统找不到指定的文件。 (os error 2)`，换系统语言就变），所以那一格只钉
  「应用写的那半句之外不许有英文外框」，不钉 OS 文案。
  顺带留下一条没做的：`workbench.rs` 尾部 `AppRuntime::dispatch`（浏览器 composition root）
  仍在给已经中文化的正文加上 `AppError` 的英文 `transport: ` 前缀 —— 那是同步返回路径，
  本轮九处之外，记在 `docs/ARCHITECTURE.md` 第 12 条。
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
