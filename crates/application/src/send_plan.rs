//! 发送命令 → (任务种类, 待投递字节) 的唯一决策点。
//!
//! 本模块**不受 `cfg(target_arch)` 门控**：native 的 `Workbench::dispatch` 与 wasm 的
//! `WebApplication::dispatch` 过去各自实现一遍「这条命令算 serial 还是 network」以及
//! 「HEX 怎么解码」，两边已经分叉到同一串 HEX 在 native 合法、在 web 非法（web 的手抄
//! 解析器只剥一层 `0x` 前缀、严格模式只按空白分词）。路由与解码的判定收在这里，
//! **字节如何真正投递仍由各平台负责**：native 走 `spawn_ordered` + transport/backend，
//! wasm 走 `spawn` + Web Serial/网络端口，两者的状态提示与事件发布也各自保留。
//!
//! HEX 规则本身在 `tool_core::{parse_hex, parse_hex_strict}`（见 [`decode_hex`]）。

use crate::command::AppCommand;
use std::fmt;

/// 网络端口的任务种类标签。字面值必须与 `spawn_ordered(.., "send_network", ..)` 以及
/// `crates/application/tests/headless.rs` 的 `task_kind` 断言逐字一致，
/// 由 `task_kind_literals_are_the_ones_the_task_registry_sees` 钉住。
pub const NETWORK_TASK_KIND: &str = "send_network";

/// 普通串口的任务种类标签，同上。
pub const SERIAL_TASK_KIND: &str = "send_serial";

/// 路由结果：任务种类标签与实际待发送的字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedSend {
    pub task_kind: &'static str,
    pub bytes: Vec<u8>,
}

impl PlannedSend {
    /// 本计划是否投向网络端口。平台据此选投递后端，不再各自判定一次。
    pub fn targets_network_port(&self) -> bool {
        self.task_kind == NETWORK_TASK_KIND
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendPlanError {
    /// HEX 内容非法（严格模式下的奇数 nibble 等）。载荷是 `tool_core` 的裸文案。
    InvalidHex(String),
    /// 命令不是发送类命令：`plan_send` 只服务 `SendText`/`SendHex`/`SendRaw`。
    NotASend,
}

impl fmt::Display for SendPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHex(message) => write!(f, "HEX 解析失败：{message}"),
            Self::NotASend => write!(f, "该命令不是发送类命令"),
        }
    }
}

impl std::error::Error for SendPlanError {}

/// HEX 解码：`strict` 决定用哪一档规则，两档都在 `tool_core` 里。
///
/// `plan_send`（真正发送）与两平台的 `validate_hex`（presentation 预检）共用本函数，
/// 所以「按钮亮起 → dispatch 却报错」这类分叉不再有第二个判定来源。
pub fn decode_hex(hex: &str, strict: bool) -> Result<Vec<u8>, String> {
    if strict {
        tool_core::parse_hex_strict(hex)
    } else {
        tool_core::parse_hex(hex)
    }
}

/// `is_network` 决定任务种类；字节编码规则两平台共用。
pub fn plan_send(command: &AppCommand, is_network: bool) -> Result<PlannedSend, SendPlanError> {
    let bytes = match command {
        AppCommand::SendText { text, .. } => text.as_bytes().to_vec(),
        AppCommand::SendHex { hex, strict, .. } => {
            decode_hex(hex, *strict).map_err(SendPlanError::InvalidHex)?
        }
        AppCommand::SendRaw { bytes, .. } => bytes.clone(),
        _ => return Err(SendPlanError::NotASend),
    };
    Ok(PlannedSend {
        task_kind: if is_network {
            NETWORK_TASK_KIND
        } else {
            SERIAL_TASK_KIND
        },
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_platform::PortId;

    fn hex(value: &str, strict: bool) -> AppCommand {
        AppCommand::SendHex {
            port: PortId::new("COM1"),
            hex: value.to_owned(),
            strict,
        }
    }

    #[test]
    fn network_flag_selects_the_task_kind() {
        let command = AppCommand::SendText {
            port: PortId::new("COM1"),
            text: "AT\r\n".into(),
        };
        assert_eq!(
            plan_send(&command, false).unwrap(),
            PlannedSend {
                task_kind: "send_serial",
                bytes: b"AT\r\n".to_vec()
            }
        );
        assert_eq!(plan_send(&command, true).unwrap().task_kind, "send_network");
        assert!(!plan_send(&command, false).unwrap().targets_network_port());
        assert!(plan_send(&command, true).unwrap().targets_network_port());
    }

    /// 真值表的一行：`(输入, 严格模式期望, 宽松模式期望)`，`None` = 该档必须拒绝。
    /// 起名字是为了把 `clippy::type_complexity` 消除在定义处 —— 用 `type` 别名而不是
    /// `#[allow]`：放宽注解会把这条 lint 从整个测试模块上关掉，而下文还有别的表。
    type HexVerdict = (&'static str, Option<Vec<u8>>, Option<Vec<u8>>);

    /// 真值表就是分叉点：每格都钉死「严格/宽松各自接不接受、接受时是哪几个字节」。
    /// `("AB C", ...)` 那格是宽松模式的单 nibble 左补 0 规则，native 一直如此，
    /// web 收敛后也必须如此（`Task 5` 的字节契约在对端验的就是它）。
    #[test]
    fn strict_and_lenient_have_exactly_one_verdict_each() {
        let table: &[HexVerdict] = &[
            // 规范写法：两档都收。
            ("AB CD", Some(vec![0xAB, 0xCD]), Some(vec![0xAB, 0xCD])),
            // 紧凑奇数长度：严格模式按 normalize 后的长度判 3 → 拒；宽松左补 0 后分块。
            ("abc", None, Some(vec![0x0A, 0xBC])),
            // 紧凑偶数长度：严格模式仍按「每 token 恰 2 字符」判 → 拒；宽松分块成两字节。
            ("abcd", None, Some(vec![0xAB, 0xCD])),
            // 单 nibble：严格拒绝、宽松左补 0 —— 不是右补（0xC0）。
            ("AB C", None, Some(vec![0xAB, 0x0C])),
            // 非 HEX 字符：两档都拒。
            ("ZZ", None, None),
            // 分隔符 `,`/`;` 与空白等价：两档都收。此前 web 的严格分支只按空白分词，
            // "0A,BB" 在 web 被拒、在 native 放行。
            ("0A,BB", Some(vec![0x0A, 0xBB]), Some(vec![0x0A, 0xBB])),
            ("0A;BB", Some(vec![0x0A, 0xBB]), Some(vec![0x0A, 0xBB])),
            // 重复 `0x` 前缀：`normalize_hex_token` 用 trim_start_matches 连续剥，
            // 剥完才是 "AB"。此前 web 只剥一层，"0x0xAB" 在 web 被拒。
            ("0x0xAB", Some(vec![0xAB]), Some(vec![0xAB])),
            // `_`/`-` 分隔符：normalize 后长度 4 → 严格拒绝、宽松分块。
            ("AA_BB-CC", None, Some(vec![0xAA, 0xBB, 0xCC])),
            // 空输入：两档都拒（宽松报 "empty input"）。
            ("   ", None, None),
        ];
        for (input, strict_expect, lenient_expect) in table {
            check_cell(input, strict_expect.as_deref(), true);
            check_cell(input, lenient_expect.as_deref(), false);
        }
    }

    fn check_cell(input: &str, expect: Option<&[u8]>, strict: bool) {
        let mode = if strict { "严格" } else { "宽松" };
        match (plan_send(&hex(input, strict), false), expect) {
            (Ok(plan), Some(bytes)) => assert_eq!(
                plan.bytes.as_slice(),
                bytes,
                "{input:?} 的{mode}模式解码结果变了：{:02X?}",
                plan.bytes
            ),
            (Err(SendPlanError::InvalidHex(_)), None) => {
                // 预期内的拒绝。
            }
            (Err(other), None) => {
                panic!("{input:?} 的{mode}模式应当报 HEX 错误，实际 {other:?}")
            }
            (Err(other), Some(_)) => panic!(
                "{input:?} 的{mode}模式本应放行，却报 {other:?}（构造的命令不是 SendHex，判定分支走错）"
            ),
            (Ok(plan), None) => panic!(
                "{input:?} 的{mode}模式本应被拒，却放行成 {:02X?}（放宽判定=改变发送契约）",
                plan.bytes
            ),
        }
    }

    #[test]
    fn bytes_are_decoded_not_passed_through() {
        let plan = plan_send(&hex("AB CD", true), false).unwrap();
        assert_eq!(plan.bytes, vec![0xAB, 0xCD], "HEX 必须解码为两字节");
        // 文本命令绝不经过 HEX 解码：否则 "AB" 会发成 1 字节。
        let text = AppCommand::SendText {
            port: PortId::new("COM1"),
            text: "AB CD".to_owned(),
        };
        assert_eq!(
            plan_send(&text, false).unwrap().bytes,
            b"AB CD".to_vec(),
            "SendText 必须原文投递"
        );
        // SendRaw 的字节一个都不动。
        let raw = AppCommand::SendRaw {
            port: PortId::new("COM1"),
            bytes: vec![0x00, 0xAB, 0xFF],
        };
        assert_eq!(
            plan_send(&raw, false).unwrap().bytes,
            vec![0x00, 0xAB, 0xFF]
        );
    }

    #[test]
    fn non_send_commands_are_rejected() {
        let command = AppCommand::SetDtr {
            port: PortId::new("COM1"),
            value: true,
        };
        assert_eq!(plan_send(&command, false), Err(SendPlanError::NotASend));
    }

    /// 字面值本身也要钉住：平台侧的分支与 headless 用例的 `task_kind` 断言都比对字符串。
    #[test]
    fn task_kind_literals_are_the_ones_the_task_registry_sees() {
        assert_eq!(NETWORK_TASK_KIND, "send_network");
        assert_eq!(SERIAL_TASK_KIND, "send_serial");
        let unknown = PlannedSend {
            task_kind: "send_something_else",
            bytes: Vec::new(),
        };
        assert!(
            !unknown.targets_network_port(),
            "未知种类不得被当成网络分支（会静默改投递后端）"
        );
    }

    /// `SendPlanError` 是两平台错误文案的同一来源：native 把它包进 `AppError::Transport`、
    /// wasm 直接 `to_string()`，所以这句话就是用户在两侧看到的同一行字。
    #[test]
    fn hex_error_display_is_shared_by_both_platforms() {
        let error = plan_send(&hex("AB C", true), false).expect_err("严格模式必须拒绝单 nibble");
        assert_eq!(
            error.to_string(),
            "HEX 解析失败：严格模式: \"C\" 规范化后为 1 个字符，必须恰为 2（偶数 hex 长度），请补0或关闭严格模式"
        );
        assert_eq!(SendPlanError::NotASend.to_string(), "该命令不是发送类命令");
    }
}
