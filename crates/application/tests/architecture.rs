//! Architecture contract：非 presentation crate 不得引入 UI/窗口依赖，
//! 也不得反向依赖 presentation 层。
//!
//! 判定基于解析后的真实依赖名（含 `package = ` 重命名、加引号的键、`[target.*]` 段，
//! 以及**根清单** `[workspace.dependencies]` 里由 `alias.workspace = true` 继承来的重命名），
//! 不是清单文本包含 —— 后者可被三种合法写法绕过，见 `manifest_deps.rs` 的元测试。
//!
//! 检查对象是**推导**出来的：扫描 `crates/*/Cargo.toml`，按 `[package] name` 跳过两个
//! presentation crate。此前这里硬编码了 `tool-application` 的 4 个可达内部 crate（实际 11 个），
//! 且完全没有覆盖 `tool-platform` —— 它是 `application` 与 `panels` 共同的无条件基座，
//! 一旦引入 egui，两条边的传递闭包同时失守而两份守卫都不红。
//!
//! 与 `crates/app/tests/architecture.rs` 共用同一份判定实现（`#[path]` 引同一文件），
//! 避免「修了一处、另一处仍可绕过」。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "../../app/tests/manifest_deps.rs"]
mod manifest_deps;

use manifest_deps::{
    DOMAIN_CRATES, PRESENTATION_CRATES, UI_BANS, manifest_dependencies, violations,
    workspace_dependency_names_from_root, workspace_dependency_renames_from_root,
    workspace_members, workspace_renames_to_banned,
};

/// 四道防空转下限：任何一道掉下来都说明扫描面在缩水，而不是边界变干净了。
/// 数值按当前工作区（15 个 crate 目录 / 2 个 presentation / 16 个成员 / 18 条 workspace deps）
/// 取「不大于实际」的口径，只在覆盖变少时变红。
const WORKSPACE_MEMBER_FLOOR: usize = 16;
const CRATE_MANIFEST_FLOOR: usize = 15;
const CHECKED_MANIFEST_FLOOR: usize = 13;
const WORKSPACE_DEPENDENCY_FLOOR: usize = 15;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/<name> 之上应恰好两级到工作区根")
        .to_path_buf()
}

/// `crates/*/Cargo.toml`，排序保证失败信息稳定。
fn crate_manifests() -> Vec<PathBuf> {
    let crates_dir = workspace_root().join("crates");
    let mut manifests: Vec<PathBuf> = fs::read_dir(&crates_dir)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", crates_dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path().join("Cargo.toml")))
        .filter(|path| path.is_file())
        .collect();
    manifests.sort();
    // 空集合会让下面的循环变成零断言 —— 正是本仓刚修过的缺陷类别。
    assert!(
        manifests.len() >= CRATE_MANIFEST_FLOOR,
        "在 {} 只扫到 {} 份 crate 清单（下限 {CRATE_MANIFEST_FLOOR}），扫描路径或过滤条件出错",
        crates_dir.display(),
        manifests.len()
    );
    manifests
}

/// 读清单并交出 `(包名, 真实依赖名)`；读不到直接 panic（不是 `unwrap_or_default()`，
/// 那会把路径写错退化成零断言）。「包名缺失」与「零依赖」两道防空转由共用判定负责。
/// `workspace_renames` 是根 `[workspace.dependencies]` 的别名表，由调用方读一次传进来
/// （全量扫描要判 13 份清单，逐份重读根清单只是噪音）。
fn read_package_manifest(
    path: &Path,
    workspace_renames: &BTreeMap<String, String>,
) -> (String, BTreeSet<String>) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()));
    manifest_dependencies(&text, &path.display().to_string(), workspace_renames)
}

#[test]
fn no_non_presentation_crate_depends_on_ui() {
    let mut checked = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    let workspace_renames = workspace_dependency_renames_from_root();

    for path in crate_manifests() {
        let (name, names) = read_package_manifest(&path, &workspace_renames);
        // 按 [package] name 跳过，不按目录名：`path.display().to_string().contains("crates/app")`
        // 在 Windows 上恒为 false（分隔符是 `\`），会把两个 presentation crate 混进检查集。
        if PRESENTATION_CRATES.contains(&name.as_str()) {
            skipped.push(name);
            continue;
        }
        let ui_hits = violations(&names, &UI_BANS);
        assert!(
            ui_hits.is_empty(),
            "{}（{}）不得依赖 UI/窗口 crate：{ui_hits:?}；实际依赖：{names:?}",
            name,
            path.display()
        );
        let upward_hits = violations(&names, &PRESENTATION_CRATES);
        assert!(
            upward_hits.is_empty(),
            "{}（{}）不得反向依赖 presentation 层：{upward_hits:?}；实际依赖：{names:?}",
            name,
            path.display()
        );
        checked += 1;
    }

    assert!(
        checked >= CHECKED_MANIFEST_FLOOR,
        "实际检查 {checked} 份清单，少于下限 {CHECKED_MANIFEST_FLOOR}：扫描或跳过条件出错，\
         本用例会退化成零断言"
    );
    // 跳过集必须**恰好**是两个 presentation crate：漏跳会让守卫当场红，多跳则是静默放行。
    skipped.sort();
    let mut expected: Vec<&str> = PRESENTATION_CRATES.to_vec();
    expected.sort();
    assert_eq!(
        skipped, expected,
        "被跳过的清单应当恰是 presentation 层（按 [package] name 判定）"
    );
}

#[test]
fn tool_platform_is_ui_free_so_the_boundary_rests_on_it() {
    // tool-platform 是 application 与 panels 的共同基座：它一旦被污染，
    // 两条边的传递闭包同时失守，而任何单清单检查都看不见。
    let path = workspace_root().join("crates/platform/Cargo.toml");
    let (name, names) = read_package_manifest(&path, &workspace_dependency_renames_from_root());
    assert_eq!(
        name, "tool-platform",
        "读错了清单，判定对象不是 tool-platform"
    );
    let hits = violations(&names, &UI_BANS);
    assert!(
        hits.is_empty(),
        "{name} 不得依赖 {hits:?}（UI/窗口依赖）；实际依赖：{names:?}"
    );
}

/// `tool-application` 自己那条边：UI 与 presentation 都不许出现。
///
/// 单独留一份针对本 crate 的窄断言，作为上面全量扫描的兜底 ——
/// 万一 `PRESENTATION_CRATES` 的跳过条件出错把本 crate 也跳掉了，全量扫描会静默失声。
#[test]
fn application_manifest_has_no_ui_or_presentation_dependencies() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let (name, names) = read_package_manifest(&path, &workspace_dependency_renames_from_root());
    assert_eq!(
        name, "tool-application",
        "读错了清单，判定对象不是 tool-application"
    );
    let banned: Vec<&'static str> = UI_BANS
        .iter()
        .chain(PRESENTATION_CRATES.iter())
        .copied()
        .collect();
    for banned in banned {
        assert!(
            !names.contains(banned),
            "tool-application Cargo.toml 不得依赖 {banned}（实际依赖：{names:?}）"
        );
    }
}

/// 根清单 `[workspace.dependencies]` 不得把别名指向 UI/领域/presentation crate。
///
/// 为什么单独钉这一条：那张表**不是**依赖边，所以所有按清单扫描的守卫都刻意看不见它 ——
/// 于是它成了「改一行就能给 13 份清单同时投毒」的唯一位置。解析层修好后（成员侧
/// `alias.workspace = true` 会解析到真实名）投毒要成立仍需再改一份成员清单，
/// 但那一步本来就会被成员守卫抓住；这条断言的价值是让**根**这次编辑当场变红并点名，
/// 而不是只靠「根清单的改动在评审里看得见」这种口头约束。
///
/// 反向自检（必须真能红；当前根表没有 `package` 条目，所以夹具才是它唯一的可失败证据，
/// 见 `manifest_deps.rs::workspace_renames_to_banned_reports_poisoned_root_entries`）：
/// 往根 `[workspace.dependencies]` 加
/// `sneaky-transport = { package = "tool-transport", path = "crates/transport" }`，
/// `cargo +1.92.0 test -p tool-application --test architecture` 必须红并点名 tool-transport。
#[test]
fn root_workspace_dependencies_do_not_rename_to_banned_crates() {
    let declared = workspace_dependency_names_from_root();
    assert!(
        declared.len() >= WORKSPACE_DEPENDENCY_FLOOR,
        "根 [workspace.dependencies] 只有 {} 项（下限 {WORKSPACE_DEPENDENCY_FLOOR}）：\
         读错文件或表被删空会让本用例退化成零断言",
        declared.len()
    );
    let banned: Vec<&'static str> = UI_BANS
        .iter()
        .chain(DOMAIN_CRATES.iter())
        .chain(PRESENTATION_CRATES.iter())
        .copied()
        .collect();
    let hits = workspace_renames_to_banned(&workspace_dependency_renames_from_root(), &banned);
    assert!(
        hits.is_empty(),
        "根清单 [workspace.dependencies] 不得用 `package = ` 把别名指向禁令 crate：{hits:?}\
         （成员写 `别名.workspace = true` 就能拿到它的 API，而这张表本身不是依赖边，\
         按清单扫描的守卫全都看不见）"
    );
}

/// 扫描面必须覆盖全部工作区成员：落在 `crates/` 下的会被自动扫到，
/// 不在该目录下的成员必须在这里显式登记，否则它就是逃出了守卫。
#[test]
fn every_workspace_member_is_covered_by_the_scan() {
    let root = workspace_root();
    let text = fs::read_to_string(root.join("Cargo.toml"))
        .unwrap_or_else(|error| panic!("读取工作区根清单失败：{error}"));
    let members = workspace_members(&text);
    assert!(
        members.len() >= WORKSPACE_MEMBER_FLOOR,
        "根清单 workspace.members 只有 {} 项（下限 {WORKSPACE_MEMBER_FLOOR}），\
         成员表被删短会让守卫覆盖变少而无声",
        members.len()
    );
    for member in &members {
        let manifest = root.join(member).join("Cargo.toml");
        assert!(
            manifest.is_file(),
            "工作区成员 {member} 没有 Cargo.toml：{} 读不到",
            manifest.display()
        );
    }
    let outside: BTreeSet<String> = members
        .iter()
        .filter(|member| !member.starts_with("crates/"))
        .cloned()
        .collect();
    assert_eq!(
        outside,
        BTreeSet::from(["vendor/egui_tiles".to_owned()]),
        "crates/ 之外的工作区成员不会被清单扫描覆盖：新增必须在此登记并说明理由"
    );
}
