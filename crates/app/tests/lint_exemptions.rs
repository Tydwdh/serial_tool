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
/// 用单字符串而不是元组，是为了让清单留在 rustfmt 的列宽内（元组会被拆成
/// 四行一组，反而看不出"这是一张清单"）。
///
/// 这份清单是**扫出来的**，不是记出来的。`dead_code` 那 3 条分两种形状：
/// `crates/app/src/shared_keymap.rs` 的 2 条写在
/// `#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]` 里 —— 那些项只有 wasm 侧的
/// `web` 模块会读（`mod web` 是 `cfg(target_arch = "wasm32")`），所以 native 构建里它们必然报死；
/// 剩下 1 条是 `crates/app/src/state.rs` 里普通的 `#[allow(dead_code)]`，护着一个刻意暂停、
/// 尚无调用者的方法（该模块本身是 `cfg(not(target_arch = "wasm32"))`）。前者对应
/// `docs/ARCHITECTURE.md` 里 Linux / wasm 死代码那张表。
/// `cfg_attr` 形式也算豁免 —— 它同样只是让告警静默，所以一起钉住。
///
/// **判"某条豁免是不是空转"的正确做法**（第 9 轮与第 17 轮都用它，第 17 轮据此删掉 6 条；
/// 第 28 轮再跑一次，当时那 10 条全部复发、一条没删；第 29 轮把
/// `transport/network.rs` 的 2 条 `too_many_arguments` 换成单参数的 `NetworkWorkerContext`，
/// 清单收到 8 条；第 30 轮把 `panels/lib.rs::next_auto_scroll_state` 的 8 个位置参数换成
/// 单参数的 `AutoScrollInputs`，清单收到 7 条；第 32 轮把 `panels/gauge.rs::draw_arc`
/// 的 5 个相邻同型 `f32` 收进命名字段的 `ArcSpec`，清单收到 6 条；第 33 轮把
/// `panels/serial.rs::render_port_editor` 的 7 个位置参数（`alias` 与 `selected_group`
/// 是相邻同型的 `&mut String`）收进命名字段的 `PortEditorInputs`，清单收到 5 条
/// —— 摘下来的那些不需要重测，代码里已经没有它们的落点）：
/// 把 `crates/` 下的豁免一次摘光，然后**分别**跑两条 clippy ——
/// `--workspace --all-targets`（native）与 `--target wasm32-unknown-unknown -p hardware-workbench-app`
/// （wasm）—— 谁点名谁就还需要，两边都不点名的才是死的。只看并集不够：
/// `cfg_attr(target_arch = "wasm32", …)` 护着的东西在 native 构建里本来就活着，
/// native 永远不会抱怨它，只在 wasm 那半边才测得出来。
///
/// 还有个坑：`-D warnings` 下某个 crate 一失败，依赖它的 crate 就**根本不会被编译**，
/// 于是这一条只报了最前面那几个站点就收工（第 28 轮 native 只跑到 `tool-transport` 停下，
/// `crates/app` 那 4 条完全没被检查到）。要把每条豁免都点到位，得把这两条命令
/// **去掉 `-D warnings` 再各跑一遍**收集全部告警 —— 否则"没复发"可能只是"没检查"。
const EXPECTED: &[&str] = &[
    // clippy 豁免（2 条，重新数过；第 29 轮把 `transport/network.rs` 的 2 条
    // `too_many_arguments` 换成单参数的 `NetworkWorkerContext`、第 30 轮把 `panels/lib.rs`
    // 的 1 条换成单参数的 `AutoScrollInputs`、第 32 轮把 `panels/gauge.rs` 的 1 条换成
    // 命名字段的 `ArcSpec`、第 33 轮把 `panels/serial.rs` 的 1 条换成命名字段的
    // `PortEditorInputs` 之后各删掉）
    "crates/app/src/config.rs clippy::large_enum_variant",
    "crates/lua_host/src/lib.rs clippy::too_many_arguments",
    // dead_code 豁免（3 条；第 17 轮摘光重测后，6 条 wasm 侧的判为空转已删）
    "crates/app/src/shared_keymap.rs dead_code",
    "crates/app/src/shared_keymap.rs dead_code",
    "crates/app/src/state.rs dead_code",
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

/// （文件, lint）→ 出现次数。同一文件同一 lint 可以有多条（`app/src/shared_keymap.rs`
/// 就有两条 `dead_code`），所以必须数次数，去重会让"在已登记文件里再加一条"溜过去。
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

/// manifest 文本里 `[lints…]` 段中把某个 lint 设成 `allow` 的行。
///
/// 只做行级文本扫描，不引 TOML 解析器：为这一条守卫加依赖，代价大于收益，
/// 而且本仓的 `manifest_deps.rs` 已经在管依赖边了。
fn manifest_allowed_lints(text: &str) -> Vec<String> {
    let mut section = String::new();
    let mut found = Vec::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = header.to_owned();
            continue;
        }
        // `lints`、`lints.clippy`、`workspace.lints.rust`、`workspace.lints.clippy` 都要算：
        // 按 `.` 拆开找一段叫 lints 的，而不是前缀匹配 —— 前缀匹配会**整段漏掉标准写法
        // `[workspace.lints.*]`**，那正好是最容易偷偷加静默的地方。
        if !section.split('.').any(|part| part == "lints") {
            continue;
        }
        // `lint = "allow"`（含 `level = "allow"`）以及 `{ level = "allow" }` 三种写法。
        let Some((_, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let quoted_allow = value.trim_matches('"') == "allow";
        let table_allow = value.contains("level") && value.contains("allow");
        if quoted_allow || table_allow {
            found.push(format!("{section} → {line}"));
        }
    }
    found
}

/// cargo 配置里的 rustflags 中会**静默 lint** 的旗标。
fn silencing_rustflags(text: &str) -> Vec<String> {
    const FORBIDDEN: &[&str] = &["-A", "--allow", "--cap-lints"];
    let mut found = Vec::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("");
        if !line.contains("rustflags") {
            continue;
        }
        for flag in FORBIDDEN {
            // 只在"作为独立 token 出现"时算数，避免把 `--cfg=…allow…` 这类误判进来。
            let hits = line
                .split(['"', ',', ' '])
                .map(str::trim)
                .any(|token| token == *flag || token.starts_with(&format!("{flag}=")));
            if hits {
                found.push(format!("{flag} ← {}", line.trim()));
            }
        }
    }
    found
}

#[test]
fn no_lint_allowance_in_manifests() {
    // 为什么要这条：棘轮原本只数源码里的 `#[allow(...)]`。往任意 `Cargo.toml` 加一行
    // `[lints.clippy] … = "allow"` 就能让全仓某个 lint 静默，而**四道门与那条守卫都会照常绿** ——
    // "豁免只准减、不准悄悄加"的承诺当场失效。这条把它也钉住。
    //
    // 本测试自带**正向对照**（下面喂一段人造的坏文本）：没有它，谓词写坏了（比如 section
    // 状态机没跟上）会让"扫到 0 条"看起来像"仓库很干净"，实际是探测器坏了。
    let planted = "[package]\nname = \"x\"\n\n[lints.clippy]\nunwrap_used = \"allow\"\n";
    assert_eq!(
        manifest_allowed_lints(planted).len(),
        1,
        "正向对照失败：人造的 `[lints.clippy] … = \"allow\"` 没被认出来 —— 谓词是空转的"
    );
    assert!(
        manifest_allowed_lints("[lints.clippy]\nunwrap_used = \"deny\"\n").is_empty(),
        "把 lint 设成 deny/警告不是静默，不该被报出来"
    );
    // 最容易漏的两种写法单独钉：cargo 的**标准**形式是 `[workspace.lints.*]`（父包声明、
    // 成员 `lints.workspace = true` 继承），以及表值写法 `{ level = "allow", priority = 1 }`。
    assert_eq!(
        manifest_allowed_lints(
            "[workspace.lints.clippy]\nunwrap_used = \"allow\"\n[workspace.lints.rust]\ndead_code = { level = \"allow\", priority = -1 }\n"
        ).len(),
        2,
        "标准写法 `[workspace.lints.*]` 或表值 `level = \"allow\"` 没被认全 —— 静默通道会漏"
    );

    let root = workspace_root();
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&root).expect("读工作区根失败") {
        let path = entry.expect("读目录项失败").path();
        if path.file_name().and_then(|n| n.to_str()) == Some("Cargo.toml") {
            manifests.push(path);
        }
    }
    for dir in ["crates", "xtask"] {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        for entry in fs::read_dir(&base).expect("读 crates/ 失败") {
            let path = entry.expect("读目录项失败").path().join("Cargo.toml");
            if path.is_file() {
                manifests.push(path);
            }
        }
    }
    assert!(
        manifests.len() >= 10,
        "只扫到 {} 份 manifest，扫描路径大概错了 —— 这条守卫会退化成零断言",
        manifests.len()
    );
    let mut offenders = Vec::new();
    for path in &manifests {
        let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("读取 {path:?} 失败：{e}"));
        for hit in manifest_allowed_lints(&text) {
            let relative = path.strip_prefix(&root).unwrap_or(path.as_path());
            offenders.push(format!("{relative:?}: {hit}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "manifest 里有把 lint 设成 allow 的段 —— 这等于绕过豁免棘轮加静默。\n{}",
        offenders.join("\n")
    );
}

#[test]
fn rustflags_do_not_silence_lints() {
    // 同上，另一条静默通道：`.cargo/config.toml` 的 rustflags。`-A` / `--allow` /
    // `--cap-lints allow` 都能在不改一行源码的前提下让告警消失。
    let planted = "[build]\nrustflags = [\"-A\", \"dead_code\"]\n";
    assert_eq!(
        silencing_rustflags(planted).len(),
        1,
        "正向对照失败：人造的 `-A` 没被认出来 —— 谓词是空转的"
    );
    assert!(
        silencing_rustflags(
            "[target.wasm32-unknown-unknown]\nrustflags = [\"--cfg=web_sys_unstable_apis\"]\n"
        )
        .is_empty(),
        "合法的 `--cfg` 不该被报成静默"
    );

    let root = workspace_root();
    let mut checked = 0usize;
    for name in [".cargo/config.toml", ".cargo/config", ".cargo/config.yml"] {
        let path = root.join(name);
        if !path.is_file() {
            continue;
        }
        checked += 1;
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {path:?} 失败：{e}"));
        let offenders = silencing_rustflags(&text);
        assert!(
            offenders.is_empty(),
            "{name} 的 rustflags 里有静默 lint 的旗标：\n{}",
            offenders.join("\n")
        );
    }
    // 一个配置文件都没有也是安全的，那就没什么可核 —— 但那种情况下这条测试确实没在守任何东西，
    // 所以把扫到的文件数打印出来，便于人工确认它不是永远空转。
    println!("[lint 静默通道] 已核对 cargo 配置文件 {checked} 份");
}
