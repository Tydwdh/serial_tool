//! Cargo.toml 依赖判定：返回**解析后的真实 crate 名**，不是文本包含。
//!
//! 为什么要解析而不是扫行：`{ package = "tool-transport" }` 重命名与
//! `"tool-transport" = {...}` 加引号的键都是合法清单，按声明键文本匹配会漏检，
//! 使 presentation 边界守卫静默保持绿色。这两个向量是评审员在仓库外副本里实证过的。
//!
//! 本文件被三个 test target 编入：它自身（cargo 自动发现）、`crates/app/tests/architecture.rs`
//! 与 `crates/application/tests/architecture.rs`（后者用 `#[path]` 引同一份实现）。
//! 刻意不复制第二份实现 —— 两份守卫必须共用同一个判定，否则「修了一个漏洞、
//! 另一个仍然可绕过」会重新出现。

use std::collections::BTreeSet;

/// 收集清单里所有依赖表中的真实 crate 名。
/// 覆盖 `[dependencies] / [dev-dependencies] / [build-dependencies]`
/// 及任意 `[target.<cfg>.*dependencies]`；重命名以 `package` 字段为准。
///
/// `[workspace.dependencies]` 不计入：那是版本声明表，不产生依赖边。
pub fn dependency_names(manifest: &str) -> BTreeSet<String> {
    let root = parse_document(manifest);
    let mut names = BTreeSet::new();
    collect_dependency_tables(&root, &mut names);
    names
}

/// `[package] name`：判定按「包」而不是按「目录名」跳过 presentation，
/// 因为目录名在 Windows 上参与路径比较会因为分隔符而静默失效。
pub fn package_name(manifest: &str) -> Option<String> {
    parse_document(manifest)
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(|name| name.as_str())
        .map(str::to_owned)
}

/// `[workspace] members`：用于断言「没有成员逃出清单扫描」。
/// 虚拟清单（只有 `[workspace]`）返回成员数组，包清单返回空。
pub fn workspace_members(manifest: &str) -> Vec<String> {
    parse_document(manifest)
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(|members| members.as_array())
        .map(|members| {
            members
                .iter()
                .map(|member| {
                    member
                        .as_str()
                        .unwrap_or_else(|| panic!("workspace.members 含非字符串项：{member}"))
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 清单文本 → `(包名, 真实依赖名集合)`，并当场拒绝两种「静默失效」：
/// * 没有 `[package] name` —— 判定对象根本不是想象中的那个包；
/// * 解析出零依赖 —— 判定逻辑本身失效，后面的断言会退化成零断言。
///
/// 两份守卫都走这里，避免「一处修了防空转、另一处没修」。
pub fn manifest_dependencies(manifest: &str, label: &str) -> (String, BTreeSet<String>) {
    let name = package_name(manifest)
        .unwrap_or_else(|| panic!("{label} 没有 [package] name，不是有效的包清单"));
    let names = dependency_names(manifest);
    assert!(
        !names.is_empty(),
        "{label}（{name}）解析出零依赖，判定可能已失效"
    );
    (name, names)
}

/// `banned` 与实际依赖名的交集：精确集合比较，永远不是子串比较。
pub fn violations(names: &BTreeSet<String>, banned: &[&'static str]) -> Vec<&'static str> {
    banned
        .iter()
        .copied()
        .filter(|banned| names.contains(*banned))
        .collect()
}

/// 清单解析：`toml::Value` 的 `FromStr` 解析的是「值」文法，整份清单会报
/// `unexpected content, expected nothing`；文档根必须按 `toml::Table` 解析。
fn parse_document(manifest: &str) -> toml::Table {
    manifest
        .parse::<toml::Table>()
        .unwrap_or_else(|error| panic!("Cargo.toml 解析失败：{error}"))
}

/// 依赖表名（同一份清单里可能出现在根层与 `target.<cfg>` 层）。
const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// 一层「表名即语义」的结构：取依赖表，或下钻 `target.<cfg>` 的一层。
fn collect_dependency_tables(table: &toml::Table, out: &mut BTreeSet<String>) {
    for (key, value) in table {
        if DEPENDENCY_TABLES.contains(&key.as_str()) {
            if let Some(entries) = value.as_table() {
                collect_dependencies(entries, out);
            }
        } else if key == "target" {
            // `target.<cfg>.<依赖表>`：中间那层是 cfg 表达式，必须再下一层。
            if let Some(targets) = value.as_table() {
                for cfg_table in targets.values() {
                    if let Some(cfg_table) = cfg_table.as_table() {
                        collect_dependency_tables(cfg_table, out);
                    }
                }
            }
        }
    }
}

/// 依赖表内部：声明名可能被 `package` 覆盖，真实 crate 名以 `package` 为准。
fn collect_dependencies(entries: &toml::Table, out: &mut BTreeSet<String>) {
    for (declared, spec) in entries {
        let real = match spec {
            toml::Value::Table(spec) => spec
                .get("package")
                .and_then(|package| package.as_str())
                .unwrap_or(declared.as_str())
                .to_owned(),
            _ => declared.to_owned(),
        };
        out.insert(real);
    }
}

/// presentation 禁止直连的领域 crate（唯一真相，两份守卫共用）。
pub const DOMAIN_CRATES: [&str; 6] = [
    "tool-transport",
    "tool-recorder",
    "tool-extension",
    "tool-marketplace",
    "tool-lua-host",
    "tool-updater",
];

/// 任何非 presentation crate 都不得出现的 UI/窗口依赖。
pub const UI_BANS: [&str; 9] = [
    "egui",
    "eframe",
    "egui_tiles",
    "egui_extras",
    "egui_kittest",
    "egui_material_icons",
    "rfd",
    "winit",
    "softbuffer",
];

/// presentation 层（composition root + panels）：允许依赖 UI；
/// 同时是「非 presentation crate 不得反向依赖 UI 层」的禁用语。
pub const PRESENTATION_CRATES: [&str; 2] = ["tool-panels", "hardware-workbench-app"];

/// 两个历史绕过向量都必须被抓到。
#[test]
fn dependency_names_resolve_renamed_and_quoted_deps() {
    let manifest = r#"
[package]
name = "demo"

[dependencies]
tool-application = { path = "../application" }
"tool-transport" = { path = "../transport" }
my-recorder = { package = "tool-recorder", path = "../recorder" }
serde.workspace = true

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
open = "5"
renamed-market = { package = "tool-marketplace", path = "../marketplace" }

[dev-dependencies]
egui_kittest = "0.35"
"#;
    let names = dependency_names(manifest);
    // 两个历史绕过向量都必须被抓到：
    assert!(
        names.contains("tool-transport"),
        "加引号的键漏检：{names:?}"
    );
    assert!(
        names.contains("tool-recorder"),
        "package 重命名漏检：{names:?}"
    );
    assert!(
        names.contains("tool-marketplace"),
        "target 段重命名漏检：{names:?}"
    );
    assert!(names.contains("tool-application"));
    assert!(names.contains("serde"));
    assert!(names.contains("open"));
    assert!(names.contains("egui_kittest"));
    // 反面：不得把 `[package] name` 或声明名误当成依赖。
    assert!(!names.contains("demo"));
    assert!(!names.contains("my-recorder"), "重命名后应只报告真实包名");
    assert!(!names.contains("renamed-market"));
}

/// 前缀近亲不得被误判：`tool-application-extra` 不是 `tool-application`。
#[test]
fn dependency_names_do_not_report_prefix_neighbours() {
    let manifest = r#"
[package]
name = "demo"

[dependencies]
tool-application-extra = { path = "../application_extra" }
"tool-transport-wrapper" = { package = "tool-transport-wrapper", path = "../t" }
"#;
    let names = dependency_names(manifest);
    assert!(names.contains("tool-application-extra"));
    assert!(names.contains("tool-transport-wrapper"));
    assert!(
        !names.contains("tool-application"),
        "把近亲名当成真实依赖会让 app 侧守卫给出假证据：{names:?}"
    );
    assert!(!names.contains("tool-transport"), "近亲名误判：{names:?}");
}

/// 只有真正的依赖表算依赖：`[workspace.dependencies]` 是版本声明、`[features]`/
/// 注释/描述里的 crate 名都不是依赖边。
#[test]
fn dependency_names_ignore_non_dependency_tables() {
    let manifest = r#"
[workspace]
members = ["crates/a"]

[workspace.dependencies]
tool-transport = { path = "crates/transport" }
egui = "0.35"

[package]
name = "b"
description = "提到 tool-transport 与 egui 的长描述"

[features]
tool-transport = []
"#;
    let names = dependency_names(manifest);
    assert!(names.is_empty(), "非依赖表被当成了依赖：{names:?}");
}

/// 防空转（其一）：判定对象没有 `[package] name` 时必须炸，而不是当作「没有依赖」。
#[test]
#[should_panic(expected = "没有 [package] name")]
fn manifest_without_package_name_cannot_pass_silently() {
    manifest_dependencies(
        "[workspace]\nmembers = [\"crates/a\"]\n",
        "fixture: 虚拟清单",
    );
}

/// 防空转（其二）：解析出零依赖的清单必须炸 —— 否则调用方的禁令循环等于零断言。
#[test]
#[should_panic(expected = "解析出零依赖")]
fn zero_dependency_manifest_cannot_pass_silently() {
    manifest_dependencies(
        "[package]\nname = \"empty\"\n\n[features]\ndefault = []\n",
        "fixture: 空依赖包",
    );
}

/// 判定「某清单是否属于 presentation」看 `[package] name`，不看目录名。
#[test]
fn package_name_reads_the_declared_package() {
    assert_eq!(
        package_name("[package]\nname = \"tool-panels\"\n"),
        Some("tool-panels".to_owned())
    );
    assert_eq!(
        package_name("[workspace]\nmembers = []\n"),
        None,
        "虚拟清单没有 [package] name，必须返回 None 而不是 panic"
    );
}

/// `workspace_members` 读的是**显式**成员表：漏一项就意味着该成员只靠 path-dep
/// 隐式纳入，任何一次其它清单的编辑都可能把它从所有门里静默删掉
/// （`crates/recorder` 在补上前正是如此）。
#[test]
fn workspace_members_reads_the_explicit_list() {
    let manifest = "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n";
    assert_eq!(workspace_members(manifest), vec!["crates/a", "crates/b"]);
    assert!(
        workspace_members("[package]\nname = \"a\"\n").is_empty(),
        "包清单不该有成员表"
    );
}

/// 禁令表本身必须可失败地精确：`egui` 不得吃掉 `egui_kittest`（子串匹配的
/// 「恒真/恒假假证据」正是本仓两次踩过的坑），同时不得漏掉任何一条真实禁令。
#[test]
fn ban_lists_match_exact_names_only() {
    // 近亲名一个都不许命中（含 `LuaOptions::default()` 这类文档里的相似写法）。
    for near_miss in [
        "egui_kittest_derive",
        "egui_material_icons_derive",
        "tool-transport-extra",
        "tool-recorder-bin",
        "tool-application",
        "LuaOptions",
    ] {
        let mut names = BTreeSet::new();
        names.insert(near_miss.to_owned());
        assert!(
            violations(&names, &UI_BANS).is_empty(),
            "{near_miss} 不该被 UI_BANS 子串误捕"
        );
        assert!(
            violations(&names, &DOMAIN_CRATES).is_empty(),
            "{near_miss} 不该被 DOMAIN_CRATES 误捕"
        );
    }
    // 反证：真名必须一条条被抓到，禁令表不能是永远通过的空壳。
    for banned in UI_BANS {
        let names = BTreeSet::from([banned.to_owned()]);
        assert_eq!(
            violations(&names, &UI_BANS),
            vec![banned],
            "{banned} 必须被 UI_BANS 抓到"
        );
    }
    for banned in DOMAIN_CRATES {
        let names = BTreeSet::from([banned.to_owned()]);
        assert_eq!(
            violations(&names, &DOMAIN_CRATES),
            vec![banned],
            "{banned} 必须被 DOMAIN_CRATES 抓到"
        );
    }
    let lists: [(&str, &[&str]); 3] = [
        ("UI_BANS", &UI_BANS),
        ("DOMAIN_CRATES", &DOMAIN_CRATES),
        ("PRESENTATION_CRATES", &PRESENTATION_CRATES),
    ];
    for (label, items) in lists {
        assert!(!items.is_empty(), "{label} 为空等于零断言");
        let deduped: BTreeSet<&str> = items.iter().copied().collect();
        assert_eq!(deduped.len(), items.len(), "{label} 有重复项");
    }
    assert!(
        UI_BANS.contains(&"rfd"),
        "rfd 是原生窗口对话框，属于 UI 禁令"
    );
}
