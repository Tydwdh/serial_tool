//! 架构回归契约：composition root 与 presentation 的依赖边界。
//!
//! 只读 manifest（不启动 egui、不构建其它 crate），因此可在 headless/CI 环境运行。
//! 目的：一旦有人把领域 crate 重新直连回 presentation 或 composition root，
//! 主门（`cargo test --all-targets`）会立刻变红，而不是在若干轮之后才发现耦合回潮。
//!
//! 判定基于**解析后的真实依赖名**：`{ package = "tool-transport" }` 重命名、
//! `"tool-transport" = {...}` 加引号的键、`[target.<cfg>.*dependencies]` 段里的声明
//! 都必须被抓到。按声明键做文本/行匹配会漏检前两种合法写法，使本守卫静默保持绿色 ——
//! 这是评审实证过的两个绕过向量，`manifest_deps.rs` 的元测试逐条钉住它们。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[path = "manifest_deps.rs"]
mod manifest_deps;

use manifest_deps::{DOMAIN_CRATES, manifest_dependencies, violations};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// 读一份 crate 清单，返回（包名，解析后的真实依赖名集合）。
///
/// 读不到就 panic（不是 `unwrap_or_default()`，那会把路径写错变成零断言）；
/// 包名缺失与零依赖这两道防空转由共用判定 `manifest_dependencies` 负责，两份守卫同一份实现。
fn read_manifest_dependencies(relative: &str) -> (String, BTreeSet<String>) {
    let path = crate_dir().join(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", path.display()));
    manifest_dependencies(&text, &path.display().to_string())
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
