//! `tool_platform::TransportError` 的中文文案契约。
//!
//! 平台错误枚举是**两端都编译**的那一份（`tool-transport` 只在 native 编译），所以状态栏
//! 里最终出现的那句中文只能由 `tool_platform::translate_error` 决定。本文件钉两件事：
//! 1. 六个变体各有其**逐字**的中文，没有哪个变体落进兜底文案；
//! 2. 它与 `tool_transport::translate_error` 在重叠的那些失败上**说的是同一句话**
//!    （两个枚举、一个用户概念；文案改一侧必须同时改另一侧，否则这里变红）。
//!
//! 与 `crates/panels/src/sender.rs`、`crates/application/src/send_plan.rs` 的表格同一形状：
//! 一格一个判定，期望值全部写成字面量，不引用实现的常量。

use tool_platform::{PortId, TransportError, translate_error};

const UNSUPPORTED: &str = "此平台不支持该传输能力";
const NO_PORT_AVAILABLE: &str = "没有可用的串口（请先连接设备或申请端口）";
const PORT_NOT_KNOWN: &str = "未知串口 serial:COM3（授权列表里没有它，请重新申请端口）";
const PORT_NOT_CONNECTED: &str = "串口 serial:COM3 未打开（可能已断开，请重连）";
const PERMISSION_DENIED: &str = "浏览器拒绝了串口授权请求";

/// `tool_transport::translate_error` 对 `QueueFull` 的那句话，逐字抄来。
/// 它同时是本文件里 `Operation` 的载荷 —— 桌面端 `operation_error` 放进来正是这种东西。
const QUEUE_FULL: &str = "发送队列已满：发送过快，请降低频率";

fn port() -> PortId {
    PortId::new("serial:COM3")
}

fn all_variants() -> Vec<(&'static str, TransportError, &'static str)> {
    vec![
        ("Unsupported", TransportError::Unsupported, UNSUPPORTED),
        (
            "NoPortAvailable",
            TransportError::NoPortAvailable,
            NO_PORT_AVAILABLE,
        ),
        (
            "PortNotKnown",
            TransportError::PortNotKnown(port()),
            PORT_NOT_KNOWN,
        ),
        (
            "PortNotConnected",
            TransportError::PortNotConnected(port()),
            PORT_NOT_CONNECTED,
        ),
        (
            "PermissionDenied",
            TransportError::PermissionDenied,
            PERMISSION_DENIED,
        ),
        (
            "Operation",
            TransportError::Operation(QUEUE_FULL.to_owned()),
            QUEUE_FULL,
        ),
    ]
}

/// 变体名清单**刻意不写 `_ =>` 分支**：给 `TransportError` 加一个变体而忘了配中文，
/// 这里就编译不过（而不是悄悄落进兜底文案）。这是「六个变体全覆盖」的机器形式。
fn variant_name(err: &TransportError) -> &'static str {
    match err {
        TransportError::Unsupported => "Unsupported",
        TransportError::NoPortAvailable => "NoPortAvailable",
        TransportError::PortNotKnown(_) => "PortNotKnown",
        TransportError::PortNotConnected(_) => "PortNotConnected",
        TransportError::PermissionDenied => "PermissionDenied",
        TransportError::Operation(_) => "Operation",
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'))
}

#[test]
fn unsupported_capability_gets_its_own_chinese_message() {
    assert_eq!(translate_error(&TransportError::Unsupported), UNSUPPORTED);
}

#[test]
fn no_available_port_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&TransportError::NoPortAvailable),
        NO_PORT_AVAILABLE
    );
}

#[test]
fn unknown_port_names_itself_and_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&TransportError::PortNotKnown(port())),
        PORT_NOT_KNOWN
    );
}

#[test]
fn unconnected_port_gets_the_same_words_transport_uses_for_a_closed_port() {
    assert_eq!(
        translate_error(&TransportError::PortNotConnected(port())),
        PORT_NOT_CONNECTED
    );
}

#[test]
fn browser_permission_denial_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&TransportError::PermissionDenied),
        PERMISSION_DENIED
    );
}

#[test]
fn operation_detail_is_passed_through_without_a_second_frame() {
    // 载荷按契约已是用户可见文案（native 侧由 `operation_error` 翻译过）；
    // 再套一层中文就成了「操作失败：串口错误：…」那样的同义反复。
    assert_eq!(
        translate_error(&TransportError::Operation(QUEUE_FULL.to_owned())),
        QUEUE_FULL
    );
}

#[test]
fn every_variant_has_its_own_chinese_message_and_none_falls_through() {
    let table = all_variants();

    // ① 清单就是枚举的全部：变体名互不相同，且与 `variant_name` 的穷尽匹配一致。
    let listed: Vec<&'static str> = table.iter().map(|(name, ..)| *name).collect();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        table.len(),
        "清单里出现了重复变体，某一格其实没被测到：{listed:?}"
    );
    for (name, error, _) in &table {
        assert_eq!(
            variant_name(error),
            *name,
            "清单标签与 `variant_name` 对不上，说明有一格测的不是它声称的变体"
        );
    }

    for (name, error, expected) in &table {
        let message = translate_error(error);
        assert_eq!(&message, expected, "{name} 的中文文案被改动了");
        assert!(
            has_cjk(&message),
            "{name} 必须给出中文文案，实际 {message:?}（落进兜底/未翻译就是这个形状）"
        );
        // 英文 `Display` 不得成为文案的一部分：那是本轮要消掉的那条差异。
        assert!(
            !message.contains(&error.to_string()),
            "{name} 的中文里混进了英文 Display 原文：{message:?}"
        );
    }

    // ② 六个变体六个说法：任何两条相同就意味着其中一条其实是兜底文案。
    let mut messages: Vec<&str> = table.iter().map(|(_, _, msg)| *msg).collect();
    messages.sort_unstable();
    let distinct = messages.len();
    messages.dedup();
    assert_eq!(
        messages.len(),
        distinct,
        "有两个变体共用一句文案，其中至少一个是兜底的：{messages:?}"
    );
}

/// 只有 transport 侧才有的那些失败，在桌面端是**经 `Operation` 载荷**到达用户的。
/// 每格钉两句：transport 侧的中文就是 `期望`，且它经平台侧翻译后逐字不变
/// —— 于是「改任一侧的文案」都会变红，而不是悄悄分叉。
///
/// `Serial(serialport::Error)` 不在此列：`tool-platform` 拿不到那个类型
/// （`serialport` 不是本 crate 的依赖，`tool_transport::sp` 也是私有的），
/// 它的中文由 `crates/transport` 自己的 `translate_error_covers_all_variants` 钉住。
#[test]
#[cfg(not(target_arch = "wasm32"))]
fn the_two_mappers_agree_on_every_overlapping_failure() {
    // ① 「端口没开」在两个枚举里各有一个变体。
    assert_eq!(
        translate_error(&TransportError::PortNotConnected(port())),
        tool_transport::translate_error(&tool_transport::TransportError::PortNotOpen(
            "serial:COM3".to_owned()
        )),
        "同一件事（要用的端口没打开）在两个枚举里说了两句话"
    );

    // ② 其余重叠部分。
    let cases: &[(&str, tool_transport::TransportError)] = &[
        (QUEUE_FULL, tool_transport::TransportError::QueueFull),
        ("未打开任何串口", tool_transport::TransportError::NoOpenPort),
        (
            "串口 COM3 未打开（可能已断开，请重连）",
            tool_transport::TransportError::PortNotOpen("COM3".to_owned()),
        ),
        (
            "串口工作线程已关闭（可能已断开，请重连）",
            tool_transport::TransportError::WorkerClosed,
        ),
        (
            "无效HEX：无效字符 'Z'",
            tool_transport::TransportError::InvalidHex("无效字符 'Z'".to_owned()),
        ),
        (
            "操作超时：read timed out",
            tool_transport::TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "read timed out",
            )),
        ),
    ];
    for (expected, case) in cases {
        let transport_words = tool_transport::translate_error(case);
        assert_eq!(
            transport_words, *expected,
            "transport 侧对 {case:?} 的中文变了，平台侧（或本用例）没跟上"
        );
        assert_eq!(
            translate_error(&TransportError::Operation(transport_words.clone())),
            transport_words,
            "transport 变体 {case:?} 的中文经 `Operation` 到达平台侧后被改写了"
        );
    }
}
