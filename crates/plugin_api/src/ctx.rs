//! Shared `ctx.*` argument and matching helpers.
//!
//! Both Lua engines used to carry private copies of these rules. The web copy
//! recompiled a regex on every match and lacked the optional `(time)` prefix
//! handling used by some Marlin firmware; keeping one implementation removes
//! that drift.

use std::sync::{Arc, LazyLock, Mutex};

use crate::PluginCapability;

/// The permission-gated `ctx.*` domains. Both engines bind the same names to
/// the same capability, so a manifest permission cannot mean one thing on
/// desktop and another in the browser.
#[derive(Debug, Clone, Copy)]
pub struct CtxDomain {
    pub name: &'static str,
    pub capability: PluginCapability,
}

pub const CTX_DOMAINS: &[CtxDomain] = &[
    CtxDomain {
        name: "log",
        capability: PluginCapability::Log,
    },
    CtxDomain {
        name: "bus",
        capability: PluginCapability::Bus,
    },
    CtxDomain {
        name: "serial",
        capability: PluginCapability::Serial,
    },
    CtxDomain {
        name: "ui",
        capability: PluginCapability::Ui,
    },
    CtxDomain {
        name: "timer",
        capability: PluginCapability::Timer,
    },
    CtxDomain {
        name: "storage",
        capability: PluginCapability::Storage,
    },
    CtxDomain {
        name: "dialog",
        capability: PluginCapability::Dialog,
    },
    CtxDomain {
        name: "fs",
        capability: PluginCapability::Filesystem,
    },
    CtxDomain {
        name: "task",
        capability: PluginCapability::Task,
    },
    CtxDomain {
        name: "config",
        capability: PluginCapability::Config,
    },
    CtxDomain {
        name: "testing",
        capability: PluginCapability::Testing,
    },
];

/// Canonical HEX parser. Kept as a named helper so both engines pass the same
/// rules (`0x` prefixes, `_`/`-` separators, single-nibble padding) instead of
/// hand-rolling a narrower parser.
pub fn parse_hex(value: &str) -> Result<Vec<u8>, String> {
    tool_core::parse_hex(value)
}

/// Topic pattern matching shared with the bus.
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    tool_core::topic_matches(pattern, topic)
}

/// Serial response pattern matching.
///
/// - `re:<regex>`: Rust-regex match, after skipping an optional leading
///   `(...)` firmware prefix. Compiled patterns are cached.
/// - `^xxx`: prefix match after the same optional prefix skip.
/// - anything else: substring match.
pub fn match_pattern(line: &str, pattern: &str) -> bool {
    if let Some(regex) = pattern.strip_prefix("re:") {
        let after_prefix = strip_leading_paren_prefix(line);
        return compile_cached_regex(regex).is_some_and(|re| re.is_match(after_prefix));
    }
    if let Some(suffix) = pattern.strip_prefix('^') {
        let after_prefix = strip_leading_paren_prefix(line);
        after_prefix.starts_with(suffix)
    } else {
        line.contains(pattern)
    }
}

/// Skip an optional `(digits)` firmware prefix (`(0.00000)ok*43`).
fn strip_leading_paren_prefix(line: &str) -> &str {
    let Some(rest) = line.strip_prefix('(') else {
        return line;
    };
    let Some(close) = rest.find(')') else {
        return line;
    };
    if rest[..close].contains('\n') {
        return line;
    }
    &rest[close + 1..]
}

fn compile_cached_regex(pattern: &str) -> Option<Arc<regex::Regex>> {
    const REGEX_CACHE_MAX_ENTRIES: usize = 256;

    static CACHE: LazyLock<Mutex<std::collections::HashMap<String, Option<Arc<regex::Regex>>>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));
    let mut map = CACHE.lock().expect("regex cache poisoned");
    if map.len() >= REGEX_CACHE_MAX_ENTRIES {
        map.clear();
    }
    if let Some(entry) = map.get(pattern) {
        return entry.clone();
    }
    let compiled = match regex::Regex::new(pattern) {
        Ok(re) => Some(Arc::new(re)),
        Err(error) => {
            log::warn!("serial expect: invalid regex pattern {pattern:?}: {error}");
            None
        }
    };
    map.insert(pattern.to_owned(), compiled.clone());
    compiled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_skips_paren_prefix() {
        assert!(match_pattern("ok*43", "^ok"));
        assert!(match_pattern("(0.00000)ok*43", "^ok"));
        assert!(!match_pattern("(0.00000)rookie", "^ok"));
        assert!(!match_pattern("(unclosed ok", "^ok"));
    }

    #[test]
    fn regex_prefix_and_cache() {
        assert!(match_pattern("(1.234)ok", "re:^ok"));
        assert!(match_pattern("M105 ok 12.3", r"re:\d+\.\d+"));
        assert!(!match_pattern("anything", "re:["));
        assert!(match_pattern("DONE", "re:(?i)^done$"));
    }

    #[test]
    fn substring_stays_substring() {
        assert!(match_pattern("(0.00000)echo:busy", "busy"));
        assert!(!match_pattern("(0.00000)ok", "resend"));
    }

    #[test]
    fn hex_and_topics_come_from_core() {
        assert_eq!(parse_hex("0x41 42").unwrap(), vec![0x41, 0x42]);
        assert!(topic_matches("transport.*", "transport.serial.rx"));
        assert!(!topic_matches("transport.*", "log.info"));
    }
}
