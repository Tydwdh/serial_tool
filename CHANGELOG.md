# Changelog

本项目的重要变更记录于此。版本号遵循语义化版本。

## [Unreleased]

### 安全

- **可直接动手的清扫/收口批（第 60 轮）：发布构建开整数溢出检查、zip 越界条目留痕、正则缓存不再永久泄漏。**
  （只动配置与既有实现，无新 crate、无新 API；`Cargo.lock` 因死依赖删除重解析，净 −36 行。）
  * `Cargo.toml` 的 `[profile.release]` 增加 `overflow-checks = true`：此前发布构建的整数溢出按回绕
    静默算错，现在与测试构建一致地 panic。这是 2026-09-19 评审登记、长期未立项的加固项。
  * `updater::extract_zip` / `extract_zip_filtered` 对 `enclosed_name()` 为 None 的条目（绝对路径
    或 `..` 越界）从"静默 `continue`"改成**先 `log::warn!` 再跳过**：自更新路径此前连一条日志都
    没有，文件缺失无从追查。是否升级为硬失败未定（行为未变，仍跳过）。
  * `lua_host::api::serial` 的正则缓存由 `Box::leak` 改为缓存持有 `Arc<regex::Regex>`：清空发生时
    旧条目随引用计数释放；此前每轮清空都永久泄漏最多 256 个已编译正则。

### 调整

- **死件/死边清扫（第 60 轮）：2 条可证明的死 Cargo 边 + 3 处死件 + 两处 `unreachable!()`。**
  （生产行为零变化；测试入口改走生产路径。）
  * 删 `egui_extras`（workspace 声明 + `crates/app` 依赖边，全仓 0 处代码引用；`Cargo.lock` 随之
    消失 `egui_extras`/`enum-map`/`enum-map-derive` 三包）与 `tool-extension → tool-plugin-api`
    死边（0 处 `use`）。
  * 删 `crates/application/src/service/device.rs`（未被任何 `mod` 声明、不参与编译）、
    `TerminalItem::last_timestamp_ms()`（零调用）、`ReplayManager::load`（仅测试调用；测试改走
    生产两步 `prepare_load` + `load_prepared`）。
  * `transport` 两处运行期 `unreachable!()` 换成编译期穷尽：桌面 `send_control_blocking` 改收
    私有枚举 `BlockingControl`（新增 `SerialCommand` 变体不再需要运行期断言），网络 worker 的
    `let-else` + 内层 match 收成单层 `match`。

- **wasm clippy 从「只 lint 主程序一个包」扩到全部能在 wasm32 上编译的工作区成员（第 60 轮）**：
  `ci.yml`/`web.yml` 改为 `--workspace --exclude tool-extension --exclude tool-lua-host
  --exclude tool-marketplace --exclude tool-transport --exclude tool-updater`（这 5 个 crate 在
  wasm32 下编不过，getrandom 直接报错）。此前 application/platform/panels 等包的 wasm 分支
  只被当作依赖编译，不在 `-D warnings` 范围内。

- **两条从未自动化的回归接进 `cargo test`（第 60 轮）**：
  * 主题「未写颜色回落同族实色」补上有区分力的断言（第 59 轮自记的欠条）：无值时
    `card→mantle`/`input→surface0`/`text_white→text` 三条回落必须落地，显式写的值不许被覆盖。
    红证明：把回落目标指错字段 ⇒ 红在"没写时回落到同族"；把条件改成无条件覆盖 ⇒ 红在"显式值被保留"。
  * `plugins/template.serial-chart/tests/serial.scenario.json` 由
    `crates/extension/tests/serial_scenario.rs` 自动执行（此前只能手工跑 example）。
    红证明：改错场景里的期望值 ⇒ 用例红。

- **文档对账（第 60 轮）**：`docs/ARCHITECTURE.md` 三处过期陈述更正（分层判据去掉不可核对的
  `todo.txt §7`；seek 性能用例条目改记现状；两处"未接映射器"补闭合结论）；review-hardening
  计划归档并标注执行状态；`serial-plugin-scenarios.md` 记上场景已接进测试。

- **legacy 主题：把 `is_ok()` 换成"颜色与 dark_mode 真的落到了主题上"（第 59 轮）**
  （**只动 `crates/panels/src/theme.rs` 的 `mod tests`**，生产部分逐字节等于 HEAD。）
  `legacy_theme_is_accepted_but_future_schema_is_rejected` 对 legacy 那半边只判 `is_ok()` ——
  回归成"解析通过但一个颜色都没落 / dark_mode 被忽略"照样绿，而用户看到的就是换了主题结果全是默认色。
  邻居那条 `custom_theme_colors_override_the_selected_base` 钉的是**映射表本身**（直接调
  `apply_color_override`），并不覆盖"没有 base 的 legacy 文档能不能走到这张表"，所以这不是重复覆盖。
  夹具的 `dark_mode` 从 `true` 改成 `false`：**无 base 时 `inherited_dark` 本来就是 `true`**，
  写 `true` 就看不出这个字段有没有被读（本轮差点交一条恒绿的断言，靠"先怀疑判据能不能红"逮回来）。
  **红证明（两个变异各红在自己的那条断言上）**：M1 把 `bg_primary` 映射到别的字段 ⇒ 红在
  「legacy 的 bg_primary 没有落进 colors.base」；M2 把 `dark_mode` 改成只取继承值 ⇒ 红在
  「legacy 的 dark_mode: false 没被读出来」。`error[` 计数 0，变异逐字节还原后复跑 EXIT=0。
  **没做到的一条**：`parse_theme_source` 里"没写到的颜色回落到同族实色"那段（`card => mantle` 等）
  仍未被钉 —— 夹具只给一个 `bg_primary` 时 `mantle` 本身就是默认值，回落是否真的消掉了透明窟窿，
  要先确认 `ThemeColors` 的 `Default` 到底是什么（本文件里没有 `impl Default`，未核）再写断言，
  否则又是恒真判据。已开条目排在下一轮。

- **内置命令标题：把"全都非空"换成逐条判据 + "不许是纯 ASCII"（第 58 轮）**
  （**只动 `crates/app/src/command_registry.rs` 的 `mod tests`**，生产部分逐字节等于 HEAD。）
  原来那条 `assert!(titles.iter().all(|t| !t.is_empty()))` 两头都松：只剩空格的标题照样过，真红了也说不出是哪条命令。
  改成逐条判（消息点名 id），并补上"内置标题是中文正文"这一格 —— 判据用 `!is_ascii()`，
  与本仓既有的本地化标签用例同形（`crates/panels/src/plugins.rs` 的状态标签就这么判），
  **不为此再造第二份"含 CJK"的 helper**（第 53 轮那份留在 transport 测试里，两处各写一份就成了新的重复）。
  **红证明**：把切换右侧边栏那条的标题换成 `Toggle Right Dock` ⇒ 新判据红并点名该命令，
  而**旧的 all() 在它下面必然全绿**（英文也非空）；`error[` 计数 0，变异逐字节还原后复跑 EXIT=0。
  本机：该用例基线与还原后 EXIT=0，`cargo fmt --all --check` 与
  `clippy -p hardware-workbench-app --all-targets -D warnings` 各 EXIT=0。用例数不变（745）。

：把"非空"判据换成"承重命令必须在册"（第 57 轮）**
  （**只动 `crates/app/src/command_registry.rs` 的 `mod tests`**，生产部分逐字节等于 HEAD；提交 `c01efe4`，
  本条是它欠的 CHANGELOG 条目 —— 那个提交的信息里还写了"顺带补回空行"，其实当时插入脚本因引号问题根本没跑成，
  空行是这一次才真正补回的。）
  `builtin_ids_are_unique_and_prefixed` 今天这三条是「非空 + 去重 + 都带 `$` 前缀」—— 注册表**悄悄少注册一批命令**时
  三条全部照绿（剩 9 条仍然非空、仍然唯一、仍然带前缀），而用户看到的就是命令面板与快捷键表里少几项。
  改成逐个点名承重命令（`$CommandPalette`、打开串口、重连串口、清空终端、添加书签）必须在册，
  并删掉被它蕴含的那条 `!ids.is_empty()`。
  **红证明**：在 `$CommandPalette` 那次 `add` 之前提前 `return registry` ⇒ 新断言红，消息是
  「内置命令 $CommandPalette 不在册：它会从菜单与快捷键表里静默消失（当前在册 9 条）」，
  而旧三条在同一变异下**全部通过** —— 本轮买到的牙齿就在这条差集上。`error[` 计数 0 ⇒ 不是编译错的假红。
  本机：`cargo test -p hardware-workbench-app --bins builtin_ids_are_unique_and_prefixed` 基线与还原后 EXIT=0，
  `cargo fmt --all --check` 与 `clippy -p hardware-workbench-app --all-targets -D warnings` EXIT=0。
  用例数不变（745 条，本轮只换判据不加用例）。

- **不匹配的事件一点都不许进 backlog：给背压计数器补一条会红的判据（第 56 轮）**

  （**只动 `crates/databus/src/lib.rs` 的 `mod tests`**，生产部分逐字节等于 HEAD：整文件相对 HEAD 只多 948 字节，全在那一条新用例里。）压力门禁里那条
  `lossless_subscriber_receives_matching_events` 对字节的判据是 `assert!(sub.queued_bytes() > 0)`，
  它分不出"过滤生效"与"过滤失效但字节恰好算成 0"，而真正会咬人的回归恰恰是**给没订阅的事件也计费**
  （`queued_bytes` 与 `queued_events` 是背压告警、`subscriber_queued_bytes` 与 `seconds_behind` 的读数来源）。
  新增一条只喂不匹配事件的用例，钉住条数、字节、`drain()` 三项全为 0。
  **红证明**：把 `publish` 里的过滤判定改成恒不跳过（等价于"所有事件都入队"）⇒ 新用例红，
  既有用例同时红（不是只有它在响）；`error[` 计数 0 ⇒ 不是编译错的假红；变异后逐字节还原、复跑绿。
  本机：`cargo test -p tool-databus --lib lossless` 基线与还原后 EXIT=0，
  `cargo fmt --all --check` 与 `cargo clippy -p tool-databus --all-targets -- -D warnings` EXIT=0。

- **两端插件 Lua API 第一次被同一份脚本对齐：新增差分用例（第 55 轮）**
  （新增 `crates/plugin_runtime/tests/lua_api_parity.rs`（12 条"脚本 → 期望值"片段 + 4 条用例）与
  `crates/plugin_runtime/Cargo.toml` 的 `[dev-dependencies]` 三条边；**生产代码只改了一处文档注释**，
  没加 trait / 泛型 / 宏 / 新 crate，没动公共 API 与 serde 结构，三份 app 守卫一字未动。）
  native 走 `crates/lua_host`（mlua）、浏览器走 `crates/plugin_runtime/src/web_lua.rs`（纯 Rust omniLua），
  两边各自往 VM 上注册一组 `hw.*` / `ctx.*` —— 本轮之前**没有任何一条用例钉住"同一份脚本两端结果相同"**，
  而这正是插件作者实际依赖的性质。用例形状：脚本自己算出可判定的值（回传常量、算术、比字符串长度、
  数注册条目），不依赖墙钟、串口与协程。
  **它现在测得到的原因**（值得记，因为我自己此前记成"测不到"）：`web_lua.rs` **全文件零 `cfg` 属性**
  ⇒ 宿主平台直接编得过，`cargo test` 覆盖得到浏览器那半边；而该文件的模块文档原话是"本模块故意做了
  target 限定"，**与事实相反**（同仓另一处 `crates/application/src/web.rs` 才是真被 `cfg(wasm32)` 限定的）——
  已按事实改掉，并把"为什么不限定"的理由写进去：一旦限定，浏览器半边就退化成"只有 wasm clippy 在看"，
  两端分叉再也抓不到。
  **红证明（五个变异，逐个 `cmp` 还原 + 强制重建）**：M1 期望值全体加后缀 → 12 条全红（消息区分"两端一致
  但得出的不是声明的期望值"）；M2 web 侧改名 `ctx.bus.subscribe` / `ctx.serial.expect` → 红在
  `bus_exposes_the_same_function_names`、`serial_exposes_the_same_function_names`（消息直接打印两端串差）；
  M3 native 语义漂移三处（`storage` set 多写一字符 / keys 多报一键、`commands` 删掉缺参守卫）→ 红在
  对应三条；M4 web `hw.codec.xor8` 末位取反 → 红在 `codec_module_require_and_xor8_fold`；
  M5 web `ctx.ui` 名表改名 → 红在 `ui_exposes_the_same_function_names`。
  **立场：差分测试只钉"确实一致"的部分。** 它一次量出 **12 处两端分叉**（`ctx.plugin`/`ctx.context` 三个
  装配点两种形状、`ctx.bus.wait` 在浏览器里顶层必失败、`hw.codec`/`hw.utils` 实际是两个不同的模块 ⇒
  `from_hex`/`crc16_modbus` 在浏览器里是 nil、`format_size` 单位词 `KB` vs `KiB` 等，逐条带行号记在 #84）。
  这些**故意没写成绿用例** —— 把现状钉住等于把分叉固化；收敛任一条都会改变插件脚本行为，归用户拍板。
  也**没有**为跑通而放宽判据：0 处 `#[ignore]`、0 处恒真断言，`ctx.task` / `ctx.config` / `ctx.dialog` /
  `ctx.fs` / `ctx.now_ms` / `ctx.serial` 的 read-expect 路径驱动不了这件事写进了新文件的模块文档，
  免得后人再往里塞假绿用例。
  五道门全 EXIT=0（fmt / 整包 clippy `-D warnings` / wasm clippy / 整包 test / Pressure gates；文档改动之后
  又单独复跑 `cargo fmt --all --check` 与 `clippy -p tool-plugin-runtime --all-targets`，各 EXIT=0），
  整包用例实测 **740 → 744**（+4 条差分用例），0 failed / 7 ignored。

- **命令面板的"最近使用"两端终于用同一个键；native 那两份逐字相同的端口条目投影收成一份（第 54 轮）**
  （动 `crates/app/src/{config.rs,ui/command_palette.rs,ui/mod.rs,ui/top_bar.rs,ui/device_panel.rs,web/mod.rs}`
  与 `crates/app/tests/architecture.rs`，净 `+304 / −75`；**serde 字段名与属性一字未动**，不加 trait / 泛型 /
  新模块，不改公共 API —— `native_port_items` 是 `crate::ui` 的私有方法。）
  **（1）为什么改键**：native 把"最近使用"记成**展示标题**（`entry.label`），web 记的是**命令 id** ⇒ 两端键空间
  分叉，而且标题一改（插件命令改名、文案微调）用户的使用历史就**静默失配**。现在 native 的记录、持久化、排序
  全部走 id，派发出口从 `Option<(String, String)>` 收成 `Option<String>`（成对带出标题的写法是病根，不是巧合）。
  **（2）代价，按未拍板处理**：老用户 `workspace.json` 里按标题记的历史从此匹配不上 ⇒ 面板回到"按分类分组的
  注册顺序"（确定、不随机，有用例钉住），重新用几次即恢复；**不删数据、不写迁移**（那要改写用户设置，归用户拍板）。
  还有一条要一起拍：这个列表在 id 方案下**不再向"只用过的命令"收敛**（旧标题项永久滞留，`retain` 只按 id 去重），
  web 一直是 id 所以没这问题。
  **（3）真实重复的位置和我的记录不一样**：原队列记的是"`web/serial.rs` 与 `panels/serial.rs` 两份逐字相同的
  端口条目投影"——**位置记错了**。实测：真存在的逐字重复在 **native 两处**（顶栏与设备面板各一份 20 行
  `SerialPortItem` 投影，去空白与路径前缀后 321 == 321 == 321 字节），已收成 `WorkbenchApp::native_port_items()`
  一份（净 −75 行）；而 web 那两处**并不逐字相同**（`kind` 取值 `"网络"/""` vs `"Serial"/"网络"/""`，三个旗标
  来源也不同），并它要么改变可见行为、要么把展示规则做成参数开关 = 新增抽象层 ⇒ 不合，理由写成
  `native_port_items` 的文档注释。（`panels/src/serial.rs` 里那 10 处 `SerialPortItem` 字样是消费侧与类型引用，
  本轮没有逐行复核它是否也构造 —— 记作待复核。）
  **（4）覆盖与牙齿**：合并前先撞了一把 —— 把那两份投影改成假值（`pending_reconnect: false`、`kind: ""`）后
  **现有用例全绿**（79 passed / 0 failed）⇒ 这份"用户可见的端口条目"此前**零覆盖**。本轮交付：2 条行为用例
  （最近使用排同分类最前 / 旧的标题值不影响顺序且保持确定）+ 1 条解析侧用例（历史值原样载入，不许裁剪）
  + 2 条形状守卫（两端"记录"与"读取"必须同一键空间；native 投影只许一份且住在 `src/ui/mod.rs`）。
  **守卫自己被变异撞出一个洞**：M6 第一次打全绿 —— 判定做了前缀匹配，把载入侧改成 `into_iter().filter(...).collect()`
  照样算命中；收紧成带分号的完整形态后 M6'/M7 才红。同理，M1 在源码里留了一条**诱饵注释**（内容正是守卫要找的
  `used == &entry.id`），守卫仍红 ⇒ 注释喂不出假绿。
  **（5）做不到的照实说**：web/wasm 半边运行期测不到（`#[cfg(target_arch = "wasm32")]`，全仓 `wasm_bindgen_test`
  为 0；M5 那一轮"只有形状守卫红"就是直接证据），`native_port_items` 合并后也仍无运行期覆盖（要覆盖得能给面板喂
  `CreationContext`）。另量出一处没动的规则重复："带 `network://` 前缀即网络口"有 3 个入口
  （`platform/src/lib.rs:105` 产生、`application/src/transport.rs:40` 与 `app/src/web/serial.rs:225` 匹配），
  收成一份要在 `tool-platform` 的 `PortId` 上加 inherent 方法 = 动公共 API ⇒ 本轮硬约束排除。
  五道门整包复跑全 EXIT=0（`target/gates-r55-{fmt,clippy,wasm,test,pressure}.log`，test 门 0 条 warning/error 行），
  整包用例实测 **735 → 740**（+5 = 2 条行为用例 + 1 条解析侧用例 + 2 条形状守卫，逐 target 三数合计）。

- **同一端口连点两次连接，前一个循环会永久失主 —— 两端"登记前先取消上一个"这条规则现在只有一份（第 52 轮）**
  （动 `crates/application/src/{task_model,workbench,web}.rs` 与 `crates/application/tests/headless.rs`；
  不改公共 API、不改 serde 字段。native 早有这道保护，**web 的四份连接/重连收尾没有**：
  `reconnect_tasks` 那张表被 `insert(port, id)` 就地覆盖后，那个被盖掉的 id **再没有任何地方能点名取消**
  —— 终态快照的清理按 id 比对（`retain(|_, id| *id != snapshot.id)`）认不出它，只有
  `Connected` / `Disconnected` 会按端口 `remove` ⇒ 用户看到的是同一端口上多了一个停不掉的后台循环。）
  **做法**：把"要不要动手、动谁"这一格收成 `task_model::cancel_deregistered_task(Option<TaskId>, impl FnOnce)`，
  两端各自的容器（native `HashMap<String, TaskId>` + `TaskManager::cancel`，web
  `Rc<RefCell<BTreeMap<PortId, TaskId>>>` + `WebApplication::cancel_task`）连键类型都不同，
  所以**摘出来与存回去仍留在调用点**，只共用这一句判定 —— 这是本轮唯一新增的抽象，它服务的是一份
  跨端规则而不是"将来可能有用"。web 侧新增 `register_reconnect_task` 承接四份收尾的重复段。
  **红证明**（`target/r54-verify.py`，基线→变异→逐字节还原→复跑）：把该函数改成空操作（`let _ = registered;`）
  ⇒ 新用例 `reconnecting_the_same_port_twice_cancels_the_first_loop` 红在 `headless.rs:2114`，
  `error[` 计数 0 ⇒ 不是编译错的假红 ⇒ 这道保护确实被用例驱动着。
  **顺带**：`headless.rs` 两条收尾预算循环此前走完就直接 `remove_dir_all`（`break` 有条件但没人对结果负责，
  后台线程没收割干净用例照样绿），补成同文件姊妹形状（`reenabled` / `window_closed`）那条带消息的
  `assert!`；这两处判据靠"条件满足"退出。**负载实测分两层报**：批次 A 那两条 200×5ms 收尾断言在
  hog=cpu×4 与 churn×6 下各 **0/8** 红，主代理自己空载背靠背 3 条用例 ×10 轮 **30/30**；
  新增那条重连用例的三段等待不是 200×5ms 而是 `const BUDGET = 30s`（同文件
  `disconnecting_a_port_that_will_not_finish_closing_reports_chinese` 用的同一个数），满载 mean 9.2s / max 11.8s。
  仍然**只欠一个数**：一次通过的收尾循环实际吃掉 200 次里的几成（0/16 只说明"没红"，说明不了余量）—— 记在队列里。
  **交付里还有半件我上一版没写**：删掉 web 侧三个零调用者的函数（`task_snapshots`、
  `replay_set_analyzer_warning`、`replay_clear_analyzer_messages`）—— "零调用者"由编译本身证明（wasm clippy
  门禁 EXIT=0，删的是唯一那份生产者，native 同名函数与它的调用链一个字没动）。
  **由此量出第 12 条待拍板**（先前就在，不是本轮造成的）：`ReplayState.analyzer_warning` 这个字段在浏览器侧
  **根本没有生产者** —— 面板照读（`panels/src/replay.rs` 的 `if let Some(warning) = &status.analyzer_warning`）、
  web 照转（`application/src/web.rs:650`），但唯一能写它的那个 web setter 从来没人调用；native 在
  `replay_task.rs:239-246` 会因"部分事件重放失败"写出 warning 加一条 Warn 状态行，浏览器里同样的结果
  一个字都不提示。补它=新增用户可见行为，归用户拍板，**别当死代码继续删**（这次删的是零调用者的那份，
  不是这条回路）。
  **如实记下的没做到处**：web 那半边的运行期行为只有编译与形状能保（`application/web.rs` 是
  `#[cfg(target_arch = "wasm32")]`，本仓没有 `wasm_bindgen_test`）；新增的"登记前先取消上一个"在浏览器里
  有一个可感后果（第二个连接会瞬时被取消，其 `TaskCancelled` 事件会把 `view.connecting` 置回 false 并写
  "任务已取消"，直到新任务自己的事件到达）—— 这是补这道保护的固有代价，需要浏览器核实，已并入 #14 那条待核实。
  五道门整包复跑全 EXIT=0（fmt / 整包 clippy `-D warnings` / wasm clippy / 整包 test / Pressure gates）。

- **`translate_error` 的三条 `!is_empty()` 换成有区分力的判据，并补上从未被驱动的两格（第 53 轮）**
  （**只动 `crates/transport/src/lib.rs` 的 `mod tests`**，生产代码零改动：`translate_error` 本体一行未改，
  新用例只是把已有行为钉住。）原来那条叫 `translate_error_covers_all_variants` 的用例，对 `NoOpenPort` /
  `WorkerClosed` / `QueueFull` 三格只断言 `!is_empty()` —— **任何非空字符串都能过**，包括把臂删成
  `err.to_string()` 之后那口英文 `Description(...)`；而"用户看到的是中文"正是第 10、22、25 轮收过的那条契约。
  改法是加一个 5 行的 `contains_chinese`（只在测试模块内）断言文案含 CJK 字符，另加一条新用例钉
  `Io` 臂里此前**一处用例都没有**的两格 `WouldBlock` / `InvalidData` —— 它们的契约恰恰是**不许**再包一层
  中文（包了就出现「IO 错误：COM3 正在关闭中，请稍后重试」这种两套说法叠在一起的用户可见文案）。
  **红证明（两个变异都红在我新写的断言上，`error[` 计数为 0 ⇒ 不是编译错的假红）**：M-A 把 `QueueFull`
  改成英文 `"serial write queue is full"` → 新断言红，而**旧的 `!is_empty()` 在它下面必然全绿**（英文也非空），
  这就是本轮买到的那条牙齿；M-B 把 `WouldBlock` 改成 `format!("IO 错误：{message}")` → 新用例红。
  **一处必须记下的自我更正**：我原本的判断是"`Io` 臂只在 linux 有用例、Windows 侧 0 覆盖"，**M-B 的第一版
  打在 `TimedOut` 上时同时红了既有的 `translate_error_keeps_the_timeout_and_generic_frames_for_other_kinds`**
  ⇒ 那三格（权限/超时/泛型）早就有 Windows 跑得到的跨平台用例，我差点把重复代码当缺口入库。据此把新用例
  **收缩**到只剩真正没覆盖的两格，并把写错的注释一并改掉（用例数实测：`translate_error*` 一族 4 条 → 5 条，
  `cargo test -p tool-transport --lib translate_error` 报 `running 5 tests`；整包总数本轮**不引**，
  那次全绿跑的是"改前的第 53 轮 + 批次 A 在飞的用例"，735 里混着别人的增量，说不清就别写）。
  `Serial(sp::Error)` 变体仍然驱动不了：全仓没有一处构造 `sp::Error`（唯一入口是 `#[from]`），本轮不为它编造
  构造方式，已在用例末尾如实注明 —— 要补它得先决定用什么方式造，那是另一件事。
  本机核过：`cargo test -p tool-transport --lib translate_error` 基线/还原后 EXIT=0、
  `cargo clippy -p tool-transport --all-targets -- -D warnings` 与 `cargo fmt --all --check` EXIT=0；
  整包五道门在第 50、51 轮那次已全绿（`8c05fc0`），本轮改动只在这个基础上加测试目标。

- **豁免棘轮补上两条它原本看不见的静默通道（第 51 轮）**（**只动 `crates/app/tests/lint_exemptions.rs`**，
  169 行全是**加强**守卫的新增断言，一条豁免都没退）：原本那条守卫只数源码里的 `#[allow(...)]`，而
  ① 往任意 `Cargo.toml` 写一段 `[lints.clippy] … = "allow"`、② 往 `.cargo/config.toml` 的 rustflags 塞
  `-A` / `--allow` / `--cap-lints`，都能在不改一行源码的前提下让全仓某个 lint 静默，且**四道门与那条棘轮照常绿**
  ⇒ "豁免只准减、不准悄悄加"的承诺本来是漏的。今天这两条通道实测都是空的（逐份扫过：全仓无任何 `[lints]` 声明、
  无 `clippy.toml`、rustflags 只有 `--cfg=web_sys_unstable_apis`）——**"此刻为空"正是钉住它的时点**。
  **两条新断言各自带正向对照**（先喂一段人造的坏文本，认不出来就直接红），因为"扫到 0 条"既可能是仓库干净也可能是探测器坏了；
  写第一版时就靠它抓到自己用前缀匹配会**整段漏掉 cargo 的标准写法 `[workspace.lints.*]`**，遂改成按 `.` 拆段找 `lints`。
  **红证明（三个独立变异，全红且都是断言红不是编译错）**：M2 根 manifest 追加 `[workspace.lints.rust] unused = "allow"`
  → 红，报出 `"Cargo.toml": workspace.lints.rust → unused = "allow"`；M3 把已有 wasm rustflags 数组扩成
  `[…, "-A", "dead_code"]` → 红，报出 `-A ← rustflags = …`（写成重复表头会是 TOML 解析错，故改数组；放宿主用不到的
  wasm 段是为了零重建）。成员 manifest 那一半的遍历由守卫自带的 `manifests.len() >= 10` 兜住（根只贡献 1 份）。
  **没守住的如实记下**：`vendor/egui_tiles/Cargo.toml` 不在扫描路径里，那里加 `[lints]` 仍看不见 —— 那是约定不碰的第三方
  目录，要扩就下一轮单独扩。（五道门 fmt/clippy/wasm/test/pressure 全 EXIT=0；test 门 32 条 `test result:` 行合计
  **733 passed / 0 failed / 7 ignored**，比第 44 轮的 731 正好多这两条新用例。）

- **CI 真正在跑的那道压力门禁里，把一条几乎不会失败的断言换成真检查（第 50 轮）**（**只动 `crates/panels/src/virtual_list.rs`
  的 `mod tests`**：`#[cfg(test)]` 之前 8737 个字符逐字节核过与 `git show HEAD:` 相同 ⇒ 生产代码与用户可见行为一个字没改。）
  `pressure_50k_rows_stays_indexed` 原来守"行索引保持完整"只有 `assert!(visited > 0)` —— **任何非空索引都会 >0**，
  只有"每个 offset 都返回空窗口"才红。换成两条：① `total_height` 等于夹具算术值 `2941×32 + 47059×16 = 847,056`
  （已核 `total_height()` 就是 `fenwick.total()`，不含 viewport/padding；全部高度都是 16 的倍数、部分和远小于 2²⁴
  ⇒ f32 里精确可表，因此**故意不写容差** —— `f32::EPSILON` 在 8e5 量级低于 ulp，写了只是自我安慰）；
  ② 对每个采样 offset 断言**窗口真的盖住视口**：首可见行的顶不晚于 overscan 起点、末可见行的底不早于视口底，只用已有的
  `row_top()` / `height()`。**不在测试里重算一遍 `index_at_offset`** —— 那等于给被测算法留第二份副本，迟早和实现各说各话
  （第 32 轮删临时几何用例就是这个理由）。`elapsed` 仍然只 `println!`，不进判据。
  **红证明**：M1「首行高度没进前缀树」（`sync_rows` 里喂给 `from_values` 的数组把 h[0] 归零）→ 新断言红在
  `left: 847040.0 / right: 847056.0`（差的正是那 16），而**改前那条旧断言在它下面必然全绿** —— 这就是"牙齿"的具体形状。
  锚点要求命中数恰好为 1 才打补丁，否则会打到 `sync_ids` 那份同形代码上（那里确实有一字不差的重载）。
  压力门同次复跑：tool-panels 1 passed、tool-databus 1 passed。

- **回放 seek 的那条"1 万条事件 < 1 秒"换成同一次运行内的缩放比 —— 旧尺子既在负载下假红、又在平方级回归下没牙（第 44 轮）**
  （**只动 `crates/recorder/src/replay.rs` 的 `#[cfg(test)]`**：主代理逐字节核过 `#[cfg(test)]` 之前的 34,467 个字符
  与 `git show HEAD:` 完全相同 ⇒ 生产代码与用户可见行为一个字没改；树上无变异残留。四道门
  fmt / native clippy `-D warnings` / wasm clippy / test 由**主代理自己重跑**，全 EXIT=0，
  **731 passed / 0 failed / 7 ignored**、32 条 `test result:` 行 —— 用例数与基线 731 一致（本轮只改一条用例的判据）；
  **CI 的第五道门 `cargo test -p tool-panels -p tool-databus -- --ignored` 第一次在本机跑，两条压力用例都绿**
  （`pressure_3mbps_rx_publish_and_drain`、`pressure_50k_rows_stays_indexed`；4 个目标里 2 个本来就是 0 测试的 doc 目标）。
  **（1）为什么换**：旧断言 `elapsed.as_millis() < 1000` 钉的是绝对墙钟，两头都不占。① **假红**：第 39 轮
  同一 hog 配方量到 churn×4 **6/10**、churn×6 **2/8** 红，本轮改前那批又量到 **1/10**；② **没牙**：它自己的注释
  写着"O(n²) 的旧实现在 100k 事件时才会显著变慢"。本轮把旧尺子原样贴回去、在**人为平方级变异**下实测 **93~96ms**
  ⇒ **旧尺子全绿**，这是"没牙"的直接证据。
  **（2）换成什么**：一份 20k 事件的文件只 `load` 一次，靠 seek 到递增位置得到三档 **5k/10k/20k**
  （相邻两档覆盖数正好翻倍 ⇒ 比值取 log2 就是增长指数），每档 K=8 次采样取**最小值**，且**按轮交错**
  （一轮跑完三档再进下一轮；否则"小档赶上风平、大档整段撞上 hog"会把比值凭空抬高）。K=8 是量出来的：
  K=5 在 churn×4 下出现过整档被饿住、min 单调性反转。
  **（3）两条独立估计量、各管一头**：`MIN_RATIO_EXPONENT_CEIL = 1.75`（各档 min 的相邻比值）管牙齿
  —— 线性侧最坏观测是空载 1.12 / 三种负载 1.47~1.55，平方侧变异最低 1.99，上下都留了余量；
  `EVERY_ROUND_EXPONENT_CEIL = 1.50`（每轮各算一次比值再取最小）管**绝不因负载假红** —— 负载只能抬高比值，
  它要越界就意味着**每一轮**都看见超线性增长。
  **（4）四条防空转断言全部排在比值断言之前**：每档 `count == covered`（实测 24/24 全等）、每档 min 非零、
  相邻档 min 单调不减（消息写明"这是尺子坏了，不是实现变快了"）、外加一次满量程预热并断言它重放满 20k 条。
  没这些的话 fixture 一旦没喂进数据，三档会一起退化成同一个最小值、比值恒为 1 —— 就是"两边都是空态"那种假通过。
  **（5）变异矩阵，含没抓到的**：M1「每次发布前从 0 重扫前缀」exp **1.92~2.23** → 红；M1b「前缀纯累加」
  **1.99~2.08** → 红；**M3「数据阶段多扫三遍并 clone」与 M4「每次 seek 重建跳表索引」≤1.08 → 绿，即没抓到**
  —— 两者仍在线性内、只放大常数（实测 21~26ms），旧尺子对它们同样无效（离 1000ms 差 40 倍）。
  **这把尺子只判形状**，这是它换取"不因机器慢而假红"的代价；要守常数回归就得钉绝对时间，而绝对时间正是本轮删掉的。
  **（6）代价，实测**：`cargo test -p tool-recorder` 空载整包墙钟 **0.09~0.10s → 1.34s**（中途一档 1.83s），
  约 13 倍，全部来自这一条用例（25 次 seek ≈ 28 万条事件重放 + 一次 20k 行 fixture）；64~96 路 hog 下整包
  mean 31s / max 85s。买回来的是"这条断言真的会红"。已为此收过一次规模：最大档试过 40k（K=5）单条 1.81~1.83s，退到 20k/K=8。
  **（7）负载复测与一处必须说清的诚实问题**：主代理在 01:18–01:19 跑过只读普查脚本，**污染了子代理那一批测量的 CPU 环境**，
  故以停手之后重跑的为准 —— churn×64 与 ×96 各 8 轮背靠背交替，**改前 0/8、改后 0/8**。
  ⇒ **不能说"改后 0 红"证明了改进**：同一台机器这两个配方下**旧尺子也 0 红**（本机 ambient 负载会漂）。
  旧尺子会假红的证据是第 39 轮的 6/10 与本轮 churn×4 的 1/10；本轮真正立得住的新证据是（1）里那条变异实验 ——
  **旧尺子在平方级下全绿**。
  另记一条先前存在的小瑕疵（本轮没动）：用例最后一行才 `remove_dir_all`，断言失败时临时目录会漏在 `%TEMP%`；
  旧版本同形，但 fixture 变大 + 连跑十轮把代价放大了。

- **删掉 native-only 的重复终端摄入管道：Application 里那份 `TerminalService` 连同它的读模型
  （净 −302 行 = 岛 172 行 + 装配点 + 只测它的两条用例；第 46 轮）**（本轮只动 `crates/application`
  一个 crate：`src/service/terminal.rs` **−145**（整删）、`src/model/terminal.rs` **−27**（整删）、
  `src/model/mod.rs` **−3**（整删，连 `model/` 目录）、`src/service/mod.rs` **−2**、`src/lib.rs` **−2**、
  `src/workbench.rs` **+9/−22**（多的 9 行是就地写清「为什么这三个分支不再动用户看到的东西」的中文注释）、
  `tests/headless.rs` **+6/−114**、`Cargo.toml` **−1**（`parking_lot` 依赖边）、`Cargo.lock` **−1**；
  不新增 trait/泛型/宏/模块/依赖，不加 `#[allow]`/`#[ignore]`，豁免计数仍 **5**；
  两份架构守卫（`crates/app/tests/architecture.rs`、`crates/application/tests/architecture.rs`）
  **一行未改**：它们的计数断言全是 `>=` 下限且按 *manifest* 统计，纯删源文件不触发（先跑一遍确认，
  不是推断）。四道门 fmt / native clippy `-D warnings` / wasm clippy / test 全 **EXIT=0**，
  **731 passed / 0 failed / 7 ignored**、32 条 `test result:` 行 —— 基线 733 **净减 2 条**，
  就是下面 (4) 点名的两条只测幻影管道的用例，没有别的差额。
  **（1）可达性取证（动手前重跑一遍，不是照抄简报）**：`grep -rnw` 逐符号 —— `TerminalService`
  3 处（自身定义 2 + `workbench.rs` 的 import/字段/构造）、`TerminalDelta`/`TerminalSeq`/`TerminalEntry`
  只在 `model/terminal.rs` 与 `service/terminal.rs` 内部互引 + `workbench.rs` 的返回类型、
  `query_terminal_since` **9 处调用全在 `tests/headless.rs`**（简报记 8 处，少算了同一条 assert 里的
  第 9 个调用）、`model::terminal`/`service::terminal` 作为路径 0 个外部使用者。岛内成员互相调用看着
  "像活的"，所以入口从外面追：唯一的进口是 `Workbench::new/with_config/tick/dispatch/query_terminal_since`，
  而这五个口子对终端做的事**只有一份幻影 store**。**根因（为什么四道门从没抓到）**：
  `service/terminal.rs` 挂在 `#[cfg(not(target_arch = "wasm32"))] pub(crate) mod terminal;` 上、
  `model` 整个模块也是 `cfg(not(wasm32))` —— wasm 那道 clippy 根本编不到它，native 那道 `dead_code`
  又因为它被 `tick`/`dispatch` 调着而必然沉默。**（2）两项设置在两端都还活着（这是能不能删的关键）**：
  真正渲染的 `crates/panels/src/terminal.rs::TerminalPanel` 有自己的
  `subscribe_ring_bounded(TopicFilter::prefix("transport.serial."), 65_536)` 与自己的 `TerminalStore`，
  `terminal_max_entries` 经 `set_max_entries`→`max(100)`→`enforce_max_entries`→`TerminalStore::set_max_entries`
  生效，`terminal_merge_window_ms` 在 `push_event` 里每事件同步成 `assembler.idle_finalize_ms`；
  native 三个入口 `app/mod.rs::apply_persisted_panel_settings`（启动回填）、
  `config.rs::load_config_from_path`（重载）、`ui/settings_panel.rs`（用户改值），持久化则反向
  **从面板读回**（`config.rs::build_config_snapshot`）；web 两个入口 `web/mod.rs` 的
  `dispatch(TerminalSettingsChanged)` 与设置卡片。**结论：幻影不是任何一项设置的唯一消费者**，
  它只是同一份设置的第二份落点、同一批字节的第二次装配。**（3）断言走的是「连代码带用例一起删」(a)**，
  证据：`entries_since(since_seq, limit) -> TerminalDelta{next_seq,truncated,dropped}` 这种
  「调用方持 seq 游标分页拉取」的读法**只存在于幻影服务**（全仓 `entries_since` 2 处命中都在被删文件里）；
  面板侧唯一相近的是导出走的 `TerminalStore::iter_after(Some(last_id))`，那是"多帧扫大 store 做导出"，
  不是同一条对外读取契约 —— 也就是说这两条用例守的是**一个产品代码从来不用的特性**，
  把它"移植"到面板等于给面板补一份它没有的 API（新抽象 + 新依赖边，越界）。且 `tool-panels`
  依赖 `tool-application`、反向没有边，`headless.rs` 根本指不到活面板。**（4）删掉的具体用例与它们
  原本守什么**：`terminal_delta_is_incremental`（守「带 `next_seq` 复查不得重投已消费条目」）、
  `terminal_bound_clamps_max_entries_and_pages_incrementally`（守四件事：`SetTerminalMaxEntries{max:5}`
  被 `max(100)` 夹取、超限按 seq 淘汰最旧、`limit=1` 必须标 `truncated`、`ClearTerminal` 后游标不回退
  且新条目 seq 继续递增）；`headless_workbench_can_dispatch_and_query` 只摘掉其中 4 行
  （`query_terminal_since(0,100)` 的三条断言 + 那句注释），命令可派发/`query_*` 可用/无效连接终态
  都还在。**仍然有牙的部分没丢**：稳定 seq 与「清空后 ID 不复用」由 `TerminalStore` 自己的
  `clear_keeps_terminal_ids_monotonic_for_incremental_clients` 守，条数上限与按 seq 淘汰由面板的
  `max_entries_is_global_across_ports_and_keeps_newest_events`、`lowering_max_entries_trims_immediately`
  守（三个名字都逐字核过存在）。**（5）实测数字，不是推断**（临时插桩 + 临时夹具，跑完全部回退并
  `cmp`/`md5sum` 逐文件验回原状；夹具是同一条事件流、600 帧）：插桩单测里幻影每帧取走的事件数与
  store 增长 = 空载 **0 件 / 0 条**、115200 连续流（1×192 B/帧）**600 件 / 600 条**、突发
  （8×1 KB/帧）**4,800 件 / 4,800 条**，即"面板收多少它装配多少"；同一构建里两者耗时同量级
  （release：幻影 1,487 vs 面板 1,417 ns/帧；14,437 vs 13,479 ns/帧）。同一夹具在两个构建里量
  `Workbench::tick()` 每帧开销（release）：空载 **74.0 → 79.8 ns**（这一行是噪声，删除的只是 45 ns
  的空 drain）、115200 流 **1,501.3 → 47.5 ns**、突发 **14,862.7 → 46.3 ns** —— 也就是
  **application 层的每帧终端开销从「与面板同量级」降到与流量无关的 ~50 ns**（debug 档给的是插桩
  单测里那个被删调用自身的开销：167.8 / 5,212.0 / 55,004.7 ns/帧，与面板同档实测 74.8 / 5,427.2 /
  56,668.7 ns/帧 —— 两份工作确实同量级）。**另一笔是启动即分配**：`subscribe_ring_bounded` 用
  `VecDeque::with_capacity`，实测 `size_of::<Event>() = 136` → 那条重复的 65_536 环在 `Workbench::new`
  当场占 **8,912,896 B ≈ 8.5 MiB**，删掉就没了（面板自己那条照旧）。所以这次既省每帧第二份装配、
  也省 8.5 MiB + 第二份 50_000 条 store；说成"纯性能胜利"不诚实：空载会话里它本来每帧只花 45 ns，
  真正的收获是有流量时的第二份装配与那两份内存。**（6）用户可见变化：无。** 终端内容、两项设置的
  生效、面板工具栏的「清空」两步确认、导出、录制回放全部走原路径（面板那份代码在删前删后逐字相同）。
  唯一"本来就不一样"的是 native 命令面板/快捷键的 `$ClearTerminal`：它此前只清那份没人读的 store，
  用户看到的面板同样不动 —— 删除后该分支只剩 `Ok(Done)`，**行为对用户逐字不变**，但这笔账被如实
  写进 `workbench.rs` 的注释里（wasm 侧靠 `notify(TerminalCleared)` 让组合根清面板，桌面侧没有这条回路）。
  **（7）三笔顺带查出、本轮按「一轮一个域」没动的账（需orchestrator拍板）**：
  ① native 的「清空终端」命令历史上是空转 —— 补这条回路（组合根收到 `Done` 后清面板，或像 web 那样
  发事件）是**产品决定**，不是机械修线；② `ApplicationConfig::terminal_merge_window_ms`/
  `terminal_max_entries` 现在**只写不读**（native 无人派发 `SetTerminalMergeWindow/MaxEntries`，
  启动值由组合根直接落面板），要不要连字段一起收掉涉及 `crates/app::application_config_from`，
  超出本轮单 crate 边界；③ `crates/application/src/service/device.rs` 是一份**没被任何 `mod` 声明**的
  遗留文件（`DeviceService` 全仓 0 引用，连编译都不参与，11 行），以及 `TerminalItem::last_timestamp_ms()`
  本轮普查里 0 调用者（本轮之前就已死，不是被这次删除变成孤儿的，故未动）。**（8）文档**：
  `docs/ARCHITECTURE.md` 的「Terminal 高频特殊处理」重写成「Terminal 高频数据路径（native 只剩一条）」，
  记 cfg 根因 + 上表实测数字，分层表/`Command·Query·Event`/「如何新增能力」第 2 步里指向
  `TerminalService`、`query_terminal_since` 的四处一并改掉，第 9 条 (b) 那颗"先记着"的疑问就地还账；
  「注释里提到 egui 有 4 处（含行号）」那条改成按符号核对、行号会漂（现 3 行）。CRLF 逐字节保持
  （1421 行全 CRLF，无裸 LF）。`headless.rs` 那条钉 `docs/ARCHITECTURE.md` 发布文案的用例钉的是
  HEX 严格模式那句，本轮没碰那一栏，两份文档也没有被任何测试从磁盘读取（已 grep 核实）。）

- **回放的三条 io 出口不再把 OS 的 `kind()` 折成 `Other`：框架点名路径、冒号改全角
  （第 40 轮记下的那笔同类账，本轮平掉）**（本轮动 `crates/recorder/src/replay.rs`
  的 `prepare_load` 两条出口 + `prepare_load_lines` 一条 + 它的 `#[cfg(test)] mod tests`；
  签名与 `io::Result<ReplayLoadData>` 不变、不新增类型/trait/宏/模块、不加 `#[allow]`、
  豁免计数仍 5；**另动了 `crates/application/tests/headless.rs` 里那条逐字钉回放文案的用例**
  —— 它是第 25 轮立的 crate 外文案契约，生产者改措辞它必红，简报的"只动 recorder"与
  "四道门全绿"在这里互斥，取"如实更新断言、一条不删"（详见下 (4)）；四道门 fmt /
  native clippy `-D warnings` / wasm clippy / test 全 **EXIT=0**，**733 passed / 0 failed /
  7 ignored**、32 条 `test result:` 行 —— 基线 732 **净增 1 条**
  （新增 `native_load_exits_keep_the_kind_of_the_underlying_os_error`）。**（1）三处的形状**：
  `获取文件大小失败: {e}` / `回放文件打开失败: {e}` / `回放文件读取失败: {e}` 全部由
  `io::Error::other(..)` 改成 `io::Error::new(e.kind(), ..)`，框架换成
  `获取文件大小 {} 失败：{e}` / `打开回放文件 {} 失败：{e}` / `读取回放文件 {} 失败：{e}`
  （路径来自各自的 `path`，OS 细节原样留在冒号后）—— 半角 `: ` 是同一个 crate 里
  兄弟文件对同一模式用两种标点，`recorder.rs` 第 40 轮的新框架与 `tool_updater` 全用全角。
  **（2）用户可见串的前后**（本机中文 Windows，逐字取自两次真跑：改前/改后各跑一次
  临时探针，跑完即删；探针打的是 `prepare_load` 的返回值，派发侧 `dispatch_load_replay`
  对它只做 `to_string()`，即任务消息正文本身）：
  取大小（路径不存在）`获取文件大小失败: 系统找不到指定的文件。 (os error 2)` →
  `获取文件大小 C:\Users\…\replay-probe-…\nope.jsonl 失败：系统找不到指定的文件。 (os error 2)`；
  打开（路径是目录）`回放文件打开失败: 拒绝访问。 (os error 5)` →
  `打开回放文件 C:\Users\…\serial-tool-headless-replay-dir-38420 失败：拒绝访问。 (os error 5)`
  —— 后一条是从 `Workbench::dispatch(LoadReplay)` 的失败任务消息里抓的，不是推的。
  **少的那半句正是用户要改的东西**：改前看不出是【哪一步】对【哪个路径】失败。**（3）kind
  是承重的，且这次的修法与第 40 轮同一个理由**：`io::Error::other(文案)` 与
  `io::Error::new(kind, 文案)` 的 `Display` **逐字相同**，所以折 kind 不会被任何文案用例抓到，
  只会让将来按 kind 分派的分支（第 9/11 轮的重试判定、`tool_transport::translate_error` 的
  `Io` 臂）静默认不出「是权限、是不存在、还是盘没了」；理由写进 `prepare_load` 的文档注释。
  **同轮的例外，代码里就地写了为什么不动**：`prepare_load_text` 的
  `io::Error::other("回放文本解析未完成")` 底下没有 OS 错误、没有 kind 可保（`step(usize::MAX)`
  读尽文本必定返回 `Some`，那是"不可能分支"），`Other` 就是对的 —— 记在这里是为了让下一轮
  别把它当漏网的顺手改掉。**（4）动手前的三份普查**：(a) **谁读这三条** —— 生产侧只有一个
  调用者 `crates/application/src/workbench.rs::dispatch_load_replay`（`load_replay` 后台任务，
  `map_err(|error| error.to_string())`），出去的两条可见路径是任务消息与
  `AppEvent::TaskFailed` 那句 `后台任务失败: {error}` 系统日志；**没有** `AppError::Replay`
  那一层（第 24 轮的分类前缀在这条路上收不到，与第 40 轮 ① 的取证同一类纠偏），
  生产侧**唯一**的调用点就是它；web 走的是 `prepare_load_text`/`ReplayTextLoader`
  （`AppCommand::LoadReplay` 只出现在 native 的 `workbench.rs:336`，wasm 那份派发的是
  `LoadReplayText`），这三条出口在浏览器侧结构上跑不到；同步的 `ReplayManager::load`
  没有生产调用者，只有本 crate 的 `rejects_oversized_file` 用例走它（本轮按范围没动）。
  **纠一条本文件自己写错的前提**：`prepare_load` 的文档注释与那条用例的注释都写着
  「少一句框架就是英文 OS 原文**上状态栏**」—— 实测**状态栏不读它**：原生侧
  `Workbench::dispatch` 之后没有任何地方渲染 `load_replay` 的任务快照（`task_message`
  这个读法全仓只有测试在用），可见的就是上面那两处。两处注释已按实况改成点名
  「任务消息 + 那句系统日志」，免得下一轮又照"状态栏"去找不存在的接线。
  **顺带查出、本轮没动**：那句系统日志的框架 `format!("后台任务失败: {error}")`
  自己也是半角 `:`（在 `crates/application`，是所有后台任务失败进日志的公共出口，
  改它牵连导出/扫描插件一整批句串），与 `autosave.rs` 那格一起记进
  `docs/ARCHITECTURE.md` 当下一轮的候选。(b) **谁钉着** ——
  `grep -rn "回放文件打开失败\|获取文件大小失败\|回放文件读取失败" crates/ --include=*.rs`
  只命中**两个文件**：本文件（三条生产者 + 用例里的三条期望）、`crates/application/tests/headless.rs` 的
  `replay_file_that_cannot_be_opened_reaches_the_user_in_chinese`（一条注释 + 两条断言）。
  改了 5 处期望、**没删一条断言**：用例 ① 原 `starts_with("获取文件大小失败: ")`（只钉前缀，OS 细节钉不住）→ 逐字
  `== "获取文件大小 <路径> 失败：<裸操作>"`；② 原 `== "回放文件打开失败: {raw}"` →
  `== "打开回放文件 <目录> 失败：{raw}"`；③ 原 `== "回放文件读取失败: 网络磁盘已断开"` →
  `== "读取回放文件 <目录> 失败：网络磁盘已断开"`；headless 那条原
  `starts_with("回放文件打开失败: ")` + `== "回放文件打开失败: {raw}"` →
  `starts_with("打开回放文件 ")` + `== "打开回放文件 <目录> 失败：{raw}"`（框架断言与
  "整句正好等于框架+细节"这条双重框架守卫都留着，OS 细节仍由用例内跑的裸操作提供）。
  新立的期望之所以**正确**而不是"让它过"：三条都改成与裸操作逐项相等的形状，路径是
  本轮新增的可见信息，kind 另立一条用例。(c) **没人拿这段文字做判定** ——
  `contains("回放`/`获取文件`/`读取`、`starts_with`、`==` 三样扫过 `crates/`，命中的两处
  `starts_with` 全在上面的用例断言里；`crates/app/src/web/replay.rs:49` 的
  `读取回放文件失败：{error:?}` 是浏览器 `File.text()` 的失败、另一条链，本轮没牵连
  （也如实记下：它与新框架只差一个路径位，将来若要统一措辞得连着它一起看）。
  `crates/platform/tests/transport_error_wording.rs` 与 `storage_error_wording.rs` 不含回放
  句子，一条没动。**（5）三道变异都验过会红**：旧实现原封（`other` + 半角 + 不点名路径）→
  文案用例与 kind 用例各红（`left: "获取文件大小失败: …" / right: "获取文件大小 …\nope.jsonl
  失败：…"`、`① 的框架把 kind 折掉了 left: Other / right: NotFound`）；**只把 kind 折回
  `io::Error::other(..)`、框架一字不改** → 文案用例全绿、kind 用例单独红
  （`② 的框架把 kind 折掉了 left: Other / right: PermissionDenied`），这就是"只有 kind 看得见"
  的那格证据；只把三处生产侧框架退回半角（期望不动）→ ①② 两格的红点正是标点那一格
  （`left: …失败: …` / `right: …失败：…`）。**一度写好的两条 `contains` 冒号断言，量过之后
  删掉了**：逐字相等的期望里已经带着全角 `：`，再钉一条 `contains(" 失败：")` 是同一事实的
  弱重复，而 `!contains("失败: ")` 会连 OS 那半句一起读、随本机文案假红 —— 冒号由相等断言钉，
  这条取舍写进了用例的文档注释，免得下一轮又补回来。用例的可移植性与第 40 轮同一套：
  **同一个用例里先跑一次不加框架的裸操作**取本机 kind 与文案再比对，不依赖 chmod、不分平台
  `#[cfg]`、不硬编 OS 原话；② 的 kind 平台不同（Windows `PermissionDenied` / POSIX
  `IsADirectory`），所以只钉"等于裸操作"与"不是 `Other`"；③ 只能注入（真掉一次网络盘
  驱动不出来），注入两条不同 kind 钉"迭代器递来什么就还什么"。

- **worker 完成通道的载荷换成 `io::Error` 本体：走过这条通道的失败从此拿得到自己那句
  中文框架，不再一律 `IO 错误：`（第 41 轮如实记下的那处收窄，根因在此闭合）**（本轮只动
  `crates/transport/`：`lib.rs` 的 `SerialCommand`/`wait_for_completion`/`complete_command`/
  `finish_signal_command`/`execute_serial_command` + 它的 `mod tests`、`windows_native.rs` 的
  `report_completion`/`drain_commands`、`network.rs` 的两处 `send`；无新类型（不加结构体/枚举/
  trait/宏/模块）、无 `pub` API 变化（`SerialCommand` 仍是 `pub(crate)`，载荷类型不出 crate）、
  不加 `#[allow]`、豁免计数仍 5；四道门 fmt / native clippy `-D warnings` / wasm clippy /
  test 全 **EXIT=0**，**732 passed / 0 failed / 7 ignored**、32 条 `test result:` 行 ——
  基线 726 **净增 6 条**（4 条可移植 + 2 条 `#[cfg(windows)]`）。wasm 门与本改动无编译边：
  `cargo tree -p hardware-workbench-app --target wasm32-unknown-unknown -i tool-transport`
  实测回 `nothing to print`，这道门有效但空转）。**（1）根因**：`completion` 原语
  `Sender<Result<(), String>>` 加等待方那句"拿文案再造一条 `Error::other`"，
  等于把 worker 侧那条 `io::Error` 先渲染成文案、再重新造一条 kind 恒为 `Other` 的错 ——
  第 41 轮改读 `kind()` 之后，那几臂对**任何走过通道的失败**都不可达（不只是权限格）。
  现在载荷是 `Sender<io::Result<()>>`，`wait_for_completion` 只做 `.map_err(TransportError::Io)`，
  kind 与原始码一路到底。**（2）用户可见串的前后**（worker 侧失败，本机中文 Windows）：
  `IO 错误：拒绝访问。 (os error 5)` → `串口被占用或无权限访问：拒绝访问。 (os error 5)，
  请检查是否已被其他程序打开`；`IO 错误：serial write timed out` → `操作超时：serial write
  timed out`（后者的出处就是本 crate：`windows_native::wait_overlapped_timeout` 的写超时造的
  是 `io::Error::new(TimedOut, …)`）。本轮普查：这条通道今天的**外部接线**只有平台后端的
  `set_dtr_blocking`/`set_rts_blocking`（`crates/platform/src/native.rs`，5 秒预算），
  `send_to_blocking` 除本 crate 用例外没有 crate 外调用者 —— 即先修好的是信号线那一格的
  用户可见句；写臂与网络臂共用同一份载荷，四处生产者一次改到位（网络臂那条本来就没有 kind
  可丢，见下）。**逐字不变的**：kind 落兜底臂的那些（`IO 错误：<文案>`、
  Windows 上 `Uncategorized` 的「数据无效。 (os error 13)」「另一个程序正在使用此文件…
  (os error 32)」）、`open_serial` 那句 `Io(WouldBlock)` 的「… 正在关闭中，请稍后重试」、
  `close_port_blocking` 的 `操作超时：{端口} 正在关闭中`、网络串口两句（`IO 错误：网络串口
  不支持 DTR/RTS`、`IO 错误：{ws 发送失败文案}` —— tungstenite 的错误本来就不是 `io::Error`，
  这里的 `other` 是第一次造错，不是本轮删掉的那种重包）、以及 `WorkerClosed`/`QueueFull`/
  `InvalidHex`/`Serial` 四臂。既有措辞用例一条没改：`transport_error_wording.rs` 的
  `操作超时：read timed out`（直接造 `Io(TimedOut)`，不经通道）与 `headless.rs` 的
  `操作超时：{端口} 正在关闭中`、`reconnect_failure_words_are_translated_…` 全绿。**（3）
  一处按值搬不过去的例外（如实记）**：`drain_commands` 的写臂有**两个**消费者 —— 等待方与
  `NativeWorker::run`（它读 `raw_os_error()` 抑制"关闭中被取消"的噪声日志，1.92 实测该码 995
  的 kind 恰是 `TimedOut`，所以丢了原始码就会多打一条日志），而 `io::Error` 不是 `Clone`：
  原件留给 `result?`，通道拿 `completion_copy` 造的**同 kind 同 `Display`** 副本（另有用例
  逐字钉这份一致）。简报里"按值、别重包"在这臂只能满足一半，是简报没算到的一处双消费者。
  其余三处生产者（`complete_command`、`finish_signal_command`、`report_completion` 的信号线两臂）
  全是"先记日志、后交给通道"的 reorder —— 副作用是 worker 侧失败时系统日志那一行现在**早于**
  完成通知发布（原来晚于），文案与 topic 逐字未变。**（4）顺手删掉两处装饰性 `Result`**：
  `finish_signal_command` 永远返回 `Ok(())`、`execute_serial_command` 的 `Err` 载荷只被
  `is_err()` 用一次，两者现在分别返回 `()` 与"worker 是否继续运行"的 `bool`（错误本体已交给
  通道，留个假错误反而是噪声）；`wait_for_completion` 改 `pub(crate)` 只为让 Windows worker
  模块的用例驱动同一个消费端。**（5）`WouldBlock` 那条注释的前提核过了，简报的怀疑在这一格
  不成立**：`Io(WouldBlock)` 全仓唯一生产者 = `open_serial` 直接 `return Err`，**从不经过完成
  通道**，正文「… 正在关闭中，请稍后重试」确实已含中文 ⇒ 注释准确、本轮改动不牵连它（重连
  可重试性那两个第 9/11 轮的产品决定按要求未动）。同一趟普查发现的**另一格**才真的没人生产：
  `InvalidData` 臂的注释说"HEX 严格模式奇偶校验文案已含中文"，而 HEX 判定早已下沉到
  `tool_core::parse_hex`（失败走 `InvalidHex`），本 crate 无任何 `Io(InvalidData)` 出口，
  且在 1.92 实测遍历 Win32 码 0..4000：**没有任何一格映射成 `InvalidData` 或 `WouldBlock`** ——
  两臂按原样透出文案的取舍没动（删臂是产品决定），注释改成写实测结论并明说不编造用例。
  **（6）新增 6 条用例**：可移植 `worker_side_permission_failure_reaches_the_advice_through_the_channel`
  （写臂 + 中文 OS 原话 → 建议分支）、`worker_side_write_timeout_keeps_the_timeout_frame_through_the_channel`
  （写臂 + `TimedOut` → `操作超时：`，逐字钉整句）、`worker_side_signal_failure_keeps_its_kind_through_the_channel`
  （DTR 臂 = reorder 后的 `finish_signal_command`，kind 选真有出处的 `PermissionDenied`）、
  `a_dropped_completion_sender_still_reports_worker_closed`（钉"发送方先消失 ⇒ `WorkerClosed`"
  这条**改前就绿**的行为没被牵连）；`#[cfg(windows)]` `report_completion_keeps_the_error_kind_for_the_waiting_caller`
  （Windows 生产路径：`report_completion` → `wait_for_completion` → `translate_error`）、
  `completion_copy_keeps_kind_and_display_of_the_original_error`（副本与原件同 kind 同 `Display`，
  用真 `from_raw_os_error(5)`）。四条"应红"用例在旧载荷语义下（把两个 producer 改回
  `Error::new(Other, 文案)` 跑一遍）确实全红，红证与逐字输出记在报告里；两条钉行为不变的用例
  改前改后同绿。既有用例只改了一处的**形状**：`worker_loop_preserves_write_and_signal_fifo`
  的三句 `assert_eq!(rx.recv().unwrap(), Ok(()))` 在 `io::Error` 上没有 `PartialEq`（1.92 实测
  E0369），换成同断言义的 `expect_completed(…, "write:A")` 助手 —— 钉的仍是"三条命令按 FIFO
  各自回报成功"，措辞用例一字未动。

- **`tool_transport::translate_error` 的 `Io` 臂改读 `kind()`：中文 Windows 上的权限失败从裸的
  `IO 错误：` 回到建议分支**（本轮只动 `crates/transport/src/lib.rs` 的 `Io` 一臂 + 它的
  `#[cfg(test)] mod tests`；`Serial` 臂、重连可重试性与 `WouldBlock`/`TimedOut` 两格逐字未动 ——
  那两个产品决定仍挂在用户处（第 9/11 轮）；不新增枚举/trait/宏/`pub`、无签名变化；四道门
  fmt / native clippy `-D warnings` / wasm clippy / test 全 **EXIT=0**，
  **726 passed / 0 failed / 7 ignored**、32 条 `test result:` 行 —— 基线 721 **净增 5 条**
  （3 条可移植 + 2 条 `#[cfg(windows)]`；另 1 条 `#[cfg(unix)]` 只在 Unix 机器上编进来；
  wasm 门与本改动无编译边 —— `cargo tree -i tool-transport --target wasm32-unknown-unknown`
  实测为空，web 用的是 `tool_platform` 自己的错误枚举）。**（1）改前的判定是
  `is_permission_denied(&message.to_ascii_lowercase())` —— 拿五个英文短语的子串猜权限**，而 OS
  文案随系统语言本地化：本机（中文 Windows）权限失败的 `Display` 实测是「拒绝访问。 (os error 5)」，
  一个英文短语都不含，Windows 报的码也是 5 而不是 13 —— 于是它永远落进兜底框架。**（2）中文
  Windows 上用户可见串的前后**：`IO 错误：拒绝访问。 (os error 5)` → `串口被占用或无权限访问：
  拒绝访问。 (os error 5)，请检查是否已被其他程序打开`（OS 原话逐字留在冒号后，只换了判定）。
  英文 Windows 与 Linux 的**带 kind 路径逐字不变**（"Access is denied. (os error 5)" 此前靠文案
  命中、现在靠 kind 命中，同一句输出，有用例钉；既有 linux-only 用例一字未动仍绿）。
  **（3）`raw_os_error()` 不需要兜（核实而非假设）**：`from_raw_os_error` 造的错必然已过 std 码表
  —— Windows 码表只有 `ERROR_ACCESS_DENIED`(5) 与 `WSAEACCES` ⇒ `PermissionDenied`（std 1.92
  `pal/windows/mod.rs`，本机两条用例实测）；POSIX `EACCES`(13) | `EPERM`(1) ⇒ `PermissionDenied`
  （`pal/unix/mod.rs` 的 `decode_error_kind`，用例 `#[cfg(unix)]` 在 Unix 上跑）。而
  `Error::new`/`other` 造的错 `raw_os_error()` 恒 `None` —— kind 之外再抄一份码表是永远多不出
  命中的死分支。**顺带治掉一个反向误判**：Windows 的 `os error 13` 是 `ERROR_INVALID_DATA`
  （本机 Display「数据无效。 (os error 13)」），旧谓词 `"os error 13"` 那格照抄的是 POSIX 的
  EACCES，把它错包成串口权限建议；现在落回 `IO 错误：`。红证三条（对未改的旧实现跑新用例）：
  中文格 `IO 错误：拒绝访问。 (os error 5)`、真 `from_raw_os_error(5)` 同框、以及 13 那格的旧输出
  正是 `串口被占用或无权限访问：数据无效。 (os error 13)，请检查是否已被其他程序打开`。**（4）一处
  收窄如实记**：kind 被上游折成字符串的 `Io(Error::other(文本))`（worker 完成通道
  `report_completion`/`complete_command` 的 `.to_string()`，`wait_for_completion` 再包一层），
  若落在英文 locale 机器上恰好带英文权限文案，改前靠文案命中建议、改后走通用框架 —— 该形状没有
  任何既有用例钉过文案，真正的修法是上游停止把 kind 折成字符串，与 `Serial` 臂一起记为下轮目标。
  **（5）`Serial` 臂是剩下的、更难的一半，本轮按要求未动，只报可达性**：`sp::Error` 带类型化
  `kind`（`ErrorKind::Io(io::ErrorKind)`），POSIX 后端 EACCES ⇒ `Io(PermissionDenied)` 活着
  （`serialport-4.9/src/posix/error.rs`）；但 **Windows 后端把 `ERROR_ACCESS_DENIED` 折进
  `ErrorKind::NoDevice`**（与 FILE/PATH_NOT_FOUND 共用，`windows/error.rs::last_os_error`），
  权限信息在依赖内部就已销毁 —— 按 kind 在 Windows 半边分不出「占用」与「不存在」，这是它比
  `Io` 臂难的确切原因。**（6）新增 6 条用例**（transport `mod tests`）：可移植
  `translate_error_routes_a_localised_permission_failure_by_kind`（本机中文原话）、
  `translate_error_keeps_english_permission_wording_on_the_same_advice`（英文行为不变钉）、
  `translate_error_keeps_the_timeout_and_generic_frames_for_other_kinds`（对照）；
  `#[cfg(windows)]` `windows_raw_access_denied_reaches_the_permission_advice`（真
  `from_raw_os_error(5)` 端到端）、`windows_os_error_13_is_data_invalid_not_permission`；
  `#[cfg(unix)]` `posix_raw_eacces_and_eperm_reach_the_permission_advice`。可移植断言只钉三件
  （OS 细节在场、不落 `IO 错误：` 框架、本 cfg 的区分句在场），不依赖机器 UI 语言。
  动手前普查：`串口被占用…`/`当前用户没有串口访问权限` 两句在测试代码里除本文件 linux-only 用例外
  零钉点；外仓引用 `IO 错误` 的三处（`headless.rs` 措辞组、`transport_error_wording.rs` 的
  `Io(TimedOut)` 逐字钉）都不走权限格，一字未动、全绿。`is_permission_denied` 保留 —— `Serial`
  臂是唯一调用者，函数头已写明它为什么还留着与逐平台修法在哪。

- **录制文件的两道 io 出口各套一句中文框架并点名路径（`create_recording_file`）；「用户可见错误正文
  里的英文」这一类到此清完**（本轮只动 `crates/recorder/src/recorder.rs` 一个函数 + 它的
  `#[cfg(test)] mod tests`，签名与 `io::Result<File>` 不变、不新增错误枚举/trait/模块、不加 `pub`；
  四道门 fmt / native clippy `-D warnings` / wasm clippy / test 全 **EXIT=0**，
  **721 passed / 0 failed / 7 ignored**、32 条 `test result:` 行 —— 基线 720 **净增 1 条**，
  新增 `create_recording_file_frames_both_io_exits_in_chinese`，另把既有
  `start_fails_with_invalid_path` 就地加强（它原本只 `assert!(result.is_err())`，钉不住任何文案），
  两条都算同一族。**（1）改前的两句用户可见串**（native 状态栏，本机中文 Windows 实测）：
  建目录失败 = `录制错误：当文件已存在时，无法创建该文件。 (os error 183)`、建文件失败 =
  `录制错误：拒绝访问。 (os error 5)` —— 两句里**没有一处**说明是哪一步、对哪个路径失败，
  而这两步的修法完全不同（一个是父路径不想要个目录、一个是目标名撞上了目录）。
  **改后**（逐字取自一次真跑通的 `Workbench::dispatch(StartRecording)`，临时探针跑完即删）：
  `录制错误：创建录制目录 C:\Users\…\probe-dir-….jsonl\subdir 失败：当文件已存在时，无法创建该文件。 (os error 183)`
  / `录制错误：创建录制文件 C:\Users\…\probe-file-… 失败：拒绝访问。 (os error 5)`。
  框架形状不是新造的：照 `tool_updater` 里**同样两处操作**已有的
  `创建父目录 {} 失败：{e}` / `创建文件 {} 失败：{e}`（全角 `：`，路径放在框架里、细节留在冒号后），
  比"两段都用 `：` 串起来"少一处歧义 —— Windows 路径自带 `C:`。**（2）kind 是承重的**：包装一律
  `io::Error::new(err.kind(), ..)`，**不是** `io::Error::other(..)`（后者会让 Display 一模一样、
  kind 却折成 `Other`，把读 kind 的分支静默改瞎 —— 第 9/11 轮的可重试判定与
  `tool_transport::translate_error` 的 `Io` 臂读的都是它），理由写进函数文档注释，并且**有用例钉它**：
  把两处改成 `ErrorKind::Other` 的变异体在文案完全一致的情况下仍红
  （`left: Other / right: PermissionDenied`）。**（3）动手前的三份普查**（这一族被咬过两次）：
  (a) **没人拿这段文字做判定** —— 全仓 `contains(`/`starts_with(`/`== "` 扫过一遍，唯一的活体
  错误文案子串判定是 `crates/transport/src/lib.rs:1419` 的 `message.contains("os error 13")`，
  它在私有 `is_permission_denied` 里、只被 `translate_error(&TransportError)` 的
  `Serial`/`Io` 两臂调用，而 `TransportError::Io` 的四个生产者全在 transport 内部
  （channel/serialport），录制失败走的是 `AppError::Recording(String)`、**结构上进不去**；
  且框架把 OS 细节**原样**留在冒号后，即使将来进得去也照旧命中。**这条判定本身仍是第 9/11 轮
  那族的隐患（读英文子串猜 kind），本轮按要求只报不修**。(b) **它到用户眼前的路不是那两块面板**：
  `crates/panels/src/recording.rs:166` 那句 `录制错误：{error}` 读的是
  `RecorderStats::last_error`（只有 worker 的写/刷盘失败写它，起录失败早于 worker 存在），
  `crates/app/src/web/mod.rs:2641` 那个 `录制错误` 徽标读的是 web 自己的内存态
  （web 起录不落盘，`create_recording_file` 在 wasm 侧跑不到）—— 真正的出口是 native 状态栏
  `commands.rs::start_or_stop_recording` 的 `set_status_force(Error, error.to_string())`。
  **web 只把原因挂在 `on_hover_text` 上、标签本身不带正文**，要不要与 native 对齐是产品决定，
  本轮不动（与已挂账的两处 web/native 差异同族）。(c) **两条分支都真能撞上**：录制路径是录制面板里
  一个**可手输**的文本框（`device_panel.rs` 把 `&mut self.recorder_path` 直接交给面板），
  另有「浏览」按钮走 `pick_recording_path` 的 rfd 另存为；默认值是
  `default_recorder_path()` = 用户日志目录下 `session-<ms>.jsonl`，且相对路径由
  `resolve_recorder_path` 折到用户数据目录。⇒ ① 靠手输一个穿过普通文件的路径（或改到一个
  建不出来的位置）撞上，② 靠把路径填成一个已存在的目录撞上（手输那条口不加 `.jsonl` 后缀）。
  **（4）三条红证**（同一份用例分别对着三种旧代码跑）：旧实现原封 → 两条都红；只留 ① 的框架、
  ② 退回裸 `File::create` → ② 那一格红；两处文案都对、kind 折成 `Other` → kind 那一格红。
  用例不依赖权限（Windows 上 chmod 造不出来）、不分平台 `#[cfg]`、不硬编 OS 原话：
  **同一个用例里先跑一次裸操作拿到本机原话与 kind**，再要求框架化的结果逐项相等，
  手法与 `replay.rs` 的 `native_load_exits_all_mint_a_chinese_frame` 同一套。
  ① 的 kind 两边都推得出来（Windows `ERROR_ALREADY_EXISTS` / POSIX `EEXIST` ⇒ `AlreadyExists`，
  实测本机 183 就是它）；② 的 kind 平台不同（Windows `PermissionDenied` / Linux `IsADirectory`），
  所以只钉"等于裸操作的 kind"与"不是 `Other`"这两条。**（5）这一类闭合**：第 22–25 轮把到用户
  眼前的英文错误正文清完，`create_recording_file` 是当时**只记不修**的最后一条（`docs/ARCHITECTURE.md`
  第 25 轮那段末尾那句话已按实况改写）。这一族到这里没有已知的活站点剩着，**还开着的两件都不是
  文案问题、而是等用户拍板的产品决定**：web 状态栏那个只挂 hover 文本的 `录制错误` 徽标要不要把
  原因摊开，以及第 38 轮留下的 `ExpectNoTx` 语义（`docs/ARCHITECTURE.md:459`）。
  **顺带记两条查出来、本轮没动的**：`replay.rs:246/:254` 那两句中文框架是
  `io::Error::other(..)` 包的、kind 已折成 `Other`，且用的是半角 `: `；`autosave.rs:23` 的
  `录制失败: {e}` 也是半角 `:`（同文件另一处与 `recorder.rs` 全用 `：`）—— 都是文案/kind 一致性问题，
  各要动自己的用例，另起一轮。

- **extension 最后那处「轮数预算」与 recorder 的 `sleep(500)` 各换成等真条件的有界等待；recorder 的
  7 处收割等待一起从无界/5s 提到实测兜底 30s**（本轮**零用户可见变化**：只动 `crates/extension/src/manager.rs`
  与 `crates/recorder/src/recorder.rs` 两个 `#[cfg(test)] mod tests`，生产代码逐字节未动 —— 两文件
  `#[cfg(test)]` 以上的整段前缀（**34 324 B** / **27 658 B**）分别与 `HEAD`（`ab213a2`）逐字节相同，
  脚本比对而非人眼；`git status` 只有这两个 .rs + CHANGELOG.md + ARCHITECTURE.md 四个文件；四道门
  fmt / native clippy `-D warnings` / wasm clippy / test 全 **EXIT=0**，**720 passed / 0 failed / 7 ignored**、
  32 条 `test result:` 行，用例数 **0 增减**）。
  **（1）`manager::tests::natural_runtime_end_also_requests_cleanup`**（队列第 6 项）原来是
  `for _ in 0..50 { sleep(5ms); reap_finished(); if 非空 { break } }` —— **轮数**用完就红在下面那条
  `assert_eq!(take_cleanup_requests(), vec![…])`，机器满载时它只是"更早放弃"。改法不新增 helper：
  第 37 轮的 `settle_until` 加一个 **pump 闭包参数**（"推进这个条件的是哪个入口"由调用点说明），
  这里传 `reap_finished`、判据 `!cleanup_requests.is_empty()` 逐字取自随后那条断言；其余 7 处照常传
  `process_pending`。**为什么必须按参数给**：`process_pending` 不回收运行时（`update_runtime_states`
  是唯一的推进点），把它并进 helper 里会让 `cmd.notfound` / `cmd.late` 那两处"等插件处于 Running"
  的判据被提前踢出 Running —— 修一处 flake 造两处假红。**没降级成"至少一条"**：判据成立时运行时已被
  收割、该插件已从 `lua_runtimes` 移走，之后不会再追加条目，所以"恰好这一条"照旧。
  **（2）`recorder::tests::stats_updated_during_recording`**（队列第 3 项，教科书形状）：
  `sleep(500)` → `assert!(events_written > 0)` 换成轮询 `rec.stats().events_written` 真离开 0
  （worker 每写一条与字节数同批递增），原有的 `let s = rec.stats();` + 那条 `assert!` **逐字保留**，
  等不到带 budget / elapsed / `running` / `last_error` 大声红。
  **（3）recorder 的 7 处收割等待**：其中 4 处（原 `:826/:856/:936/:1042`）确实**无上界** ——
  `reap_stopping()` 返回 `None` 有两条结构不同的路（`stopping` 里没人 / 线程还没退），那个 `while`
  把两条都读成"再转一圈"，所以既可能永远自旋，也没有任何东西把它转成红。判据没有可依赖的契约：
  worker 走完 `stop` 要等一轮 `recv_timeout(100ms)`，之后还要 flush + 写 `.summary.json`，那是**磁盘
  与调度**的性质而不是不变量 —— 结论是"加 deadline"（第 38 轮留的话照办）。另外 3 处本来带 5s，
  实测同一条等待**通过**时最坏已花 **3.34s**（churn×6），5s 只剩 1.5 倍余量，是同一族的假红候选，
  一并提到 30s：**抬高上界不改变任何通过路径上的断言**，只把"挂死"换成"红"。
  **预算是量出来的，不是抄的**：临时把 8 处等待的上界抬到 60s 并打点，跑整包 libtest（16 线程并行）
  记「等待开始 → 条件成立」的墙钟 —— 收割等待 churn×4 最坏 **1.435s**、churn×6 最坏 **3.337s**
  （中位 100~150ms，就是 `recv_timeout(100ms)` 那道结构性下限）；`events_written` 首次离开 0
  churn×4 最坏 **1.236s**；`natural_runtime_end` 的清理请求 churn×4 最坏 **1.387s**（旧 250ms 预算
  连最坏的零头都不够，这就是它红的机制）。取 **30s** = 实测最坏的 9~21 倍，与 `manager.rs` 第 36/37 轮
  那 8 处同数；正向等待命中即返回，**不白等**（recorder 空载整包 **0.52s → 0.12s**，extension 不变
  0.28s → 0.28s，各 5 轮同场交替）。
  **同机背靠背、而且按轮交替**（`target/r39-ab.py`：同一批 hog 下 before / after 二进制一轮一轮换着跑，
  机器抖动两边同摊；`N/M` = M 轮里目标用例红 N 轮；单跑一条用例两侧都不红，**见红必须跑整包**）：
  `natural_runtime_end` 整包 cpu×2 **3/10 → 0/10**、churn×4 **3/10 → 0/10**、churn×6 **1/8 → 0/8**；
  `stats_updated_during_recording` 整包 cpu×2 **0/10 → 0/10**、churn×4 **2/10 → 0/10**、
  churn×6 **2/8 → 0/8**。**两侧都另测了单跑本用例**（`--filter` 精确一条，脚本会核对"恰好命中 1 条"）：
  `natural_runtime_end` 两侧 cpu×2 **0/20** · churn×4 **0/30**；`stats_updated_during_recording`
  两侧同样 **0/20** · **0/30**，而单跑均值 **1.00s → 0.53s** 就是那 500ms 白等的直接体现 ——
  两句合起来仍是第 36/38 轮的结论：**赌时长的失败面只在 16 线程并行的整包里暴露**。
  改前红点原文与编排器目击的一致：`manager.rs:1505 assertion left == right
  failed: left: [] right: ["panel.finished"]`、`recorder.rs:890 expected some events written, got 0`。
  **变异探针 10 个，全部还原并逐字节核过还原**（脚本会在还原后再建一次二进制并 `snapshot` 比对，
  不一致就点名）：extension 侧 ① 生产删掉 `update_runtime_states` 的 `request_cleanup` →
  **30.0s 到点**红在等待上（`运行时自然收尾应在 30s 内产生清理请求，实际等了 30.0020725s，最后观测 []`）；
  ② 清理请求记到别的插件名下 → 等待通过但 `assert_eq` 红 `left: ["other.plugin"] / right: ["panel.finished"]`
  （"恰好这一条"仍在）；
  ③ 多投一条 → 红 `left: ["panel.finished", "extra.plugin"]`（**没有**被"至少一条"悄悄放过）；
  ④ 夹具换成常驻 `ctx.bus.on`（运行时永不收尾）→ 30.00s 大声红而不是挂死；⑤ 新站点 pump 换回
  `process_pending` → 30.00s 红，证明 pump 这个参数是**受力构件**；⑥ 反过来把既有 `cmd.tracker` 那处的
  pump 换成 `reap_finished` → 该处 30.00s 红并打印 `最后观测 Some([])`（两侧都受力：pump 传错就是
  等不到，不存在"反正都能过"）。
  recorder 侧 ⑦ 录制判定恒假（worker 一条都不写）→ **30.0s** 红
  `录制 worker 应在 30s 内写出首条事件，实际等了 30.0032908s，最后观测 events_written=0 running=true
  last_error=None`；⑧⑨⑩ 把 `reap_stopping` 的判活改成恒假 → 原无界的 `pause_resume`、原无界的
  `stop_is_idempotent`、原带 5s 的 `start_stop_lifecycle` 各在 **30.0s** 红并带 `is_stopping=true`
  （同一条 mutation 打在三个点上，逐个证明"挂死"确实被换成了"红"，且三处的消息形状一致）。
  **本轮明确没做的两件事**：① `scenario.rs` 第 8 步 `ExpectNoTx` 的语义问题（队列第 12 项）按第 38 轮
  的记录继续等用户拍板 —— 同一批 A/B 里它改前 5/10、改后 4/10 红，量级没变，本轮没有为它写 workaround；
  ② 顺手实测到 `replay::tests::seek_performance_with_many_events`（`crates/recorder/src/replay.rs:1381`
  把"1 万条事件 seek < 1s"钉成定长墙钟断言）在同一片负载里改前 **6/10**、改后 **3/10** 红 —— 它**不是**
  `sleep(N)` 形状而是"拿墙钟当断言"，修法（等条件 + 只把耗时记成观测值，或按机器倍率放宽）与本轮
  三类都不同，如实登记进 `docs/ARCHITECTURE.md` 的 (b) 桶交给下一轮，不顺手改。
  **主代理独立复测**：两文件 `#[cfg(test)]` 以上的生产前缀另以脚本逐字节比对 `HEAD` 相同；自带 64 个
  搅缓存 hog、整包 libtest 各 8 轮的样本里**本轮改过的用例一条都没红** —— extension 3 轮红全在
  `scenario` 那条第 8 步（上面①），recorder 2 轮红全在 `replay.rs:1381` 的墙钟性能断言（上面②）；
  四道门重跑仍 0 / 0 / 0 / 0，**720 passed / 0 failed / 7 ignored**。
- **`scenario_covers_retry_cancel_and_timeout` 的 500ms 固定窗口换成实测兜底 5000ms（顺带量出这条用例
  真正的头号红因在反向步骤，本轮如实记录、不粉饰）**（本轮**零用户可见变化**：只改
  `crates/extension/src/scenario.rs` 的 `#[cfg(test)] mod tests`，生产代码逐字节未动 —— `cmp` 过
  `#[cfg(test)]` 以上的整段前缀（274 行 / 9569 字节）与 `083b18c` 相同，`git diff` 唯一的 hunk 在第 287 行之后，
  `git status` 只有 scenario.rs / CHANGELOG.md / ARCHITECTURE.md 三个文件；四道门 fmt / native clippy
  -D warnings / wasm clippy / test 全 **EXIT=0**，**720 passed / 0 failed / 7 ignored**、32 条
  `test result:` 行，用例数 **0 增减**；空闲整包 `cargo test -p tool-extension --lib`
  改前 0.26~0.28s、改后 0.26~0.27s（各测 3 次，抬窗口不花时间：`expect_event` 每 1ms 轮询、命中即返回））。
  **先量再改**：把窗口临时抬到 8000ms 让每一步都能命中，用一次性探针记「步骤开始 → 命中」的墙钟（19 轮）。
  原值 500ms 红在第 2 步（`Some("step 2: 未收到预期 TX: PING\n")`，与编排器目击的一致），但根因不是
  「插件定时器要 500ms」：**派发命令 → 看到该任务第一条 TX** 实测最坏 **3270ms**（churn×4 跑整包时；
  cpu×2 下 606ms；空载 1ms），**同一个任务已跑起来的下一拍**（fixture 的 `sleep_ms(40)` 周期）实测最坏
  **630ms**（空载 40ms）—— 两档都远超 500ms，所以红的一直不止第 2 步。链路是
  命令 → `PluginManager::process_pending` 转发 → 插件线程被调度 → 写 → 端口 worker 发布 → runner 排空订阅，
  **每一环都要抢 CPU**，正向步骤之间没有本质区别，所以**不给两档数**：第一版按 1.5 倍把「下一拍」单独压到
  1000ms，**它自己立刻成了新红点**（churn×4 复红 1/8，红在第 3 步），遂并成单一
  `TX_BACKSTOP_MS = 5_000`（= 实测最坏 3270ms 的 ≈1.5 倍）。反向的 `ExpectNoTx { Some(80) }` 不继承它
  —— 缺席检查会整段白等，那等于把 80ms 的有界检查变成 5s 白等。
  **前后同机背靠背复测**（先测改后，立刻换回 HEAD 二进制测改前，消机器抖动；`N/M` = M 轮里红 N 轮）：
  单跑本用例（`target/r36-load.py --targets scenario`）改前 cpu×2 **0/20** · churn×4 **0/30**，改后
  **0/20** · **0/30**（更早一次独立采样的改前 churn×4 是 1/30）；**跑整包 libtest 才是见红的环境**
  （16 线程并行，`target/r38-steps.py`）：改前 cpu×2 **10/16** · churn×4 **5/16**，改后 cpu×2 **3/8** ·
  churn×4 **6/8**。**按红因拆开才看得清**：改前 32 轮里「正向窗口到点」**5 次**，改后 16 轮里 **0 次**
  （第 36 轮那次 500ms + churn×4 诊断跑是 4/5 全红在第 2 步）；剩下的红**全部**在第 8 步
  （`等待窗口内出现了意外 TX`），**改前就占 10/32**，且各批之间从 3/8 抖到 6/8 —— 这两列**不能直接相减**：
  改前那些跑在第 2 步就早退，根本没走到第 8 步，掩盖了它。
- **（B）结论：第 8 步 `ExpectNoTx` 既证明不了「取消让循环停下来」，自身也是负载红点 —— 记档，本轮不改**
  （要改得动生产语义，那是用户可见 API 问题，不在本轮裁量里）。观测日志**确实拿得到**
  （`report.cases[0].raw_packets` 由 `SerialPluginScenarioRunner::observed` 过滤 RX/TX 生成，
  `scenario.rs:144` 与 `packet_log`），所以「整场恰好 N 条 PING」是**写得出**的断言 —— 但它**不成立**：
  空载恰好 **4** 条（第一次 3 条 + 第二次被取消前的 1 条），而 19 轮负载探测里 **4 / 5 / 6 条都出现过**
  （churn×4 8 轮里 2 轮 6 条 = 两次 retry 各跑满 3 条；cpu×2 6 轮里只有 1 轮是 4 条），因为
  **取消命令的派发最坏要 3.3s，而这循环总共只跑 120ms**，它常常在取消送达前就自己跑完了。
  加 `assert_eq!(ping_count, 4)` 就是第 36/37 轮正在消灭的那种新假红；退一步写成 `<= 6` 又没有区分力
  （取消坏掉的变异恰好也是 6 条）。**为什么加大窗口也救不了**（三角不可能）：可靠需要
  「写 → 被观测」的往返 < 循环周期，有牙需要缺席窗口 > 循环周期，合起来就是窗口 > 派发延迟
  （实测 ≈3.3s），而缺席检查每次**通过**都要白等那么久；何况窗口长度救不了可靠 ——
  `expect_event` 进门先扫 backlog，取消前那几条 PING 是 **0ms 命中**（多条红的第 8 步实测耗时 0ms）。
  **已核实两件事**：① 这一步仍然有牙 —— 把 fixture 的 `scenario.cancel` 改成空操作，**空载单跑立刻红**
  在第 8 步；② 负载下它红的原因与假红**同源**，所以这句话在负载下不带信息 —— 同一空操作变异在 churn×4
  整包 **8/8 全红**、cpu×2 单跑 **13/13 全红**，消息与诚实路径的假红逐字相同。而假绿（跑完还在写却通过）
  在本次能看到 PING 条数的 19 轮负载探测里**一个实例都没抓到**（多出的那几条总是同批涌进窗口、先把假红触发了）：
  它是**结构性**的（没有任何机制保证那批事件何时浮现 —— 端口 worker 实测会把相隔 40ms 的三次写攒进
  同一毫秒发布），不是已观测到的通过。**关闭它的代价**（下一轮的活，两条路都要动生产/DSL）：
  让 `ExpectNoTx` 只统计**本步开始之后**发布的 TX（现在它把步前就躺在 backlog 里的旧事件也算违规），
  80ms 窗口即可既可靠又有牙；或给 DSL 一个「排空」步、让 `Cancel` 等到插件真处理完再返回。
  测试侧唯一自洽的形状是把 fixture 改成**永不自行结束**的循环（`while true` + 取消后写一条终止标记），
  把「停了」变成正向期望 —— 那是重写这条用例，不在本轮范围内。
- **变异探针 2 个，全部还原并 `cmp` 逐字节核过**（工作树与留档副本 `cmp` 相同，`#[cfg(test)]` 以上与
  `083b18c` 相同）：① fixture 根本不写 PING（等的那件事永远不发生）→ **5.07s 到点**红在
  `Some("step 2: 未收到预期 TX: PING\n")`，不挂死也不静默过 —— 证明 5000ms 是真上界而不是「大到永远通过」；
  ② `scenario.cancel` 变空操作 → 见上条（空载即红、负载 8/8 与 13/13 全红，且与假红同源）。
- **把 `settle()` 的固定 500ms 换成按条件有界的 `settle_until()`（7 处调用逐条等真条件）**（本轮
  **零用户可见变化**：只改 `crates/extension/src/manager.rs` 的 `#[cfg(test)] mod tests`，生产代码逐字节
  未动 —— `git diff` 的每个 hunk 都在第 920 行 `#[cfg(test)]` 之后，`cmp` 过 `#[cfg(test)]` 以上的整段
  前缀与 `5f4a78f` 逐字节相同；四道门 fmt / native clippy -D warnings / wasm clippy / test 全 **EXIT=0**，
  **720 passed / 0 failed / 7 ignored**、32 条 `test result:` 行，用例数 **0 增减**）。
  `settle()` 原先对 7 条用例**无条件**睡满 20×25ms 再 `process_pending()`，既不看过随后要断言的那件事
  有没有发生，每条又白付 500ms。改法是第 36 轮立的形状：一个就地 helper `settle_until(manager, budget, |m| …)`
  （墙钟 deadline + `FnMut` 判据，照搬本仓 `crates/application/tests/headless.rs:151` 的 `tick_until`），
  每处判据**逐字取自紧随其后的断言**，条件一成立立即返回、等不到就带 `elapsed()` 和最后观测红掉。
  **先量再改**（配方同 `docs/ARCHITECTURE.md`，本机 16 核，`--full` 跑整包列红名，同机背靠背复测消机器抖动）：
  churn×6 30 轮，改前 4 条依赖 register 事件流的用例见红 —— `tracks_registered_commands_after_enable` **8/30**、
  `clears_registered_commands_on_disable` **10/30**、`detects_undeclared_dynamic_commands` **5/30**、
  `command_dedup_on_repeated_register` **4/30**；另 3 条（`detects_missing_declared_commands`、
  `diagnoses_command_not_found_on_execute`、`command_not_found_cleared_on_register`）本就 0/30（它们断的是
  同步 `process_event` 的反应，旧 `settle` 对它们纯属白等）。churn×4 30 轮见红 7 / 8 / 1 / 4。
  **改后同一配方两条 hog 全部 0/30**，空闲整包 wall time **1.63s → 0.26s**（7×500ms 的固定等待没了）。
  同配方里不碰 `settle` 的两条（`natural_runtime_end_also_requests_cleanup` 桶 (b)#6、
  `scenario_covers_retry_cancel_and_timeout` 桶 (b)#1）前后都随机器负载在 5~23/30 抖动 —— 与本轮无关，
  仍是后续轮次的活。**「恰好 1 条」没被悄悄降级成「至少 1 条」**：判据只能停在「已注册 cmd.dedup.run」
  （去重后恒 1），所以判据成立后再 `process_pending()` 干排几轮把第二条 register 事件也排干才计数 ——
  两条 publish 是脚本相邻两行、中间无 timer/wait，第一条可观测时第二条已在队列里，去重坏掉会稳定读到 2。
  **4 个生产变异、覆盖全部 7 条 + 3 个数量/形状敏感位，全部还原并 `cmp` 核过**：① `handle_command_registered`
  直接 return（不记录）→ 4 条 register 类用例 30.00s 到点红，消息带 `实际等了 30.0001695s，最后观测 Some([])`；
  ② 把命令记到错的 plugin key 下（`{plugin_id}!wrong`）→ `tracks` / `detects_undeclared` 仍红（有数据到达
  但判据绑死 plugin_id+命令名，错数据不认，超时 `Some([])`）；③ 去掉去重守卫（`if true`）→ `command_dedup`
  红在 `manager.rs:1650 left: 2 right: 1`（收尾排干后精确计数抓到重复），`tracks` 在此变异下仍绿（注册的是不同命令，
  去重无关，证明变异是定点的）；④ `enable_lua` 不把状态置 `Running` → `detects_missing_declared_commands`
  红（`missing_commands 应在 30s 内含 …，最后观测 Some([])`）、`diagnoses_command_not_found_on_execute` /
  `command_not_found_cleared_on_register` 红（`应在 30s 内处于 Running，最后状态 Some(Disabled)`）——
  后两条断的是同步反应，前置只是 Running，等不到只会是 Running 之外的真坏状态。
- **修掉「sleep 完就断言异步结果」这一类测试随机红里的两条**（本轮
  **零用户可见变化**：只换两个用例
  的等待方式，生产代码一个字节都没动 —— `git status` 只有 `crates/extension/src/manager.rs` 与
  `crates/application/tests/headless.rs` 两个文件；四道门 fmt / clippy / wasm clippy / test 全
  **EXIT=0**，**720 passed / 0 failed / 7 ignored**、32 条 `test result:` 行，与 `a5e0eae` 基线逐位相同，
  用例数 **0 增减**）。**失效模式先量出来再改**（round 16 立的规矩）：负载配方 = 起 `cores×N` 个 python
  忙等进程（本机 16 核），再把两个测试二进制各跑 N 次记录红绿。改前：
  `headless_workbench_can_dispatch_and_query` 红在 `headless.rs:58`（`assert!(… connect_serial … Failed)`）
  **12/20**（32 个纯 CPU hog）· **27/30**（64 个 CPU+缓存 hog）；
  `manager::tests::discovers_manifest_and_enables_lua_plugin` 红在 `manager.rs:947`
  （`assert_eq!(rx.drain().len(), 1)` → `left: 0, right: 1`）**7/30**（64 个 CPU+缓存 hog）——
  **纯 CPU hog 下它 0/90 不红**，只有把缓存一起搅浑才复现，两处 panic 原文与编排器目击的一致。
  改法不引新抽象：headless 复用本文件已有的 `tick_until(…, timeout: Duration, |wb| …)`（`:151`，另有
  30 处调用），把「tick 满 100 轮」换成「等到 `connect_serial` 真的落 `Failed`」；extension 等到
  `reap_finished()` 之后 `plugin_state` 离开 `Running` —— Lua 线程跑完脚本才置 `alive=false`，收割时
  顺带 `join` 那条线程，`join` 就是「脚本里的 publish 已全部可见」的边界。**两处都保留真上界 30s、
  条件一成立立即返回、等不到就带着 `elapsed()` 和最后一轮观测红掉**，不挂死也不静默过。
  **「恰好 1 条」没有被悄悄放宽成「至少 1 条」**：等的是运行时收尾，收尾之后一次性 `rx.drain().len()`
  拿到的就是脚本发布过的全部。**变异探针五条，全部还原并 `cmp` 逐字节核过**：① `ctx.bus.publish`
  发两遍 → `manager.rs:968 left: 2 right: 1` 红；② 脚本根本不执行（`lua.load("")`）→ `left: 0 right: 1` 红；
  ③ 运行时永远不收尾（脚本挂一个 `ctx.bus.on`）→ **30.00s 到点**红在等待断言上（消息含耗时与
  `Some(Running)`），证明的是「响不出来就大声失败」而不是赌时长；④ 连接错误被吞成成功 → headless 红并打印
  `[("refresh_ports", Completed), ("connect_serial", Completed)]`；⑤ 任务 kind 改成 `open_serial` → 红并打印
  `[("refresh_ports", Completed), ("open_serial", Failed)]`。**同配方（同样的 hog、同样的 N、二进制重建）复测**：
  churn×4 30 轮 **0/30 + 0/30**，cpu×2 20 轮 **0/20 + 0/20**，再加一档 churn×6（96 个 hog）20 轮
  **0/20 + 0/20**。其余 `thread::sleep` 站点（全仓 **54 处**，其中 1 处是 `timing.rs:30` 的注释提及）
  按「生产码 / 同类队列 / 有意节奏」三桶记进 `docs/ARCHITECTURE.md`「验证」一节，配方也写在那里；
  下一轮的排队者已用同一配方实测见红：`scenario::tests::scenario_covers_retry_cancel_and_timeout`
  （整包 2/2 红）、三条依赖 `settle()` 的 manager 用例、`recorder::tests::stats_updated_during_recording`
  （1/2 红）、`plugin_enable_disable_transitions_state_and_rejects_unknown_ids`（1/2 红）。
- **删掉已被游标路径取代、却仍各存一份的整座快照导出域**（`tool-panels` 净 **−306 行**：
  `terminal.rs` 3593 → 3441、`log.rs` 2400 → 2281、`manager.rs` 1683 → 1663（步骤 B）、
  `table.rs` 1340 → 1325（级联）、`lib.rs` 只改两条 `pub use`；四道门
  **720 passed / 0 failed / 7 ignored**，725 → 720 逐条见下）。删的是"点导出先物化全量快照"
  那一族：`TerminalExportJob` + `impl`（`len`/`is_empty`/`render`/`render_chunk`）、
  `LogExportJob` + 同形 `impl`、两份 `export_job()`、两份 `export_visible_text/csv/json` ——
  **同一个域以同一形状存在两份（terminal/log），这次两份一起走**。活路径原样保留：两端都走
  `begin_export_cursor()` → `export_cursor_chunk(cursor, format, 512)`
  （`app/src/commands.rs:136/140`、`app/src/web/mod.rs:2576/2580`）。
  **这正是符号普查的已知盲区**：岛内的边（`export_job → render → render_chunk`）按引用计数
  看着是"生产"——调用者确实不在 `#[cfg(test)]` 里——但进岛的唯一门是测试，判据取可达性。
  **用户下载的字节删除前后完全相同，证据是字面量钉的两次红，不是"测试仍绿"**：旧的差分用例拿
  `export_job().render(format)` 当期望值，先把它换成从活路径捕获的字面量表（时间列经 `fmt_ts`
  按本地时间渲染，只有时间标签插值，表头/引号/换行/JSON 框架全是字面量），绿了之后做两次变异
  探针 —— ①改活路径 CSV 表头顺序：
  `assertion left == right failed: Csv 分块导出字节与字面量不一致，
  left: "time,direction,port,text\n…" right: "time,port,direction,text\n…"`（log 同形一条）；
  ②剥掉 `csv_cell` 的双引号：三条字面量用例（terminal/log/unicode）全部变红。还原后复绿。
  删除的用例里有三项**唯一**的钉（CSV 引号双写、HEX 模式内容列、TX 方向过滤）已先折进字面量表，
  不随删除丢钉。
  **用例 725 → 720，−5 全部点名**：① `terminal::export_uses_standard_json_and_the_current_visible_content_mode`
  （主体是 `export_visible_*`）、② `terminal::chunked_export_preserves_format_boundaries`、
  ③ `log::chunked_export_preserves_csv_and_json_structure`（②③主体是 `job.render_chunk`，
  批大小=1 的游标用例钉得更密的边界）、④ `table::message_search_is_unicode_case_insensitive`
  （主体是级联删除的 `MessageSearch::matches`，Unicode 大小写折叠改由重写后的
  `log::unicode_case_insensitive_search_filters_exported_rows` 在活路径上逐字节钉）、
  ⑤ `manager::select_center_panel_switches_active`（主体是步骤 B 删除的方法）。
  `log::cursor_export_matches_snapshot_in_small_batches` 与 terminal 同名差分用例**改名续用**，
  计数不变。**编译器找到的级联恰好一条**（`clippy -p tool-panels --all-targets` 不挂 `-D` 列出）：
  `MessageSearch::matches`——最后一个非测试调用者就是被删的 `LogPanel::collect_visible_entries`；
  它连同源测试 ④ 一起删。其余每个私有助手逐个核过仍被活路径（分块渲染或右键"复制全部可见"）
  到达、原样保留：`render_terminal_export_chunk`/`render_log_export_chunk`、`visible_row_*`、
  `log_export_line`/`log_csv_record`/`log_json_value`、`csv_cell`、`json_str`、
  `content_column_name`、`direction_export_label`；`collect_visible_entries` 岛亡后活引用为零，
  连岛带走；两份活渲染器的 doc 里"与快照 job 共用实现"那两句已成假话，就地改写而非删除。
  **步骤 B（同 crate，单独记）**：`PanelManager::select_center_panel` 生产零调用、唯一调用者是
  自己那条单测，按"只测自己"删除。**此前 docs 记的理由"组合根直接赋 `active_tab` 字段绕过了它"
  当场证伪**：那两处直写（`app/src/ui/tiles.rs` 的 `tile_panel_body`、`app/src/web/mod.rs` 的
  `render_panel`）发生在**绘制一块 tile 的过程中**，改走 `select_center_panel` 等于在布局途中
  执行 `ensure_tiles_layout().select_pane()`，是另一种操作而不是绕过；活的面板选择走
  `TilesLayout::select_pane` 一族 handler（`manager.rs:621/648/678/690/714`，经
  `add_to_main_tabs`/`add_to_plugin_tabs`），兄弟方法 `open_tab`/`open_plugin_tab`/`close_tab`
  都是活的（`dynamic/ingest.rs`、`runtime/plugin.rs` 在调），一字未动。
  **刻意没做的**：`NativeExportJob`/`WebExportJob`（app 层与游标同名的活类型）不在射程内；
  第 20 轮 CHANGELOG/ARCHITECTURE 里那段"曾删掉 `ServiceTerminal::export_job` 链"的历史叙述是
  记录不是承诺，不改写；豁免棘轮仍 5 条（本轮没碰任何 `#[allow]`）。

- **删掉 `LuaHost` 整座零使用者的死岛**（`crates/lua_host/src/lib.rs` 净 **−143 行**，2792 → 2649；
  四道门 **725 passed / 0 failed / 7 ignored —— 与基线逐字相同，一条用例都没丢**）。删的四组符号：
  `pub struct LuaHost` + 它的 `impl`（`new`/`run_script`/`stop`/`state`/`reap_finished` 五个方法）+
  `impl Drop for LuaHost` + 只有它一个用户的 `struct LuaWorker` —— 第 23 轮记下、第 26 轮没还的那颗，
  本轮按"一轮一个域"还掉。**零使用者是按调用点形式证的，不是"grep 没搜到"**：
  ```bash
  grep -rnw 'LuaHost'   crates/   # 删前恰好 3 行，全在 lib.rs：结构体 / impl / impl Drop
  grep -rnw 'LuaWorker' crates/   # 恰好 3 行：定义 + LuaHost 的字段与构造点
  grep -rn  'LuaHost::' crates/   # 删前就 0 命中：一个调用点都没有（含它自己 crate 的单测）
  ls crates/lua_host/             # 无 tests/ 目录；本 crate 的测试只有 lib.rs 里那个 #[cfg(test)] mod
  grep -rn 'LuaHost' plugins/ --include=*.lua   # 0 命中：插件侧不知道这个名字
  ```
  岛外只剩三个**同前缀但活着**的名字（`LuaHostError` / `LuaHostResult` / `LuaHostServices`）和另一个
  类型 `LuaPluginRuntime`，它们都不是本轮对象。插件事件循环走 `run_plugin` / `LuaPluginRuntime`
  （`crates/extension/src/manager.rs:11`），测试走自由函数 `run_script_blocking` / `run_script_for_test`
  （两个 crate 的测试在用），**两条活路径原样保留**。
  **用户可见行为一处都没变，理由是这条路径从来没人走过**：全仓没有任何地方构造过 `LuaHost`，那 5 个
  方法一次也没被调用，它们写的日志（`正在运行 {}` / `{} 已完成` / `{} 已停止：{error}` /
  `{} 失败：{error}` / `请求停止`）在这台机器上从没可能到达用户。字面量层面也审计了，不是推的：
  改动前后字符串字面量 **509 → 503**，消失的 6 条（上面 5 条中文日志 + 那条 `"lua"` source 标签）
  **全部位于被删的函数体内**，新增 **0** 条。
  **留下的尾巴逐个查过而不是假设**：① 两处删除边界（`LuaRunState` → `LuaPluginRuntime`、
  `impl Drop for LuaPluginRuntime` → `pub fn run_plugin`）都重读过 —— `LuaWorker` 与 `LuaHost` 本来
  就没有 doc，所以没复发第 23 轮那种"doc 落到下一个条目头上"的事故；`lib.rs` 没有 `//!` 模块 doc、
  `pub use` 清单里也没有被删类型，两者都不需改。② imports 逐个核：`ParkingMutex`/`AtomicBool`/
  `thread`/`JoinHandle`/`Receiver`/`DataBus`/`TransportManager`/`LogLevel` 全都另有使用者
  （`run_plugin`、`plugin_event_loop`、`fail_startup`、`join_with_timeout`、测试），
  **所以一条 `use` 都没删** —— `clippy --workspace --all-targets -- -D warnings` exit 0 是这件事的机器验证。
  **本轮刻意没做的两件事**：① `LuaHostError::AlreadyRunning` 随删除**跌到全仓零生产者**
  （`grep -rn 'Err(LuaHostError' crates/` 现在 0 命中；只剩 `crates/extension/src/lib.rs:103` 的映射臂
  与 `crates/extension/tests/error_wording.rs:99` / `:190` 两条手工构造），而 `Lua` 臂经
  `#[from] mlua::Error` 仍有 8 处 `?` 生产者（`run_script_blocking` 4 + `replay` 分析器入口 4）——
  两个变体的形状从此不同。**收不收它们是同一个未做的决定**（"是否让 `run_plugin` 继续返回 `Result`"），
  故两条臂、`extension/src/lib.rs:103` 的中文措辞与 `tests/error_wording.rs` 全部一字未动，
  只把这条事实变化记进 `docs/ARCHITECTURE.md`（改了 4 处：第 23 轮盲区那条的触发点、它的延烧清单、
  "剩下的 DEAD"状态行，以及第 22 轮那条待决判据）；② `plugin_event_loop` 上那条
  `#[allow(clippy::too_many_arguments)]` 连同它写明理由的注释原样留着 —— 那是下一轮的靶子，
  **豁免棘轮因此仍是 5 条**。另：`docs/architecture-refactor-baseline.md` 里那个 `LuaHost{…}` 形状描述
  没改，它是钉在 `0fc66b3` 的 Phase 0 只读快照（同文件至今还写着第 27 轮删掉的 `Clock`），改它等于伪造取证。
- 串口分组编辑器里那段**只为"未来调用者"兜底的同步块删掉**，参数墙收掉、**棘轮再降一条（6 → 5）**；
  四道门 **725 passed / 0 failed / 7 ignored**（723 → 725；`tool-panels` lib 的用例数 215 → 217，
  新增的 2 条全是那条形状守卫拆出来的两个用例）。**用户侧一处都没变**：删的是恒等价的重复写入，别名输入与分组
  下拉的行为、外观与持久化全部照旧。
  `crates/panels/src/serial.rs::render_port_editor` 末尾原先有 11 行
  `if selected_group != current_group { … }`，注释说它是为了"将来有人不走 ComboBox 回调就改掉初值"。
  普查下来这个调用者**不存在**，于是它不是兜底而是逐字重复：① `current_group` 全文件只被这一处读；
  ② 两个调用点（`render_grouped_port` 的单行内联分支与窄屏显式两行分支）都是
  `let mut selected_group = group_name.to_owned();` 并把同一个 `group_name` 当 `current_group`
  传进去 —— 进函数时必然相等；
  ③ 能把它改不等的只有 `ComboBox::show_ui` 里那两个 `ui.selectable_value(selected_group, …)`，
  而 **egui 0.35 写值与标脏在同一支**（`egui-0.35.0/src/ui.rs:1944`
  `if response.clicked() && *current_value != selected_value { *current_value = selected_value;
  response.mark_changed(); }`，`changed()` 读的就是这个 flag，`response.rs:593/605`），
  所以"选中值 ≠ 初值"⟺那两个 `if … .changed()` 本帧已经跑过，而它们做的 remove/insert 与那段
  逐字相同（`UNGROUPED` → `groups.remove`，组名 → `groups.insert`）。按本仓"不留投机兜底"的规矩，
  **块连同那个参数一起删，注释里也没留那句"未来调用者"**。
  剩下的是第 29/30/32 轮同一式：7 个位置参数（`ui` 除外）收进 `PortEditorInputs`（字段名与原参数
  逐字相同，私有、无 trait / 无泛型 / 无 builder，`SerialView`/`SerialPortMetadata`/`SerialPanel::*`
  签名与 serde 字段全不动），函数体只在开头多 6 行解构（`ComboBox` 的闭包要同时握
  `selected_group` 与 `metadata`，走 `&mut` 结构体的字段借用进不了闭包），其余**逐字节不变**；
  参数 **8 → 2**，函数 **68 → 53 行**，`#[allow(clippy::too_many_arguments)]` 删除。
  **一处与简报不同的地方**：`metadata` 字段没法写成 `&'a mut SerialPortMetadata<'a>` ——
  `&mut` 对内部生命周期是**不变的**，那样写等于要求元数据引用的数据活得和调用点的局部
  `alias`/`selected_group` 一样久，实测 8 条错误（E0597×4 / E0507 / 3 条 lifetime）。rustc 的
  "建议"是把 `render_grouped_port` 所有参数并成一个生命周期 —— 那是把墙摊开而不是收掉。
  正解是给 `metadata` 单独的 `'b`（`PortEditorInputs<'a, 'b>`），那正是原参数
  `&mut SerialPortMetadata<'_>` 省略掉的两个生命周期；限度已写进结构体 doc，免得后来人"简化"回去。
  **覆盖要说准**：这条路径改动前后**都没有任何行为用例**驱动 —— 全仓 grep `grouped_port_list_ui`
  / `SerialPortMetadata` 在 `crates/` 下零测试命中，生产消费者只有
  `app/src/ui/device_panel.rs:347` 与 `app/src/web/serial.rs:265`。所以本轮的答复不是补一个
  行为测试，而是一条**源码形状守卫**（`serial.rs` 首个 `#[cfg(test)] mod tests`，2 条用例）：
  读文件、归一行尾、把注释与字符串的内容抹成空格再扫，断言 `current_group` **全文件 0 次**
  （连注释里都不许留名，这条先红过一次：被抓住的正是守卫自己的散文）、两处调用点各有一条
  `alias: &mut alias,` 与 `selected_group: &mut selected_group,`、任何 `PortEditorInputs {` 字面量里
  不得出现 `X: &mut Y` 且 `X != Y`，另加两条防空转下界（字面量恰好 2 个、函数体顶层语句 ≥11 条，
  实测 11）；被搜索的字面量一律 `concat!` 拼，否则本模块自己的文本会被当成命中。
  **红证明**：把一处调用点的两个字段值对调 —— 先确认这个版本**编译零告警通过**（危险是真的，
  类型系统看不见），守卫随即红并点名两侧，原文是
  `PortEditorInputs 字面量里出现交叉绑定：alias: &mut selected_group、selected_group: &mut alias`。
  恢复后回绿。**顺带纠一条事实（简报担心的方向是对的，对象错了）**：`.gitattributes` 钉着
  `*.rs text eol=lf`，`serial.rs` 实测也确实是纯 LF（0 处 CRLF），所以本文件切片不会因为行尾错位；
  但"整棵树是 CRLF"和"整棵树是 LF"两个说法都不成立 —— `crates/app/src/config.rs`（722 行全 CRLF）与
  `crates/core/src/lib.rs`（993 行全 CRLF）在盘上就是 CRLF，本轮改的 `docs/ARCHITECTURE.md` 也是。
  守卫里那步归一化因此不是装饰，是必须的。这条限度照第 32 轮的口径说：`from_t`-式的
  `alias: &mut selected_group` 如今**在源码里自我暴露，且被形状守卫生效**，不是一句"错误无法表达"。
- 仪表盘弧线几何抽成可测函数，`draw_arc` 的参数墙收掉、**棘轮再降一条（7 → 6）**；
  四道门 **723 passed / 0 failed / 7 ignored**（718 → 723，多的 5 条全是用例）。
  原先 9 个位置参数里 `radius`/`start_deg`/`sweep_deg`/`from_t`/`to_t` 是**五个相邻 f32**，
  而"角度 vs [0,1] 归一化位置"的区别只活在签名上方一句注释里。改动前逐对普查：**四对对调全部
  编译零告警、`tool-panels` 209 条测试全绿**（gauge 原有 11 条用例一条都不碰弧线几何）。
  现在 `ArcSpec` + 纯函数 `arc_points(&ArcSpec, steps)`，`draw_arc` 只剩"取点 + 够两点才
  `painter.line`"四个参数，`#[allow(clippy::too_many_arguments)]` 删除（3 个调用点各自构造 `ArcSpec`）。
  **渲染没变是证出来的不是推出来的**：临时逐字符抄了一份 `git show HEAD` 的旧实现，在
  3×4×5×4×6×6×5 输入网格上按 `to_bits()` **逐位比了 1,520,640 个点、0 处不等**，然后把这份
  临时用例删掉（在测试里留第二份公式，迟早和实现各说各话）。我另做了一次独立复核：新函数体
  与旧循环逐行同序，只差 `spec.` 前缀。5 条新用例对四对相邻字段每一对都至少抓一条 —— 判据是
  去把 `arc_points` 里的字段读法改成对调、看哪几条红（最小偏离 90° 弧度 / 105 px 半径，
  浮点噪声 ≤1e-5，不是踩 epsilon 蒙过去的）。
  `steps == 0` 的 `0/0 = NaN` 形状**没有顺手"修"**：三处调用点共用一个 `arc_steps = 60` 绑定，
  0 不可达，真到 0 也因 `len() >= 2` 不画；现状用一条用例钉住，注释写明"要改是有意的决定"。
  **本轮明确没做到的那一半**（别读成"参数墙收掉=调用点有覆盖"）：危险搬了家 —— 我用一个合法的
  `ArcSpec` 字面量把 `sweep_deg`(270.0) 塞进底轨的 `from_t` 槽位，clippy 零告警、214 条测试全绿。
  新用例钉的是字段名在 `arc_points` 里的含义，没有东西钉 `paint` 往字段里填了什么；那一层需要
  `Painter`/`Ui` 驱动（第 31 轮那套事件注入只对纯判定管用），所以停手而没有再造一层抽象。
  守卫收到 6 条（3 clippy + 3 `dead_code`，计数从清单重数，顺手把上一轮遗留的"4 条"注释改对）；
  re-add 探针照旧：给 `draw_arc` 挂一条 clippy 完全看不见的冗余 `#[allow]` ⇒ 报
  "清单 6 条，实际 7 条"，移除后回绿。
- 自动滚动的**调用点**第一次有了真驱动的用例，第 30 轮普查里"六行全静默"变成"六对全被抓到"
  （四道门 718 passed / 0 failed / 7 ignored，基线 711 → 718，多的 7 条全是用例；
  **生产逻辑只改了一处**：`inner_rect.height()` 从各算一遍绑成一次 `viewport_height`，
  因为状态机与"该不该修复贴底"读的是同一个量，两处独立求值等于留一条漂移的口子）。
  `crates/panels/src/table.rs` 的测试模块加一套 `Frame` + `drive()`：按真实帧跑
  `AutoScrollState::update`，基准帧给定几何（视口 100 / 内容 1000 / 底部 = 900，
  两个高度必须不等否则那一对分辨不出），每只用例只改自己判别的一两个字段。
  **普查 A 层重跑：六对相邻同型字段每一对都至少被一条用例抓到**（最薄的一对是
  `content_height`↔`viewport_height`，恰好 2 条，删用例时得记得）；另外 5 个探针
  （`scroll_is_at_bottom` 的两处位置对调、`previous_offsets` 被收成共用一份、悬停注到别处、
  按下事件丢掉）也全红。变异仍然**全部编译得过** —— 抓它的只有运行时断言，这正是本轮要补的东西。
  **顺带查到的 egui 0.35 注入事实**（以后本仓写 UI 用例都要用）：`RawInput` 已经没有 `pointer`
  字段，`hover_pos()` / `primary_down()` 只能靠 `Event::PointerMoved` / `Event::PointerButton` 推；
  而 `PointerButton` 按下抬起都会顺带写下位置，所以"本帧无悬停"必须一个指针事件都不发 ——
  自检用例第一版因此是**假绿**（只用按下帧，破坏 `PointerMoved` 的变异照样通过），改成
  "悬停 / 悬停+按下 / 全新 Context 无事件"三种读回才咬得住；且 egui 跨 pass 保留指针状态，
  "无悬停"那一帧必须换新 `Context`。稳定性另测：`tool-panels` 连跑 15 次 0 红，
  单线程再跑一遍同样 209/0/1。
- 收掉自动滚动的参数墙，棘轮再降一条（8 → 7 条豁免）：`panels/lib.rs::next_auto_scroll_state`
  收 3 个相邻 bool + 4 个相邻 f32，全部相邻同型对调里 5 组零告警编译得过（只有 `bool`↔`f32`
  那组被 `E0308` 拦住）。四道门 711 passed / 0 failed / 7 ignored（与基线逐字相同，纯重构）。
  沿用第 29 轮式子：一个 `pub(crate) struct AutoScrollInputs`（字段名与原参数逐字相同，六个字段
  第一次有自己的注释），函数只剩一个参数，`#[allow]` 删除；函数体差异**只有一个空行**
  （解构那行之后），那条"滚轮向上不等 offset 同帧变化"的长注释原样保留。
  **普查在改动前的树上做、并且分两层**（A 换调用点实参、B 换签名形参），结论有两处和我原本的判断相反：
  ① A 层六行**全静默**，原因是全仓没有任何用例驱动过这个唯一的生产调用点
  （`table.rs` 的 `AutoScrollState::update`）—— 命名巧合连测试兜底都没有，比我以为的更严重；
  ② 真正会静默上线的一对**不是** `offset_y`↔`previous_offset_y`（那对至少有一条用例能区分），
  而是 **`content_height`↔`viewport_height`**：换过之后 `at_bottom` 恒真，
  而现有每条期望值**照旧成立**。另记一处结构保护不到的重复：`table.rs:241` 为算"该不该修复贴底"
  又独立按同一顺序把三个 f32 传给 `scroll_is_at_bottom`（3 参不触发该 lint，故无豁免亦无命名保护）。
  下一轮的补法已经查好：`table.rs` 测试模块本就用 `ctx.run_ui(RawInput::default(), …)` 驱动真 `Ui`，
  照此喂一个合成 hover 位置 + 滚轮增量，可把那六行从"静默"变成"对调即红"。
  守卫同步收到 7 条（4 clippy + 3 `dead_code`，计数从清单本身重数而不是从注释抄），
  探针仍是"在被移除那一行所在的文件里加一条 clippy 看不见的 `#[allow]`" ⇒ 实测报
  "清单 7 条，实际 8 条"后移除。
- 把网络 worker 的参数墙收进命名字段，**退掉 2 条 `too_many_arguments` 豁免（棘轮 10 → 8）**；
  四道门 711 passed / 0 failed / 7 ignored（与基线逐字相同 —— 纯重构，不新增用例也不删用例）。
  `transport/src/network.rs` 的 `spawn_network_worker` 与 `network_worker_loop` 收同一串 8 个
  位置参数，其中 `stop`/`alive`/`connecting` 是**三个相邻同型 `Arc<AtomicBool>`**。
  **先证明危险不是假想**：三种两两对调（stop↔alive、alive↔connecting、stop↔connecting）
  `cargo check --workspace` 全部零告警通过。**但"测试抓不抓得到"必须分开说**：
  `alive`↔`connecting` 是唯一初值相同的一对（都 `true`），它让 `cargo test -p tool-transport`
  **32/32 全绿**，只有 `crates/application/tests/headless.rs` 的网络用例抓得住；另两种会红，
  只是因为 `stop` 初值 `false`、换错后 worker 读到 `stop == true` 就在握手前退出 ——
  **是初值救了测试，不是测试设计到位**（这句别写成"测试覆盖了参数顺序"）。
  收成一个 `pub(crate) struct NetworkWorkerContext`（第 8 轮 `RowsViewInputs` 同一式子，
  无 trait/泛型/无 builder），两个函数各自只剩一个参数、两条 `#[allow]` 删除；调用点
  六个 `thread_*` 临时别名剩一个（`waker` 必须留在字面量外取，否则 `parking_lot` 的守卫会
  跨 `thread::spawn` 存活，与原代码同一做法）。三个标记第一次各自有了注释（原先只存在于名字里）。
  **准确的说法**：命名字段拦的是**写法**不是**取值** —— 位置排错在结构体字面量上没有语法、
  漏字段 E0063、类型不对 E0308（三条都实测过），但 `alive: Arc::clone(&connecting)` 这种同型
  对调照样编译得过（也实测了），只是必须把矛盾写在字段名旁边；这条限度写进了结构体 doc。
  守卫同步收紧：`EXPECTED` 去掉那两行、文件里"7 条 clippy / 10 条"的计数一起改，
  并把 `collected()` 举例用的"同文件同 lint 两条"改指到仍然成立的 `shared_keymap.rs`
  （原来那个例子正是被本轮删掉的两条）；重新加回一条 `#[allow]` 实测报
  "清单 8 条，实际 9 条"后移除。顺带自查：`grep allow` 会在 `send_plan.rs`、`sender.rs`
  各命中一行 —— 那是**注释里引用的字面量**，守卫跳过 `//` 开头的行，所以清单没有洞。
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

### 文档

- **文档集重启（第 61 轮）：docs/ 从零重写为 13 个文件——架构正文 + 已知差异登记册 +
  插件四件套 + 在线/性能/发布/安装文档 + 发布说明模板；旧的 148KB 架构台账与 75KB 执行计划不再继承。**
  （生产行为零变化；代码面唯一改动是生成物路径：`crates/extension/src/spec.rs` 两行。）
  * 新结构：`docs/README.md`（索引）、`docs/ARCHITECTURE.md`、`docs/known-gaps.md`、
    `docs/web.md`、`docs/performance.md`、`docs/RELEASE.md`、`docs/INSTALLER.md`、
    `docs/plugins/{README,manifest,lua-api,scenarios}.md`、`docs/plugins/spec.generated.md`（生成物）、
    `docs/releases/TEMPLATE.md`。
  * 不重建：`architecture-refactor-baseline.md`、`superpowers/plans/*`、历史 `releases/v1.0.0–v1.3.0.md`
    （历史由本 CHANGELOG 与 GitHub Releases 承担）；根 README 的旧发布说明链接改指 Releases 页。
  * 引用对账：代码/测试/CI 里所有指向旧文档的注释（「发送路径的残留差异」「预检的规则来源」
    「Linux clippy 缺口」「ExpectNoTx 半条契约」「Serial 臂权限修法」等）与根 README 链接逐一改指
    `docs/known-gaps.md` / 新路径；`plugin-spec.generated.md` 迁至 `docs/plugins/`，生成器与
    `include_str!` 用例随路径同步。
  * 验证：`cargo test -p tool-extension` 42 通过（生成物用例转绿）、`cargo check --workspace --all-targets`
    通过；README + docs 共 14 份 Markdown 的 63 条相对链接经脚本体检全部可解析；全仓旧路径清扫后
    只剩指向新文档的有意引用。

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
