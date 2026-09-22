//! 豁免清单守卫：`#[allow(...)]` 只准减、不准悄悄加。
//!
//! 为什么需要它：本仓的门槛是 `clippy --workspace --all-targets -- -D warnings`，
//! 一条 `#[allow]` 就能把一个真实告警变成静默。此前"还剩几条豁免"只能靠人记，
//! 于是出现过两种失败：豁免早已空转却还挂着（第 6 轮一次摘光才发现 3 条是死的），
//! 以及新改动顺手加一条而没人注意。这里把清单钉成字面量：
//! 多一条就红（要求改动者显式承认），少一条也红（提醒把清单收紧）。
//!
//! 这条断言不判断"该不该豁免"—— 那是评审的事；它保证的是**加豁免必须留下痕迹**。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// 当前允许的豁免，每项写成「路径 空格 lint 名」，同一项重复几次就代表几条。
///
/// 用单字符串而不是元组，是为了让 16 行都留在 rustfmt 的列宽内（元组会被拆成
/// 四行一组，反而看不出"这是一张清单"）。
///
/// 这份清单是**扫出来的**，不是记出来的：`dead_code` 那 9 条多数写在
/// `#[cfg_attr(target_arch = …, allow(dead_code))]` 里（某个 target 上整块关掉时，
/// 剩下的引用会变死），对应 `docs/ARCHITECTURE.md` 里 Linux / wasm 死代码那张表。
/// `cfg_attr` 形式也算豁免 —— 它同样只是让告警静默，所以一起钉住。
const EXPECTED: &[&str] = &[
    // clippy 豁免（7 条）
    "crates/app/src/config.rs clippy::large_enum_variant",
    "crates/lua_host/src/lib.rs clippy::too_many_arguments",
    "crates/panels/src/gauge.rs clippy::too_many_arguments",
    "crates/panels/src/lib.rs clippy::too_many_arguments",
    "crates/panels/src/serial.rs clippy::too_many_arguments",
    "crates/transport/src/network.rs clippy::too_many_arguments",
    "crates/transport/src/network.rs clippy::too_many_arguments",
    // dead_code 豁免（9 条，多数随 cfg 生效）
    "crates/app/src/panel_registry.rs dead_code",
    "crates/app/src/shared_keymap.rs dead_code",
    "crates/app/src/shared_keymap.rs dead_code",
    "crates/app/src/state.rs dead_code",
    "crates/panels/src/gauge.rs dead_code",
    "crates/panels/src/gauge.rs dead_code",
    "crates/panels/src/gauge.rs dead_code",
    "crates/panels/src/gauge.rs dead_code",
    "crates/panels/src/gauge.rs dead_code",
];

/// 把清单折成「（路径, lint）→ 条数」。
fn expected_counts() -> BTreeMap<(String, String), usize> {
    let mut out: BTreeMap<(String, String), usize> = BTreeMap::new();
    for entry in EXPECTED {
        let (path, lint) = entry
            .split_once(' ')
            .unwrap_or_else(|| panic!("清单项 `{entry}` 不是「路径 空格 lint」的形状"));
        *out.entry((path.to_owned(), lint.to_owned())).or_insert(0) += 1;
    }
    assert!(
        out.values().sum::<usize>() == EXPECTED.len(),
        "清单折出来的条数与行数不一致（解析写错了）"
    );
    out
}

fn workspace_root() -> PathBuf {
    // 本测试在 crates/app 下编译，工作区根就是它的上一级。
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(PathBuf::from)
        .expect("crates/app 的上一级应当是工作区根")
}

/// 递归收集 `crates/**/*.rs`（`target/` 与 `vendor/` 不在 `crates/` 下，无需排除）。
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|error| panic!("读取 {dir:?} 失败：{error}"))
    {
        let path = entry
            .unwrap_or_else(|error| panic!("读取 {dir:?} 目录项失败：{error}"))
            .path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// 从一行里取出 `allow(` 与匹配右括号之间的 lint 列表，按顶层逗号切分。
fn allows_in_line(line: &str) -> Vec<String> {
    // 用 concat 拼出被搜索的字面量：写死成字符串会让扫描器把本行也算成一条豁免。
    const NEEDLE: &str = concat!("allow", "(");
    let mut found = Vec::new();
    let bytes: Vec<char> = line.chars().collect();
    let needle: Vec<char> = NEEDLE.chars().collect();
    let mut index = 0;
    while index + needle.len() < bytes.len() {
        if bytes[index..index + needle.len()] == needle[..] {
            let mut depth = 1;
            let mut end = index + needle.len();
            while end < bytes.len() && depth > 0 {
                match bytes[end] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                end += 1;
            }
            if depth == 0 {
                let inner: String = bytes[index + needle.len()..end - 1].iter().collect();
                let mut current = String::new();
                let mut paren = 0;
                for ch in inner.chars() {
                    match ch {
                        '(' => paren += 1,
                        ')' => paren -= 1,
                        ',' if paren == 0 => {
                            found.push(current.trim().to_owned());
                            current = String::new();
                            continue;
                        }
                        _ => {}
                    }
                    current.push(ch);
                }
                found.push(current.trim().to_owned());
            }
            index = end;
        } else {
            index += 1;
        }
    }
    found
        .into_iter()
        .filter(|lint| lint.starts_with("clippy::") || lint.starts_with("dead_code"))
        .collect()
}

/// （文件, lint）→ 出现次数。同一文件同一 lint 可以有多条（`transport/network.rs`
/// 就有两条 `too_many_arguments`），所以必须数次数，去重会让"在已登记文件里再加一条"溜过去。
fn collected() -> BTreeMap<(String, String), usize> {
    let root = workspace_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(
        files.len() > 50,
        "只扫到 {} 个 .rs 文件，扫描路径大概错了 —— 这条守卫会退化成零断言",
        files.len()
    );
    let mut out: BTreeMap<(String, String), usize> = BTreeMap::new();
    for path in files {
        let text =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("读取 {path:?} 失败：{error}"));
        let relative = path
            .strip_prefix(&root)
            .unwrap_or_else(|_| panic!("{path:?} 不在工作区根下"))
            .to_string_lossy()
            .replace('\\', "/");
        for line in text.lines() {
            // 注释里出现的豁免字面量不算（文档会引用它）。
            if line.trim_start().starts_with("//") {
                continue;
            }
            for lint in allows_in_line(line) {
                *out.entry((relative.clone(), lint)).or_insert(0) += 1;
            }
        }
    }
    out
}

#[test]
fn lint_exemptions_match_the_pinned_list() {
    let actual = collected();
    let expected = expected_counts();
    // 同一 (文件, lint) 的多条豁免要在清单里写多次，否则次数对不上是清单的问题。
    let expected_total: usize = expected.values().sum();

    // 防空转：清单本身为空、或扫描什么都没抓到，都等于没有守卫。
    assert!(!expected.is_empty(), "钉住的清单为空，这条守卫没有意义");
    assert!(
        !actual.is_empty(),
        "扫描没抓到任何豁免，说明扫描或过滤写错了（当前应当有 {expected_total} 条）"
    );

    let mut added = Vec::new();
    let mut removed = Vec::new();
    for key in expected
        .keys()
        .chain(actual.keys())
        .collect::<BTreeSet<_>>()
    {
        let want = expected.get(key).copied().unwrap_or(0);
        let have = actual.get(key).copied().unwrap_or(0);
        if have > want {
            added.push(format!("{key:?} 多了 {} 条", have - want));
        } else if want > have {
            removed.push(format!("{key:?} 少了 {} 条", want - have));
        }
    }
    assert_eq!(
        expected_total,
        actual.values().sum::<usize>(),
        "豁免总数变了：清单 {expected_total} 条，实际 {} 条。\n新增：{added:?}\n已消失：{removed:?}",
        actual.values().sum::<usize>()
    );
    assert!(
        added.is_empty() && removed.is_empty(),
        "豁免清单与代码不一致。\n新增（需要显式承认：把条目加进 EXPECTED 并在 ARCHITECTURE 里说明理由）：{added:?}\n\
         已消失（很好，把 EXPECTED 收紧到实际条数）：{removed:?}"
    );
}
