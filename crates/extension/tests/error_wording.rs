//! `tool_extension::ExtensionError` 的中文文案契约。
//!
//! 状态栏上那句中文由 `translate_error` 决定（第 22 轮把 Enable/Disable 两条派发与
//! `reload_plugins` / `discover_plugins` 两个后台任务从 `.to_string()` 改到它）。本文件钉两件事：
//! 1. 八个变体各有其**逐字**的中文，没有哪个变体落进兜底文案；
//! 2. 「中文框架 + 外来细节」这一取舍没走样：OS / serde / mlua 的原文要留在句子里（否则
//!    没法排查），但英文 `Display` 的那层**框架**（`io error:` 一类）一个字都不许跟着进来，
//!    否则就成了「插件文件访问失败：io error: …」这种双重框架。
//!
//! 与 `crates/platform/tests/transport_error_wording.rs` 同一形状：一格一个判定，
//! 期望值全部写成字面量，不引用实现的常量。

use std::io;

use tool_extension::{ExtensionError, translate_error};
use tool_lua_host::LuaHostError;

/// 外来细节：按契约原样带出，所以它出现在期望值里是**设计**，不是漏译。
const IO_DETAIL: &str = "No such file or directory (os error 2)";
const MANIFEST_DETAIL: &str = "EOF while parsing an object at line 1 column 1";
/// `Lua(LuaHostError::Lua)` 那一臂的中文框架（细节是 mlua 的原文，钉不住，只钉框架）。
const LUA_FRAME: &str = "插件脚本错误：";

const NOT_FOUND: &str = "未找到插件 demo.missing（请先重新扫描插件目录）";
const ALREADY_ENABLED: &str = "插件 demo.probe 已在运行（无需重复启用）";
const STOPPING: &str = "插件 demo.probe 正在停止，请稍候再启用";
const UNSUPPORTED_RUNTIME: &str = "不支持的插件运行时 nodejs（请检查 plugin.json 的 runtime 字段）";
const PERMISSION_DENIED: &str =
    "插件 demo.probe 声明了宿主不允许的权限 filesystem（请检查 plugin.json 的 permissions 字段）";
const ALREADY_RUNNING: &str = "插件脚本已在运行（请等待上一次启用结束）";

/// 一格 = 一个变体：`name` 与 `variant_name` 对账，`expected` 是逐字中文，
/// `english_frame` 是该变体英文 `Display` 里的框架部分（payload 为空的变体就是整句），
/// 期望中文里不得出现它。
struct Case {
    name: &'static str,
    error: ExtensionError,
    expected: &'static str,
    english_frame: &'static str,
}

fn io_failure() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, IO_DETAIL)
}

fn manifest_failure() -> serde_json::Error {
    serde_json::from_str::<serde_json::Value>("{").expect_err("这段文本不是合法 JSON")
}

fn all_variants() -> Vec<Case> {
    vec![
        Case {
            name: "Io",
            error: ExtensionError::Io(io_failure()),
            expected: "插件文件访问失败：No such file or directory (os error 2)",
            english_frame: "io error:",
        },
        Case {
            name: "Manifest",
            error: ExtensionError::Manifest(manifest_failure()),
            expected: "manifest 解析失败：EOF while parsing an object at line 1 column 1",
            english_frame: "manifest parse error:",
        },
        Case {
            name: "NotFound",
            error: ExtensionError::NotFound("demo.missing".to_owned()),
            expected: NOT_FOUND,
            english_frame: "plugin 'demo.missing' was not found",
        },
        Case {
            name: "AlreadyEnabled",
            error: ExtensionError::AlreadyEnabled("demo.probe".to_owned()),
            expected: ALREADY_ENABLED,
            english_frame: "plugin 'demo.probe' is already enabled",
        },
        Case {
            name: "Stopping",
            error: ExtensionError::Stopping("demo.probe".to_owned()),
            expected: STOPPING,
            english_frame: "plugin 'demo.probe' is still shutting down",
        },
        Case {
            name: "UnsupportedRuntime",
            error: ExtensionError::UnsupportedRuntime("nodejs".to_owned()),
            expected: UNSUPPORTED_RUNTIME,
            english_frame: "unsupported runtime 'nodejs'",
        },
        Case {
            name: "PermissionDenied",
            error: ExtensionError::PermissionDenied {
                plugin_id: "demo.probe".to_owned(),
                permission: "filesystem".to_owned(),
            },
            expected: PERMISSION_DENIED,
            english_frame: "permission 'filesystem' is not allowed for plugin 'demo.probe'",
        },
        Case {
            name: "Lua",
            error: ExtensionError::Lua(LuaHostError::AlreadyRunning),
            expected: ALREADY_RUNNING,
            english_frame: "script is already running",
        },
    ]
}

/// 变体名清单**刻意不写 `_ =>` 分支**：给 `ExtensionError` 加一个变体而忘了配中文，
/// 这里与 `translate_error` 一起编译不过（而不是悄悄漏一句英文给用户）。
fn variant_name(err: &ExtensionError) -> &'static str {
    match err {
        ExtensionError::Io(_) => "Io",
        ExtensionError::Manifest(_) => "Manifest",
        ExtensionError::NotFound(_) => "NotFound",
        ExtensionError::AlreadyEnabled(_) => "AlreadyEnabled",
        ExtensionError::Stopping(_) => "Stopping",
        ExtensionError::UnsupportedRuntime(_) => "UnsupportedRuntime",
        ExtensionError::PermissionDenied { .. } => "PermissionDenied",
        ExtensionError::Lua(_) => "Lua",
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'))
}

#[test]
fn io_failure_keeps_the_os_detail_behind_a_chinese_frame() {
    assert_eq!(
        translate_error(&ExtensionError::Io(io_failure())),
        "插件文件访问失败：No such file or directory (os error 2)"
    );
}

#[test]
fn manifest_failure_keeps_the_serde_detail_behind_a_chinese_frame() {
    // 这一臂当前不会经状态栏到达用户（`scan_roots` 把它折成 `manifest_parse_error`
    // 诊断），但那条诊断内插的正是它的 `Display`，所以措辞同样要有测试兜着。
    assert_eq!(
        translate_error(&ExtensionError::Manifest(manifest_failure())),
        "manifest 解析失败：EOF while parsing an object at line 1 column 1"
    );
}

#[test]
fn unknown_plugin_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&ExtensionError::NotFound("demo.missing".to_owned())),
        NOT_FOUND
    );
}

#[test]
fn already_enabled_plugin_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&ExtensionError::AlreadyEnabled("demo.probe".to_owned())),
        ALREADY_ENABLED
    );
}

#[test]
fn stopping_plugin_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&ExtensionError::Stopping("demo.probe".to_owned())),
        STOPPING
    );
}

#[test]
fn unsupported_runtime_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&ExtensionError::UnsupportedRuntime("nodejs".to_owned())),
        UNSUPPORTED_RUNTIME
    );
}

#[test]
fn undeclared_permission_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&ExtensionError::PermissionDenied {
            plugin_id: "demo.probe".to_owned(),
            permission: "filesystem".to_owned(),
        }),
        PERMISSION_DENIED
    );
}

#[test]
fn already_running_script_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&ExtensionError::Lua(LuaHostError::AlreadyRunning)),
        ALREADY_RUNNING
    );
}

/// `LuaHostError::Lua(mlua::Error)` 只能由真实 Lua VM 造出来：`mlua` 不是本 crate 的
/// 依赖，测试里连那个类型都写不出来。所以这里跑一段语法不通的脚本，拿一个**真的**
/// mlua 失败来钉这一臂 —— 细节是第三方的原文，钉不住，能钉的是框架。
fn real_mlua_failure() -> LuaHostError {
    let bus = tool_databus::DataBus::new();
    let transport = tool_transport::TransportManager::new(bus.clone());
    tool_lua_host::run_script_for_test("this is not lua ~~", bus, transport)
        .expect_err("语法不通的脚本必须以 LuaHostError 失败")
}

#[test]
fn lua_script_failure_is_framed_once_not_twice() {
    let host_error = real_mlua_failure();
    // `LuaHostError::Lua` 的 Display 自带「lua error: 」：把它整句塞进中文框架，
    // 用户就读到「插件脚本错误：lua error: lua runtime error: …」。
    let whole_english_display = host_error.to_string();
    let message = translate_error(&ExtensionError::Lua(host_error));

    assert!(
        has_cjk(&message),
        "Lua 臂必须给中文文案，实际 {message:?}（落进兜底/未翻译就是这个形状）"
    );
    assert!(
        !message.contains(&whole_english_display),
        "Lua 臂把 `LuaHostError` 的英文 Display 整句带了进来，双重框架：{message:?}"
    );
    let detail = message.strip_prefix(LUA_FRAME).unwrap_or_else(|| {
        panic!("Lua 臂必须是「{LUA_FRAME} + mlua 原文」的形状，实际 {message:?}")
    });
    assert!(
        !detail.trim().is_empty(),
        "mlua 的原文被吞掉了，用户将无法排查脚本错误：{message:?}"
    );
}

/// 两个不同概念不得 collapse 成一句话：`PermissionDenied` 说的是「插件在 plugin.json 里
/// 声明了宿主白名单外的权限」，与 `crates/transport` 那句「浏览器拒绝了串口授权请求」
/// （Web Serial 弹窗被用户点掉）毫无关系。这里不 import 那边的文案，只钉住边界。
#[test]
fn undeclared_permission_is_not_the_browser_serial_authorisation_sentence() {
    let browser_denial = "浏览器拒绝了串口授权请求";
    let message = translate_error(&ExtensionError::PermissionDenied {
        plugin_id: "demo.probe".to_owned(),
        permission: "filesystem".to_owned(),
    });

    assert_ne!(message, browser_denial);
    assert!(
        !message.contains(browser_denial),
        "插件权限声明错误被写成了浏览器授权失败：{message:?}"
    );
    assert!(
        message.contains("demo.probe") && message.contains("filesystem"),
        "这句必须说清是哪个插件的哪个权限：{message:?}"
    );
}

#[test]
fn every_variant_has_its_own_chinese_message_and_none_falls_through() {
    let table = all_variants();

    // ① 清单就是枚举的全部：变体名互不相同，且与 `variant_name` 的穷尽匹配一致。
    let listed: Vec<&'static str> = table.iter().map(|case| case.name).collect();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        table.len(),
        "清单里出现了重复变体，某一格其实没被测到：{listed:?}"
    );
    for case in &table {
        assert_eq!(
            variant_name(&case.error),
            case.name,
            "清单标签与 `variant_name` 对不上，说明有一格测的不是它声称的变体"
        );
    }

    for case in &table {
        let message = translate_error(&case.error);
        assert_eq!(message, case.expected, "{} 的中文文案被改动了", case.name);
        assert!(
            has_cjk(&message),
            "{} 必须给出中文文案，实际 {message:?}（落进兜底/未翻译就是这个形状）",
            case.name
        );
        // 英文框架不得成为文案的一部分：那是本轮要消掉的那条差异。
        assert!(
            !message.contains(case.english_frame),
            "{} 的中文里混进了英文 Display 框架 {:?}：{message:?}",
            case.name,
            case.english_frame
        );
    }

    // ② 八个变体八个说法：任何两条相同就意味着其中一条其实是兜底文案。
    let mut messages: Vec<&str> = table.iter().map(|case| case.expected).collect();
    messages.sort_unstable();
    let distinct = messages.len();
    messages.dedup();
    assert_eq!(
        messages.len(),
        distinct,
        "有两个变体共用一句文案，其中至少一个是兜底的：{messages:?}"
    );
}

/// 「中文框架 + 外来细节」里的细节不能顺手被翻译掉：OS / serde 的原文是排查线索
/// （与 `tool_transport` 的 `Io` 臂同一取舍，如 `操作超时：read timed out`）。
#[test]
fn foreign_details_survive_their_chinese_frames() {
    let io_message = translate_error(&ExtensionError::Io(io_failure()));
    assert!(
        io_message.contains(IO_DETAIL),
        "OS 的错误原文被丢了，用户将无法排查：{io_message:?}"
    );

    let manifest_message = translate_error(&ExtensionError::Manifest(manifest_failure()));
    assert!(
        manifest_message.contains(MANIFEST_DETAIL),
        "serde 的位置线索被丢了，用户将无法排查：{manifest_message:?}"
    );
}
