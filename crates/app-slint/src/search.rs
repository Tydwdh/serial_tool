// 统一搜索：字面量 + re: 正则，与 crates/panels/search.rs 行为一致
use regex::{Regex, RegexBuilder};

pub enum SearchQuery {
    Literal { needle: String, case_sensitive: bool },
    Regex(Regex),
}
impl SearchQuery {
    pub fn new(query: &str, case_sensitive: bool) -> Self {
        let trimmed = query.trim();
        if let Some(pattern) = trimmed.strip_prefix("re:") {
            match RegexBuilder::new(pattern)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(re) => return Self::Regex(re),
                Err(e) => {
                    log::warn!("search: invalid regex {pattern:?}: {e}");
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
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Literal { needle, .. } => needle.is_empty(),
            Self::Regex(_) => false,
        }
    }
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
