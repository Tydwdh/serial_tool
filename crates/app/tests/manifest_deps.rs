//! Cargo.toml 依赖判定：返回**解析后的真实 crate 名**，不是文本包含。
//!
//! 为什么要解析而不是扫行：`{ package = "tool-transport" }` 重命名与
//! `"tool-transport" = {...}` 加引号的键都是合法清单，按声明键文本匹配会漏检，
//! 使 presentation 边界守卫静默保持绿色。这两个向量是评审员在仓库外副本里实证过的。
//!
//! 第三类提权同源但不同文件：重命名写在**根清单**的 `[workspace.dependencies]` 里，
//! 成员只写 `sneaky-transport.workspace = true`。cargo 解析时从根那一条取 `package`，
//! 成员侧完全看不到 `tool-transport` 字样，所以只看成员清单仍会少报
//! （1.92.0 在临时工作区实测：`cargo tree -p member` -> `inner`，
//! `cargo metadata` -> `{"name":"inner","rename":"sneaky-transport"}`）。
//! 故 `dependency_names` 必须额外吃一张「根别名 -> 真实包名」表。
//!
//! 本文件被三个 test target 编入：它自身（cargo 自动发现）、`crates/app/tests/architecture.rs`
//! 与 `crates/application/tests/architecture.rs`（后者用 `#[path]` 引同一份实现）。
//! 刻意不复制第二份实现 —— 两份守卫必须共用同一个判定，否则「修了一个漏洞、
//! 另一个仍然可绕过」会重新出现。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 收集清单里所有依赖表中的真实 crate 名。
/// 覆盖 `[dependencies] / [dev-dependencies] / [build-dependencies]`
/// 及任意 `[target.<cfg>.*dependencies]`；重命名以 `package` 字段为准。
///
/// `workspace_renames` 是**根清单** `[workspace.dependencies]` 的「别名 -> 真实包名」表
/// （`workspace_dependency_renames()`）：`alias.workspace = true` 的重命名只写在根那一份里，
/// 少了这张表就会少报。被判定清单自己的 `[workspace.dependencies]` 依旧不计入依赖边。
pub fn dependency_names(
    manifest: &str,
    workspace_renames: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let root = parse_document(manifest);
    let mut names = BTreeSet::new();
    collect_dependency_tables(&root, workspace_renames, &mut names);
    names
}

/// 根清单 `[workspace.dependencies]` 里**声明**的全部别名（不含真实名）。
/// 只用于防空转：这张表为空就说明根清单路径推导出错或表被删空，
/// 于是所有 `alias.workspace = true` 的重命名都会静默漏检。
pub fn workspace_dependency_names(manifest: &str) -> BTreeSet<String> {
    workspace_dependency_table(manifest)
        .map(|(declared, _)| declared)
        .unwrap_or_default()
}

/// 根清单 `[workspace.dependencies]` 里带 `package = ` 的条目：`别名 -> 真实包名`。
/// 不带 `package` 的条目不进这张表（那时声明名就是包名，cargo 也拒绝键名与包名不一致：
/// 1.92.0 实测根表写 `sneaky-transport = { path = "inner" }` 会 `no matching package` 报错）。
pub fn workspace_dependency_renames(manifest: &str) -> BTreeMap<String, String> {
    workspace_dependency_table(manifest)
        .map(|(_, renames)| renames)
        .unwrap_or_default()
}

/// `[workspace.dependencies]` 的一次解析：`(全部声明名, 带 package 的别名表)`。
fn workspace_dependency_table(
    manifest: &str,
) -> Option<(BTreeSet<String>, BTreeMap<String, String>)> {
    let entries = parse_document(manifest)
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|dependencies| dependencies.as_table())
        .cloned()?;
    let mut declared = BTreeSet::new();
    let mut renames = BTreeMap::new();
    for (alias, spec) in entries {
        declared.insert(alias.clone());
        if let Some(package) = spec
            .as_table()
            .and_then(|spec| spec.get("package"))
            .and_then(toml::Value::as_str)
        {
            renames.insert(alias.clone(), package.to_owned());
        }
    }
    Some((declared, renames))
}

/// 读**工作区根清单**并交出它的「别名 -> 真实包名」表。
///
/// 根路径从 `env!("CARGO_MANIFEST_DIR")` 推导：本文件被三个 target 各编一次，`env!` 每次
/// 展开成该 target 自己的包目录（`crates/app`、`crates/application`），两者都在根下两级，
/// 所以同一份推导对两份守卫都成立 —— 不依赖 cwd，也不各写一份。
pub fn workspace_dependency_renames_from_root() -> BTreeMap<String, String> {
    workspace_dependency_renames(&workspace_root_manifest())
}

/// 同上，但交出 `[workspace.dependencies]` 的**全部声明名**，供调用方设防空转下限：
/// 今天这张表 18 条且没有一条带 `package`，所以「零命中」既可能是根真干净、
/// 也可能是读错了文件。
pub fn workspace_dependency_names_from_root() -> BTreeSet<String> {
    workspace_dependency_names(&workspace_root_manifest())
}

/// 定位并读取工作区根清单；表为空即说明推导出错（成员清单没有这张表），当场 panic。
fn workspace_root_manifest() -> String {
    let path = workspace_root_manifest_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取工作区根清单 {} 失败：{error}", path.display()));
    assert!(
        !workspace_dependency_names(&text).is_empty(),
        "{} 的 [workspace.dependencies] 一个条目都没有：根清单路径推导出错，\
         `alias.workspace = true` 形式的重命名会全部漏检",
        path.display()
    );
    text
}

fn workspace_root_manifest_path() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .ancestors()
        .nth(2)
        .unwrap_or_else(|| {
            panic!(
                "{} 不在 <工作区根>/crates/<name> 下，无法定位根清单",
                crate_dir.display()
            )
        })
        .join("Cargo.toml")
}

/// 根 `[workspace.dependencies]` 里把别名重命名到禁令 crate 的条目：`[(真实名, 别名)]`。
/// 那张表本身不是依赖边（所有按清单扫描的守卫都刻意不看它），但它是唯一
/// 「改一行就能给 13 份清单同时投毒」的地方，所以要单独钉住。
pub fn workspace_renames_to_banned(
    renames: &BTreeMap<String, String>,
    banned: &[&'static str],
) -> Vec<(&'static str, String)> {
    renames
        .iter()
        .filter_map(|(alias, real)| {
            // 与 violations() 同口径：精确相等，不是子串（`tool-transport-extra` 不算）。
            banned
                .iter()
                .copied()
                .find(|banned| *banned == real.as_str())
                .map(|banned| (banned, alias.clone()))
        })
        .collect()
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
/// `workspace_renames` 必传（而不是在这里偷偷读盘）：让「有没有解析根别名」在类型上
/// 就是编译期看得见的义务，新加守卫时漏掉它是编译错误，不是又一次静默放行。
pub fn manifest_dependencies(
    manifest: &str,
    label: &str,
    workspace_renames: &BTreeMap<String, String>,
) -> (String, BTreeSet<String>) {
    let name = package_name(manifest)
        .unwrap_or_else(|| panic!("{label} 没有 [package] name，不是有效的包清单"));
    let names = dependency_names(manifest, workspace_renames);
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
fn collect_dependency_tables(
    table: &toml::Table,
    workspace_renames: &BTreeMap<String, String>,
    out: &mut BTreeSet<String>,
) {
    for (key, value) in table {
        if DEPENDENCY_TABLES.contains(&key.as_str()) {
            if let Some(entries) = value.as_table() {
                collect_dependencies(entries, workspace_renames, out);
            }
        } else if key == "target" {
            // `target.<cfg>.<依赖表>`：中间那层是 cfg 表达式，必须再下一层。
            if let Some(targets) = value.as_table() {
                for cfg_table in targets.values() {
                    if let Some(cfg_table) = cfg_table.as_table() {
                        collect_dependency_tables(cfg_table, workspace_renames, out);
                    }
                }
            }
        }
    }
}

/// 依赖表内部：声明名可能被覆盖成别的包，且**覆盖可以写在另一份文件里**
/// （`alias.workspace = true` + 根清单 `alias = { package = "real" }`）。
/// 两种写法都要落到真实包名上，否则守卫拿着别名判禁令，永远绿。
fn collect_dependencies(
    entries: &toml::Table,
    workspace_renames: &BTreeMap<String, String>,
    out: &mut BTreeSet<String>,
) {
    for (declared, spec) in entries {
        let Some(spec) = spec.as_table() else {
            out.insert(declared.clone()); // `serde = "1"`：声明名就是包名
            continue;
        };
        // 成员自己写的 package。
        let local_package = spec
            .get("package")
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        let inherits = spec.get("workspace") == Some(&toml::Value::Boolean(true));
        // 根清单里那条的重命名：只有继承项才适用（没写 `workspace = true` 就不发生继承，
        // 所以根表投毒而没人继承时不会凭空造出依赖边，那由根守卫单独管）。
        let workspace_package = if inherits {
            workspace_renames.get(declared.as_str()).cloned()
        } else {
            None
        };
        let mut resolved = false;
        for candidate in [local_package, workspace_package].into_iter().flatten() {
            out.insert(candidate);
            resolved = true;
        }
        if !resolved {
            // `{ version = "1" }` / `{ path = ".." }` / 未被根重命名的继承项。
            out.insert(declared.clone());
        }
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
    let names = dependency_names(manifest, &BTreeMap::new());
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
    let names = dependency_names(manifest, &BTreeMap::new());
    assert!(names.contains("tool-application-extra"));
    assert!(names.contains("tool-transport-wrapper"));
    assert!(
        !names.contains("tool-application"),
        "把近亲名当成真实依赖会让 app 侧守卫给出假证据：{names:?}"
    );
    assert!(!names.contains("tool-transport"), "近亲名误判：{names:?}");
}

/// 第三类提权：重命名声明在**另一份文件**（根清单的 `[workspace.dependencies]`），
/// 成员只写 `alias.workspace = true`。cargo 1.92.0 在临时工作区实测：
/// `cargo tree -p member` -> `inner`，`cargo metadata` ->
/// `{"name":"inner","rename":"sneaky-transport"}`，即解析后的真实名来自根那一条。
/// 只读成员清单就会少报，两份守卫静默保持绿色。
#[test]
fn dependency_names_resolve_aliases_declared_in_the_workspace_table() {
    let root = r#"
[workspace]
members = ["crates/member"]

[workspace.dependencies]
sneaky-transport = { package = "tool-transport", path = "crates/transport" }
sneaky-lua = { package = "tool-lua-host", path = "crates/lua_host" }
sneaky-recorder = { package = "tool-recorder", path = "crates/recorder" }
both-way = { package = "tool-marketplace", path = "crates/marketplace" }
plain-dep = { package = "tool-updater", path = "crates/updater" }
serde = { version = "1", features = ["derive"] }
"#;
    let renames = workspace_dependency_renames(root);
    let member = r#"
[package]
name = "member"

[dependencies]
sneaky-transport.workspace = true
serde.workspace = true
both-way = { workspace = true, package = "tool-extension" }
plain-dep = { path = "../plain" }

[dev-dependencies]
sneaky-lua = { workspace = true }

[target.'cfg(unix)'.build-dependencies]
sneaky-recorder = { workspace = true }
"#;
    let names = dependency_names(member, &renames);
    // 三条继承边（dependencies / dev-dependencies / target 段）都必须落在真实包名上，
    // 别名一个都不许留下：
    for (alias, real) in [
        ("sneaky-transport", "tool-transport"),
        ("sneaky-lua", "tool-lua-host"),
        ("sneaky-recorder", "tool-recorder"),
    ] {
        assert!(
            names.contains(real),
            "{alias}.workspace = true 没解析到 {real}：{names:?}"
        );
        assert!(
            !names.contains(alias),
            "解析后仍报告别名 {alias}，守卫给出的是假证据：{names:?}"
        );
    }
    // 成员同时写 `workspace = true` 和自己的 `package` 时两个候选名都要报：
    // 1.92.0 实测 cargo 用**根**那一条（`cargo tree` -> inner），只报成员侧那个就会少报。
    assert!(
        names.contains("tool-extension") && names.contains("tool-marketplace"),
        "混合写法的两个候选真实名必须都在：{names:?}"
    );
    assert!(
        !names.contains("both-way"),
        "混合写法仍报告声明名：{names:?}"
    );
    // 根表里没有 package 的继承项：声明名就是包名。
    assert!(names.contains("serde"), "未重命名的继承项被弄丢：{names:?}");
    // 没有 `workspace = true` 的条目不受根别名影响（根投毒但未被继承 != 依赖边）。
    assert!(
        names.contains("plain-dep") && !names.contains("tool-updater"),
        "未继承的条目被根别名污染了：{names:?}"
    );
    assert!(!names.contains("member"));
}

/// 别名解析必须一路穿过防空转入口 `manifest_dependencies`，
/// 否则守卫拿到的是「解析前」的集合而元测试仍然全绿。
#[test]
fn manifest_dependencies_threads_the_workspace_alias_table() {
    let root = "[workspace]\nmembers = [\"m\"]\n\n[workspace.dependencies]\nsneaky-transport = { package = \"tool-transport\", path = \"crates/transport\" }\n";
    let renames = workspace_dependency_renames(root);
    let (name, names) = manifest_dependencies(
        "[package]\nname = \"tool-panels\"\n\n[dependencies]\nsneaky-transport.workspace = true\n",
        "fixture: 继承别名的 presentation 清单",
        &renames,
    );
    assert_eq!(name, "tool-panels");
    assert!(
        names.contains("tool-transport"),
        "别名没穿过入口就等于漏洞还在：{names:?}"
    );
    assert_eq!(
        violations(&names, &DOMAIN_CRATES),
        vec!["tool-transport"],
        "共用判定必须给出可点名的违规：{names:?}"
    );
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
sneaky-egui = { package = "egui", path = "crates/egui" }
egui = "0.35"

[package]
name = "b"
description = "提到 tool-transport 与 egui 的长描述"

[features]
tool-transport = []
"#;
    // 同一份文本既当被判定清单、又当根清单：别名表里确实有 egui，
    // 但没有成员继承它，所以它仍然不是依赖边。
    let names = dependency_names(manifest, &workspace_dependency_renames(manifest));
    assert!(names.is_empty(), "非依赖表被当成了依赖：{names:?}");
}

/// 防空转（其一）：判定对象没有 `[package] name` 时必须炸，而不是当作「没有依赖」。
#[test]
#[should_panic(expected = "没有 [package] name")]
fn manifest_without_package_name_cannot_pass_silently() {
    manifest_dependencies(
        "[workspace]\nmembers = [\"crates/a\"]\n",
        "fixture: 虚拟清单",
        &BTreeMap::new(),
    );
}

/// 防空转（其二）：解析出零依赖的清单必须炸 —— 否则调用方的禁令循环等于零断言。
#[test]
#[should_panic(expected = "解析出零依赖")]
fn zero_dependency_manifest_cannot_pass_silently() {
    manifest_dependencies(
        "[package]\nname = \"empty\"\n\n[features]\ndefault = []\n",
        "fixture: 空依赖包",
        &BTreeMap::new(),
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

/// 别名表只收 `[workspace.dependencies]` 里**带 `package`** 的条目：
/// 其余写法（裸版本串、不带 package 的表、加引号的键）都只在「声明名集合」里。
/// 这张表是解析与根守卫的共同输入，读错口径等于两边同时失准。
#[test]
fn workspace_dependency_renames_reads_only_package_entries() {
    let root = r#"
[workspace]
members = ["m"]

[workspace.dependencies]
toml = "1.1"
serde = { version = "1", features = ["derive"] }
"quoted-plain" = { path = "crates/quoted" }
sneaky-transport = { package = "tool-transport", path = "crates/transport" }
"#;
    let renames = workspace_dependency_renames(root);
    let pairs: Vec<(&str, &str)> = renames
        .iter()
        .map(|(alias, real)| (alias.as_str(), real.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![("sneaky-transport", "tool-transport")],
        "别名表口径错了：既不能漏掉 package，也不能把普通条目当成重命名"
    );
    let declared = workspace_dependency_names(root);
    assert_eq!(
        declared.len(),
        4,
        "声明名集合必须覆盖全部四类写法（防空转用的就是这个集合）：{declared:?}"
    );
    assert!(
        declared.contains("quoted-plain"),
        "加引号的键被吞了：{declared:?}"
    );
    assert!(
        workspace_dependency_renames("[package]\nname = \"a\"\n").is_empty(),
        "包清单没有 [workspace.dependencies]，不该凭空造出别名"
    );
    assert!(
        workspace_dependency_names("[package]\nname = \"a\"\n").is_empty(),
        "同上：读错文件时这个集合是空的，调用方据此变红"
    );
}

/// 根清单定位本身也要可验证：本模块被三个 target 各编一次，`env!` 每次展开成
/// 该 target 自己的包目录，所以这条用例是在**逐个 target** 上钉「能读到真的工作区根」——
/// 否则「别名表为空」既可能是根真干净、也可能是路径推导出错悄悄放行了 `alias.workspace = true`。
#[test]
fn root_workspace_manifest_is_reachable_from_every_target() {
    let declared = workspace_dependency_names_from_root();
    for known in ["egui", "serde", "toml", "windows-sys"] {
        assert!(
            declared.contains(known),
            "读到的 [workspace.dependencies] 不含 {known}，不像本工作区的根清单：{declared:?}"
        );
    }
}

/// 根投毒检测器本身必须可失败：禁令条目要点名报出来，干净清单与近亲名不得误报。
/// 当前根清单**没有**任何 `package` 条目，所以真实守卫是零命中运行 ——
/// 这条夹具就是它「能红」的证据之一（另一条：往根清单真加一条重命名，见报告 M2）。
#[test]
fn workspace_renames_to_banned_reports_poisoned_root_entries() {
    let poisoned = r#"
[workspace]
members = ["m"]

[workspace.dependencies]
sneaky-transport = { package = "tool-transport", path = "crates/transport" }
sneaky-egui = { package = "egui", version = "0.35" }
innocent = { package = "serde", version = "1" }
near-miss = { package = "tool-transport-extra", path = "crates/x" }
"#;
    let renames = workspace_dependency_renames(poisoned);
    let mut banned: Vec<&'static str> = Vec::new();
    banned.extend_from_slice(&DOMAIN_CRATES);
    banned.extend_from_slice(&UI_BANS);
    let hits = workspace_renames_to_banned(&renames, &banned);
    assert_eq!(
        hits,
        vec![
            ("egui", "sneaky-egui".to_owned()),
            ("tool-transport", "sneaky-transport".to_owned())
        ],
        "根投毒必须逐条点名（真实名+别名），且不得被子串误报：{hits:?}"
    );
    // 反证：干净根表零命中，但它确实被读到了（否则「零命中」就是空转）。
    let clean = "[workspace]\nmembers = [\"m\"]\n\n[workspace.dependencies]\ntoml = \"1.1\"\nserde = { version = \"1\" }\n";
    let clean_renames = workspace_dependency_renames(clean);
    assert!(
        workspace_renames_to_banned(&clean_renames, &banned).is_empty(),
        "正常根表被误判：{clean_renames:?}"
    );
    assert_eq!(workspace_dependency_names(clean).len(), 2);
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
