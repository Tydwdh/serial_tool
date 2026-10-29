//! 架构回归契约：composition root 与 presentation 的依赖边界。
//!
//! 只读 manifest（不启动 egui、不构建其它 crate），因此可在 headless/CI 环境运行。
//! 目的：一旦有人把领域 crate 重新直连回 presentation 或 composition root，
//! 主门（`cargo test --all-targets`）会立刻变红，而不是在若干轮之后才发现耦合回潮。
//!
//! 判定基于**解析后的真实依赖名**：`{ package = "tool-transport" }` 重命名、
//! `"tool-transport" = {...}` 加引号的键、`[target.<cfg>.*dependencies]` 段里的声明、
//! 以及写在**根清单** `[workspace.dependencies]` 里再由 `alias.workspace = true` 继承的重命名，
//! 都必须被抓到。按声明键做文本/行匹配会漏检前两种合法写法，只读成员清单会漏检最后一种，
//! 两种情况下本守卫都会静默保持绿色 —— 这些都是实证过的绕过向量，
//! `manifest_deps.rs` 的元测试逐条钉住它们。
//!
//! 本文件还有第二类判定：**源码文本形状守卫**。它只用于钉住 native 侧
//! 无法在运行期驱动的形状（命令面板使用键、端口条目投影）：这些模块能编译，
//! 但渲染需要 egui 上下文，运行期用例覆盖不到，于是把"只能有一份/只能用一个键"
//! 钉在源码文本上。读文件失败一律 panic（路径写错时不能退化成零断言），
//! 判定前还会去掉整行注释，免得注释里的代码样本把守卫喂成假绿。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[path = "manifest_deps.rs"]
mod manifest_deps;

use manifest_deps::{
    DOMAIN_CRATES, manifest_dependencies, violations, workspace_dependency_renames_from_root,
};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// 读一份 crate 清单，返回（包名，解析后的真实依赖名集合）。
///
/// 读不到就 panic（不是 `unwrap_or_default()`，那会把路径写错变成零断言）；
/// 包名缺失与零依赖这两道防空转由共用判定 `manifest_dependencies` 负责，两份守卫同一份实现。
/// 根清单的别名表由共用判定之外的 `workspace_dependency_renames_from_root()` 提供，
/// 它内部用 `env!("CARGO_MANIFEST_DIR")` 定位根，故两份守卫拿到的是同一张表。
fn read_manifest_dependencies(relative: &str) -> (String, BTreeSet<String>) {
    let path = crate_dir().join(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()));
    manifest_dependencies(
        &text,
        &path.display().to_string(),
        &workspace_dependency_renames_from_root(),
    )
}

/// presentation 层只允许经 `tool-application` 的 DTO 访问领域能力。
///
/// 反向自检（必须真能失败）：往 `crates/panels/Cargo.toml` 的 `[dependencies]` 临时加
/// `my-transport = { package = "tool-transport", path = "../transport" }`，
/// `cargo test -p hardware-workbench-app --test architecture` 必须红并点名 tool-transport。
#[test]
fn panels_manifest_has_no_domain_dependencies() {
    let (name, names) = read_manifest_dependencies("../panels/Cargo.toml");
    assert_eq!(name, "tool-panels", "读错了清单，判定对象不是 tool-panels");
    let hits = violations(&names, &DOMAIN_CRATES);
    assert!(
        hits.is_empty(),
        "crates/panels 必须经由 tool-application 的 DTO 访问领域能力，\
         但 Cargo.toml 实际依赖了 {hits:?}（解析后集合：{names:?}）"
    );
    assert!(
        names.contains("tool-application"),
        "crates/panels 必须显式声明 tool-application（否则是把能力删掉了，而不是收敛到边界）"
    );
}

/// composition root 仍站在 application 边界上，且不直连属于 application 的底层 crate。
///
/// 说明：`tool-databus` / `tool-transport` / `tool-marketplace` / `tool-lua-host` /
/// `tool-updater` 目前仍是 composition root 的真实能力（bus 构造、RepaintWaker、
/// 端口自然排序、市场下载/安装、更新器），收敛它们需要 `tool-application` 先暴露等价 API。
/// 这里只锁定**已经落地**的边界，不写"未来目标"式断言（那种断言现在恒失败）。
#[test]
fn app_manifest_keeps_the_application_boundary() {
    let (name, names) = read_manifest_dependencies("Cargo.toml");
    assert_eq!(
        name, "hardware-workbench-app",
        "读错了清单，判定对象不是 composition root"
    );
    for required in ["tool-application", "tool-panels"] {
        assert!(
            names.contains(required),
            "composition root 必须依赖 {required}（实际依赖：{names:?}）"
        );
    }
    let hits = violations(&names, &["tool-recorder", "tool-extension", "tool-testing"]);
    assert!(
        hits.is_empty(),
        "crates/app 不应直连 {hits:?}（该能力属于 tool-application）；实际依赖：{names:?}"
    );
}

/// 依赖裁剪的回归保护：`crossbeam-channel` 在本 crate 内已无任何引用
/// （工作区其它 crate 仍用它，因此裁掉它不会动 `Cargo.lock`）。
#[test]
fn app_manifest_does_not_redeclare_pruned_dependencies() {
    let (_, names) = read_manifest_dependencies("Cargo.toml");
    assert!(
        !names.contains("crossbeam-channel"),
        "crates/app 已无 crossbeam-channel 的任何引用，不应重新声明该直接依赖（实际依赖：{names:?}）"
    );
}

/// 读一份本 crate 的源码文本（形状守卫的判定对象）。
///
/// 读不到必须 panic：`unwrap_or_default()` 会让"路径写错"退化成"文件里没有那条
/// 写法"，正/负判定同时静默通过。
fn read_source(relative: &str) -> String {
    let path = crate_dir().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()))
}

/// 去掉整行注释后把所有空白压掉：判定就不再依赖换行与缩进（rustfmt 重排链条会让
/// 逐行匹配假红），而注释里的代码样本也不会把守卫喂成假绿。
fn squeezed_code(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(|line| line.chars().filter(|ch| !ch.is_whitespace()))
        .collect()
}

/// 命令面板的「使用顺序」必须以命令 id 为键，而不是标题。
///
/// 标题会随插件与改名变，id 不会；设置项 `command_usage_order` 的读写都必须用 id。
#[test]
fn palette_usage_order_is_keyed_by_command_id() {
    // 判定带分号：只比"前缀"会让 insert(0, command_id.clone().to_uppercase()) 蒙过去。
    const RECORD_TO_FRONT: &str = "usage_order.insert(0,command_id.clone());";

    let native = squeezed_code(&read_source("src/ui/command_palette.rs"));
    let config = squeezed_code(&read_source("src/config.rs"));

    // 载入侧必须整份赋回：`usage_order` 里认不出的值只是排不到前面，不是脏数据，
    // 在读入侧按 id 裁剪等于把用户的历史删掉。分号同样不能省 ——
    // `= cfg.command_usage_order.into_iter().filter(..).collect()` 也得被抓到。
    assert!(
        config.contains("usage_order=cfg.command_usage_order;"),
        "载入配置必须把 command_usage_order **整份**赋回面板状态（找 `usage_order = cfg.command_usage_order;`）"
    );

    assert!(
        native.contains("position(|used|used==&entry.id)"),
        "排序读取必须拿使用顺序里的字符串比命令 id（找 `used == &entry.id`）"
    );
    assert!(
        native.contains(RECORD_TO_FRONT),
        "记录使用必须把命令 id 插到最前（找 {RECORD_TO_FRONT}）；写成标题就是键空间分叉"
    );
    // 派发出口只有一个值，且就是命令 id：标题再被带出来当键，这条就会红。
    assert!(
        native.contains("pending_command=Some(command_id);"),
        "面板派发出去的命令 id 必须就是记录使用的那个值（找 `pending_command = Some(command_id);`）"
    );
    for forbidden in [
        "used==&entry.label",
        "used==&entry.title",
        "Option<(String,String)>",
    ] {
        assert!(
            !native.contains(forbidden),
            "命令面板的候选派发/排序不许再把标题（label/title）当使用键，\
             也不许把 (id, 标题) 成对带出渲染层：发现 {forbidden}"
        );
    }
}

/// native 端口条目投影只准有一份，且就住在 `ui/mod.rs`（顶栏与设备面板都从那儿取）。
///
/// 这份映射喂给 tool-panels 的可见判据：`kind == "网络"` 决定"移除网络端口"按钮出不
/// 出现，`kind` 非空才在 label 后加 ` · 类别`；同一个映射分写两处就会改一处漏一处。
/// 而它钉不住运行期用例——实测把 `kind` 写空、`pending_reconnect` 写死 false，整包仍
/// 全绿（那两个面板都要 `&eframe::CreationContext` 才渲染得动），所以只能钉源码形状。
#[test]
fn native_port_item_projection_exists_exactly_once() {
    let dir = crate_dir().join("src/ui");
    let mut hits: Vec<String> = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    entries.sort();
    assert!(
        entries.len() >= 3,
        "src/ui 至少要扫到 3 个源文件，实际 {} 个（扫不到就是判定对象为空）",
        entries.len()
    );
    for path in entries {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()));
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        // 只数真正的字段初始化：`-> SerialPortItem {` 后面跟的是函数体，不是投影。
        let occurrences = squeezed_code(&text).matches("SerialPortItem{id:").count();
        for _ in 0..occurrences {
            hits.push(name.clone());
        }
    }
    assert_eq!(
        hits,
        vec!["mod.rs".to_owned()],
        "native 的 SerialPortItem 投影必须只有一份、且在 src/ui/mod.rs 里；\
         多出来的名字就是又分家了（顶栏/设备面板请调用 WorkbenchApp::native_port_items）"
    );
}

fn collect_production_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_production_rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// 删除 Web 版后，生产源码与清单里不许再出现 wasm 目标。
#[test]
fn no_wasm_target_in_production_sources() {
    let root = crate_dir()
        .parent()
        .and_then(Path::parent)
        .expect("crates/app 之上应恰好两级到工作区根")
        .to_path_buf();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(root.join("crates")).expect("读取 crates/") {
        let crate_path = entry.expect("读取 crate 目录项").path();
        if !crate_path.is_dir() {
            continue;
        }
        let manifest = crate_path.join("Cargo.toml");
        if manifest.is_file() {
            files.push(manifest);
        }
        let build = crate_path.join("build.rs");
        if build.is_file() {
            files.push(build);
        }
        collect_production_rust_sources(&crate_path.join("src"), &mut files);
    }
    assert!(
        files.len() > 50,
        "只扫到 {} 个生产文件，扫描路径大概错了 —— 这条守卫会退化成零断言",
        files.len()
    );
    let mut offenders = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()));
        let haystack = if path.extension().is_some_and(|ext| ext == "rs") {
            squeezed_code(&text)
        } else {
            text
        };
        if haystack.contains(concat!("wasm", "32")) {
            offenders.push(
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    assert!(
        offenders.is_empty(),
        "生产源码/清单里仍有 wasm 目标：{offenders:?}"
    );
}
