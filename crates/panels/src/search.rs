//! 统一搜索词编译与匹配。
//!
//! 终端、日志、数据表、插件市场、命令面板这些用户搜索入口共用同一套
//! 语义：**普通词做字面子串匹配（保持各入口原有的大小写规则），`re:` 前缀
//! 的输入按正则匹配**（Rust regex 语法）。与插件侧 `ctx.serial.expect` /
//! `expect_from` / `request` / `write_line_and_expect` 的响应模式约定保持一致
//! （那三个形态见 `crates/lua_host/src/api/serial.rs` 的 `match_pat`）。
//!
//! **不在共用范围内的入口**：发送器的历史过滤框（`sender.rs` 的 `history_search`）
//! 至今仍按小写子串比较，不编译 `SearchQuery`，所以那里 `re:` 没有特殊含义。
//!
//! # 规则
//!
//! - `re:<regex>`：正则匹配。`case_sensitive=false` 时自动注入大小写不敏感
//!   （等价 `(?i)`）。非法正则回退为字面量搜索并记录 warning，保证搜索框
//!   行为可预测（不会出现"搜什么都空"的僵局）。
//! - 其它输入：字面子串匹配。`case_sensitive=false` 时对查询词与目标文本
//!   统一转小写（与既有行为一致）。

use regex::{Regex, RegexBuilder};

/// 编译后的搜索词。
pub enum SearchQuery {
    /// 字面子串匹配。
    Literal {
        needle: String,
        case_sensitive: bool,
        /// 输入带 `re:` 前缀但正则非法，已回退成字面量搜索。
        invalid_regex: bool,
    },
    /// 正则匹配。
    Regex(Regex),
}

impl SearchQuery {
    /// 编译搜索词。`case_sensitive` 控制大小写（正则模式下注入 `(?i)` 等价标志）。
    pub fn new(query: &str, case_sensitive: bool) -> Self {
        let trimmed = query.trim();
        if let Some(pattern) = trimmed.strip_prefix("re:") {
            match RegexBuilder::new(pattern)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(re) => return Self::Regex(re),
                Err(e) => {
                    log::warn!("search: invalid regex pattern {pattern:?}: {e}");
                    // 回退为字面量，保证行为可预测
                    return Self::literal(trimmed, case_sensitive, true);
                }
            }
        }
        Self::literal(trimmed, case_sensitive, false)
    }

    fn literal(needle: &str, case_sensitive: bool, invalid_regex: bool) -> Self {
        Self::Literal {
            needle: if case_sensitive {
                needle.to_owned()
            } else {
                needle.to_lowercase()
            },
            case_sensitive,
            invalid_regex,
        }
    }

    /// 是否发生了"看着像正则、其实按字面量搜"的回退。
    ///
    /// UI 用它把这件事显式告诉用户：静默回退会让用户以为自己搜的是正则。
    pub fn used_invalid_regex_fallback(&self) -> bool {
        matches!(
            self,
            Self::Literal {
                invalid_regex: true,
                ..
            }
        )
    }

    /// 搜索框的统一 hover 文案。
    ///
    /// 所有入口共用这一句，避免终端/日志/表格、插件市场、命令面板各写一遍而漂移。
    pub fn hover_hint(fell_back: bool) -> &'static str {
        if fell_back {
            "正则表达式非法，已按字面量搜索：去掉 `re:` 前缀或修正写法"
        } else {
            "支持正则：以 re: 开头（如 re:^ok\\d+）；否则按字面量搜索"
        }
    }

    /// 查询词是否为空（无过滤效果）。
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Literal { needle, .. } => needle.is_empty(),
            Self::Regex(_) => false,
        }
    }

    /// 判断目标文本是否命中。
    pub fn matches(&self, haystack: &str) -> bool {
        match self {
            Self::Literal {
                needle,
                case_sensitive,
                ..
            } => {
                if *case_sensitive {
                    haystack.contains(needle)
                } else {
                    haystack.to_lowercase().contains(needle)
                }
            }
            Self::Regex(re) => re.is_match(haystack),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_case_insensitive_default() {
        let q = SearchQuery::new("G1 X", false);
        assert!(q.matches("N1 G1 X10"));
        assert!(q.matches("g1 x10")); // 大小写不敏感
        assert!(!q.matches("M105"));
    }

    #[test]
    fn literal_case_sensitive() {
        let q = SearchQuery::new("G1", true);
        assert!(q.matches("N1 G1 X10"));
        assert!(!q.matches("g1 x10"));
    }

    #[test]
    fn literal_whitespace_trimmed() {
        let q = SearchQuery::new("  ok  ", false);
        assert!(q.matches("... ok ..."));
        let empty = SearchQuery::new("   ", false);
        assert!(empty.is_empty());
    }

    #[test]
    fn regex_prefix_matches() {
        let q = SearchQuery::new("re:^ok\\b", false);
        assert!(q.matches("ok"));
        assert!(q.matches("OK")); // case_insensitive 注入
        assert!(q.matches("ok 12"));
        assert!(!q.matches("rookie"));
        assert!(!q.matches("okay"));
    }

    #[test]
    fn regex_case_sensitive_respected() {
        let q = SearchQuery::new("re:^ok", true);
        assert!(q.matches("ok"));
        assert!(!q.matches("OK"));
    }

    #[test]
    fn regex_matches_multi_field() {
        let q = SearchQuery::new("re:(error|failed)", false);
        assert!(q.matches("thermal error"));
        assert!(q.matches("build failed"));
        assert!(!q.matches("finished ok"));
    }

    #[test]
    fn invalid_regex_falls_back_to_literal() {
        // 非法正则不 panic：回退为把整个输入当字面量
        let q = SearchQuery::new("re:[", false);
        assert!(q.matches("some re:[ text"));
        assert!(!q.matches("plain text"));
    }

    #[test]
    fn regex_empty_pattern_is_nonempty_query() {
        let q = SearchQuery::new("re:", false);
        assert!(!q.is_empty());
        assert!(q.matches("anything")); // 空正则匹配一切
    }

    #[test]
    fn invalid_regex_reports_the_fallback() {
        // UI 要靠这个标志告诉用户"你以为在搜正则，其实按字面量搜"，
        // 只记 log::warn 用户永远看不到。
        assert!(SearchQuery::new("re:[", false).used_invalid_regex_fallback());
    }

    #[test]
    fn valid_regex_and_literal_do_not_report_a_fallback() {
        assert!(!SearchQuery::new("re:^ok", false).used_invalid_regex_fallback());
        assert!(!SearchQuery::new("ok", false).used_invalid_regex_fallback());
        // `re:` 之外没有前缀含义的串，即使长得像坏正则也不该报警。
        assert!(!SearchQuery::new("[bad", false).used_invalid_regex_fallback());
    }
}
