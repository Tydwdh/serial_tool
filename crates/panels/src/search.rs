//! 统一搜索词编译与匹配。
//!
//! 终端、日志、发送历史、插件市场、命令面板等所有用户搜索入口共用同一套
//! 语义：**普通词做字面子串匹配（保持各入口原有的大小写规则），`re:` 前缀
//! 的输入按正则匹配**（Rust regex 语法）。与 `ctx.serial.write_line_and_expect`
//! 的 `re:` 响应模式约定保持一致。
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
                }
            }
        }
        let needle = if case_sensitive {
            trimmed.to_owned()
        } else {
            trimmed.to_lowercase()
        };
        Self::Literal {
            needle,
            case_sensitive,
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
}
