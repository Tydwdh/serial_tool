//! 架构回归契约：composition root 与 presentation 的依赖边界。
//!
//! 只读 manifest（不启动 egui、不构建其它 crate），因此可在 headless/CI 环境运行。
//! 目的：一旦有人把领域 crate 重新直连回 presentation 或 composition root，
//! 主门（`cargo test --all-targets`）会立刻变红，而不是在若干轮之后才发现耦合回潮。

use std::path::{Path, PathBuf};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read_manifest(relative: &str) -> String {
    let path = crate_dir().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("读取 {path:?} 失败：{error}"))
}

/// 依赖声明行判定：`name = …`、`name.version = …` 或 `name.workspace = true`。
///
/// 刻意不用裸子串匹配：注释/描述里出现 crate 名时裸子串会给出恒真（或恒假）的假证据，
/// 而 `tool-extension` / `tool-application` 这类前缀关系只有按声明名精确判定才安全。
fn declares_dependency(manifest: &str, crate_name: &str) -> bool {
    manifest.lines().map(str::trim).any(|line| {
        line.starts_with(crate_name) && line[crate_name.len()..].starts_with([' ', '.', '='])
    })
}

/// 依赖判定本身不是恒真/恒假的子串匹配。
#[test]
fn dependency_line_detection_is_exact() {
    let manifest = concat!(
        "[dependencies]\n",
        "tool-application = { path = \"../application\" }\n",
        "crossbeam-channel.workspace = true\n",
        "[features]\n",
        "# 注释里提到 tool-marketplace 不算依赖\n",
        "tool-application-extra = \"1\"\n",
    );
    assert!(declares_dependency(manifest, "tool-application"));
    assert!(declares_dependency(manifest, "crossbeam-channel"));
    assert!(!declares_dependency(manifest, "tool-marketplace"));
    assert!(!declares_dependency(manifest, "tool-app"));
}

/// presentation 层只允许经 `tool-application` 的 DTO 访问领域能力。
///
/// 反向自检（必须能失败）：把 `tool-marketplace = { path = "../marketplace" }`
/// 临时加回 `crates/panels/Cargo.toml`，本用例必须变红。
#[test]
fn panels_manifest_has_no_domain_dependencies() {
    let manifest = read_manifest("../panels/Cargo.toml");
    for forbidden in [
        "tool-transport",
        "tool-recorder",
        "tool-extension",
        "tool-marketplace",
        "tool-lua-host",
        "tool-updater",
    ] {
        assert!(
            !declares_dependency(&manifest, forbidden),
            "crates/panels 必须经由 tool-application 的 DTO 访问领域能力，但 Cargo.toml 仍声明了 {forbidden}"
        );
    }
    assert!(
        declares_dependency(&manifest, "tool-application"),
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
    let manifest = read_manifest("Cargo.toml");
    assert!(declares_dependency(&manifest, "tool-application"));
    assert!(declares_dependency(&manifest, "tool-panels"));
    for forbidden in ["tool-recorder", "tool-extension"] {
        assert!(
            !declares_dependency(&manifest, forbidden),
            "crates/app 不应直连 {forbidden}（该能力属于 tool-application）"
        );
    }
}

/// 依赖裁剪的回归保护：`crossbeam-channel` 在本 crate 内已无任何引用
/// （工作区其它 crate 仍用它，因此裁掉它不会动 `Cargo.lock`）。
#[test]
fn app_manifest_does_not_redeclare_pruned_dependencies() {
    let manifest = read_manifest("Cargo.toml");
    assert!(
        !declares_dependency(&manifest, "crossbeam-channel"),
        "crates/app 已无 crossbeam-channel 的任何引用，不应重新声明该直接依赖"
    );
}
