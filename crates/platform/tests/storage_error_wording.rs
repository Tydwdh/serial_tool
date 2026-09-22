//! `tool_platform::storage::StorageError` 的中文文案契约。
//!
//! 存储错误到用户眼前的那条路是 `write_exported_content` → `TaskSnapshot::message`
//! （状态栏），第 25 轮之前它走的是 `.to_string()`，于是桌面端导出失败读起来是一句
//! `storage operation failed: …`。本文件钉三件事：
//! 1. 三个变体各有其**逐字**的中文，没有哪个变体落进兜底文案；
//! 2. `Operation` 的载荷**全部**是英文/外来原文（生产者清单记在
//!    `every_operation_payload_is_foreign_and_takes_exactly_one_frame` 的抬头里），
//!    所以只套**一层**中文框架，绝不出现「存储操作失败：存储操作失败：…」；
//! 3. 它与 `tool_platform::translate_error`（`TransportError` 那一份）**不共用任何句子**——
//!    存储与传输是两种能力，撞车就等于骗用户去看串口。
//!
//! 与 `crates/platform/tests/transport_error_wording.rs`、
//! `crates/extension/tests/error_wording.rs` 同一形状：一格一个判定，
//! 期望值全部写成字面量，不引用实现的常量。

use tool_platform::TransportError;
use tool_platform::storage::{StorageError, translate_error};

const UNSUPPORTED: &str = "此平台不支持该存储能力";
const NOT_FOUND: &str = "未找到存储项 settings.json（它还没有被保存过）";
/// `Operation` 的框架：载荷原样跟在它后面，一个字的改写都没有。
const OPERATION_FRAME: &str = "存储操作失败：";

fn all_variants() -> Vec<(&'static str, StorageError, &'static str)> {
    vec![
        ("Unsupported", StorageError::Unsupported, UNSUPPORTED),
        (
            "NotFound",
            StorageError::NotFound("settings.json".to_owned()),
            NOT_FOUND,
        ),
        (
            "Operation",
            StorageError::Operation("native storage worker stopped".to_owned()),
            // 框架 + 原文：期望值写成逐字整句，别写成 `OPERATION_FRAME` 拼接，
            // 否则改框架时会连它一起改掉，测不出「框架被动了」。
            "存储操作失败：native storage worker stopped",
        ),
    ]
}

/// 变体名清单**刻意不写 `_ =>` 分支**：给 `StorageError` 加一个变体而忘了配中文，
/// 这里就编译不过（而不是悄悄落进兜底文案）。这是「三个变体全覆盖」的机器形式。
fn variant_name(err: &StorageError) -> &'static str {
    match err {
        StorageError::Unsupported => "Unsupported",
        StorageError::NotFound(_) => "NotFound",
        StorageError::Operation(_) => "Operation",
    }
}

fn has_cjk(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}'))
}

#[test]
fn unsupported_capability_gets_its_own_chinese_message() {
    assert_eq!(translate_error(&StorageError::Unsupported), UNSUPPORTED);
}

#[test]
fn missing_key_names_itself_and_gets_its_own_chinese_message() {
    assert_eq!(
        translate_error(&StorageError::NotFound("settings.json".to_owned())),
        NOT_FOUND
    );
}

#[test]
fn operation_detail_is_passed_through_under_exactly_one_chinese_frame() {
    assert_eq!(
        translate_error(&StorageError::Operation(
            "native storage worker stopped".to_owned()
        )),
        format!("{OPERATION_FRAME}native storage worker stopped")
    );
}

/// `Operation` 的**全部**载荷（按生产者逐条列，出处见 `crates/platform/src/storage.rs`）。
/// 这张表是「只套一层框架」这条决定的凭据：只要有一条已经是中文框架，映射器就该改成
/// 原样返回（transport 侧那样），否则用户会读到双重框架。
///
/// - `native::spawn_io`：`native storage worker stopped` —— 英文常量（IO worker 线程退出）；
/// - `native::io_error`（`load` / `save` / `remove` / `write_path` / `FileService` 的
///   `read`+`write` 全走它）：`std::io::Error` 的 `Display` —— **英文框架 + 按系统 locale
///   本地化的 OS 细节**，没有中文框架，所以套一层是对的（本机实测：
///   `系统找不到指定的路径。 (os error 3)`，尾部 `(os error 3)` 仍是英文）；
/// - `native::ensure_relative`：`file id escapes storage root` —— 英文常量
///   （文件 id 越过存储根目录的那道防越界判定）；
/// - `web::WebSettingsStore::from_window`：`window unavailable` —— 英文常量；
/// - `web::js_error`：JS 值原文（`as_string()`，取不到就 `{:?}`）—— 外来文本；
/// - `web::decode_settings`：`decode settings: {serde_json 原文}` —— 英文框架；
/// - `web::save`：`serde_json::to_string` 的错误原文 —— 英文。
///
/// 表里的原文按契约**不得被改写**，所以每格两句：框架逐字在最前，载荷逐字跟在后面。
#[test]
fn every_operation_payload_is_foreign_and_takes_exactly_one_frame() {
    let payloads = [
        "native storage worker stopped",
        "file id escapes storage root",
        "window unavailable",
        "decode settings: expected value at line 1 column 1",
        "系统找不到指定的路径。 (os error 3)",
        "SecurityError: The operation is insecure.",
    ];
    for payload in payloads {
        let message = translate_error(&StorageError::Operation(payload.to_owned()));
        assert_eq!(
            message,
            format!("{OPERATION_FRAME}{payload}"),
            "载荷 {payload:?} 被改写了：它原样跟着框架走才是契约"
        );
        assert_eq!(
            message.matches(OPERATION_FRAME).count(),
            1,
            "框架 {OPERATION_FRAME} 在 {message:?} 里出现了不止一次 —— \
             有生产者开始自己写中文框架了，按 transport 侧的规矩把映射器改成原样返回"
        );
    }
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

    // ② 三个变体三个说法：任何两条相同就意味着其中一条其实是兜底文案。
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

/// 两个映射器各管一个枚举，**句子必须互不通用**：`Unsupported` 是两个枚举都有的形状，
/// 撞车的话用户在导出失败时会看到「不支持该传输能力」——那是第 22 轮给插件权限句
/// 立过的那条避让规则，这里把它从「读代码时注意」变成机器判定。
#[test]
fn the_storage_mapper_never_borrows_a_transport_sentence() {
    let storage_words = translate_error(&StorageError::Unsupported);
    // 全路径写出来：本文件顶部 import 的是 storage 那一份同名函数，这一行调的是
    // crate 根的 `TransportError` 那一份。
    let transport_words = tool_platform::translate_error(&TransportError::Unsupported);
    assert_eq!(transport_words, "此平台不支持该传输能力");
    assert_ne!(
        storage_words, transport_words,
        "存储与传输共用了同一句「不支持」，用户分不清是哪一种能力不可用"
    );

    // transport 侧每一句中文，都不许出现在本枚举的任何一句里（相等算最严重的那种）。
    let transport_cases = [
        TransportError::Unsupported,
        TransportError::NoPortAvailable,
        TransportError::PortNotKnown(tool_platform::PortId::new("serial:COM3")),
        TransportError::PortNotConnected(tool_platform::PortId::new("serial:COM3")),
        TransportError::PermissionDenied,
        TransportError::Operation("发送队列已满：发送过快，请降低频率".to_owned()),
    ];
    let transport_messages: Vec<String> = transport_cases
        .iter()
        .map(tool_platform::translate_error)
        .collect();
    for (name, _, expected) in all_variants() {
        for transport in &transport_messages {
            assert_ne!(
                expected,
                transport.as_str(),
                "StorageError 的 {name} 与 transport 侧某句逐字相同：{transport}"
            );
            assert!(
                !expected.contains(transport.as_str()),
                "StorageError 的 {name} 把 transport 侧那句 {transport:?} 装了进来"
            );
        }
    }
}
