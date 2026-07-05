//! 插件市场客户端：拉取 registry.json、下载并安装插件。
//!
//! 复用 `tool-updater` 的安全/下载/解压能力（域白名单 + https + SHA256 + 扩展名黑名单），
//! 避免重复引入 reqwest/tokio/zip/sha2 等大依赖。
//!
//! 安装目标：`install_dir/<plugin_id>/`（跟随 exe，卸载干净）。
//! 不持久化"已安装列表"——由 PluginManager::discover_roots 扫描决定。

use std::path::{Path, PathBuf};
use tool_updater as updater;

/// 默认市场 registry URL。
pub const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/Tydwdh/serial_tool-plugins/main/registry.json";

// ── 数据模型（对应 registry.json schema） ──

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Registry {
    pub version: u32,
    pub updated: String,
    pub plugins: Vec<RegistryPlugin>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct RegistryPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    /// 图标 URL（当前 registry 固定为 null，保留字段以兼容）。
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    pub download_url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub published: Option<String>,
}

// ── 拉取 registry ──

/// 拉取并解析 registry.json。
///
/// 复用 updater 的 HTTP 客户端（代理/直连/DNS override 逻辑一致）。
/// 必须在 tokio runtime 上下文中调用。
pub async fn fetch_registry(url: &str) -> Result<Registry, String> {
    // 安全：registry 源 URL 同样要走域白名单 + https 校验（纵深防御，与 download_url 一致）。
    updater::validate_download_url(url)?;

    let resp = updater::send_update_get(url)
        .await
        .map_err(|e| format!("拉取市场索引失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("拉取市场索引返回状态码 {}", resp.status()));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取市场索引失败：{e}"))?;
    let registry: Registry =
        serde_json::from_str(&text).map_err(|e| format!("解析市场索引失败：{e}"))?;
    Ok(registry)
}

/// 校验插件 id 是否可作为安全的文件系统路径片段。
///
/// id 直接拼进 `install_dir/<id>/` 等路径，必须禁止路径穿越（`/`、`\`、`..`、
/// 绝对路径前缀、以 `.` 开头等）。合法 id 形如 `gcode-sender`。
pub fn validate_plugin_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("插件 id 不能为空".to_owned());
    }
    if id.starts_with('.') {
        return Err(format!("插件 id 不能以点开头：{id}"));
    }
    // 禁止任何路径分隔符与父目录引用。
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("插件 id 含非法路径字符：{id}"));
    }
    // 禁止 Windows 驱动器/UNC 前缀（如 C:、\\host）。
    if id.len() >= 2 && id.as_bytes()[1] == b':' {
        return Err(format!("插件 id 形似驱动器路径：{id}"));
    }
    // 仅允许字母、数字、点、连字符、下划线（与常见插件 id 规范一致）。
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(format!("插件 id 含非法字符（仅允许字母数字 . - _）：{id}"));
    }
    Ok(())
}

// ── 安装 ──

/// 安装一个市场插件到 `install_dir/<id>/`。
///
/// 流程：id 校验 → URL 校验 → 下载到临时 zip → SHA256 校验 → 解压到临时目录
///      → 定位 `<id>/` 顶层目录 → 原子替换 `install_dir/<id>/`（旧版本改名留待清理）→ 清理临时文件。
///
/// `on_progress(downloaded, total)` 用于进度反馈（total=0 表示未知长度）。
/// 若 `install_dir/<id>/` 已存在（重装/升级），将旧目录同卷 rename 到
/// `<id>.old.<pid>/` 而非直接删除——避免 Windows 下文件被占用导致删除失败
/// （同卷 rename 仅需目录本身可写，不要求逐个文件未被打开）。
pub async fn install_plugin(
    entry: &RegistryPlugin,
    install_dir: &Path,
    on_progress: impl Fn(u64, u64),
) -> Result<(), String> {
    // 0. id 路径校验（防止恶意 registry 用 `../` 之类 id 写到 install_dir 之外）
    validate_plugin_id(&entry.id)?;

    // 1. 域白名单 + https 校验
    updater::validate_download_url(&entry.download_url)?;

    std::fs::create_dir_all(install_dir).map_err(|e| format!("创建插件目录失败：{e}"))?;

    // 2. 下载到临时 zip（复用通用下载，含原子 rename + 流式 SHA256）
    let tmp_zip = install_dir.join(format!("{}.download.zip", entry.id));
    let actual_sha = updater::download_to_file(&entry.download_url, &tmp_zip, on_progress).await?;

    // 3. SHA256 校验（大小写不敏感）
    if !actual_sha.eq_ignore_ascii_case(&entry.sha256) {
        let _ = std::fs::remove_file(&tmp_zip);
        return Err(format!(
            "插件 {} 校验失败：SHA256 不匹配（期望 {}，实际 {}）",
            entry.id, entry.sha256, actual_sha
        ));
    }

    // 4. 解压到临时目录
    let extract_dir = install_dir.join(format!("{}.extract_tmp", entry.id));
    if extract_dir.exists() {
        std::fs::remove_dir_all(&extract_dir).map_err(|e| format!("清理旧解压目录失败：{e}"))?;
    }
    std::fs::create_dir_all(&extract_dir).map_err(|e| format!("创建解压目录失败：{e}"))?;

    // RAII：无论后续成败都清理临时解压目录与下载 zip。
    struct TempGuard {
        paths: Vec<PathBuf>,
    }
    impl Drop for TempGuard {
        fn drop(&mut self) {
            for p in &self.paths {
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(p);
                } else {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }
    let _guard = TempGuard {
        paths: vec![tmp_zip.clone(), extract_dir.clone()],
    };

    // 安全：解压时拒绝危险可执行扩展名（纵深防御）。
    updater::extract_zip_filtered(&tmp_zip, &extract_dir)?;

    // 5. 定位 zip 内的 `<id>/` 顶层目录
    let plugin_root = find_plugin_root_in_extracted(&extract_dir, &entry.id).ok_or_else(|| {
        format!(
            "插件包结构异常：解压后未找到顶层目录 {}/（含 plugin.json）",
            entry.id
        )
    })?;

    // 6. 原子替换 install_dir/<id>/：
    //    若旧版本存在，先同卷 rename 到 <id>.old.<pid>/（不删，避免文件锁失败），
    //    再把新版本 rename 到位。旧目录留待下次启动清理（retire_old_plugin_dirs）。
    let dest = install_dir.join(&entry.id);
    let mut retired: Option<PathBuf> = None;
    if dest.exists() {
        let old_dir = install_dir.join(format!("{}.old.{}", entry.id, std::process::id()));
        // 若上次残留同名 .old 目录，先尝试清理（失败则换名）。
        let old_dir = ensure_unique_dir(&old_dir);
        std::fs::rename(&dest, &old_dir).map_err(|e| {
            format!(
                "暂存旧版本插件 {} 失败（{} → {}）：{e}。请先在「已安装」tab 禁用该插件后重试",
                entry.id,
                dest.display(),
                old_dir.display()
            )
        })?;
        retired = Some(old_dir);
    }
    // 移动而非复制：插件包通常不大，且避免重复 IO。
    if let Err(e) = std::fs::rename(&plugin_root, &dest) {
        // 新版本就位失败：若已暂存旧版本，回滚——把旧目录 rename 回 dest。
        if let Some(old_dir) = retired.as_ref() {
            let _ = std::fs::rename(old_dir, &dest);
        }
        return Err(format!(
            "安装插件 {} 失败（{} → {}）：{e}",
            entry.id,
            plugin_root.display(),
            dest.display()
        ));
    }
    // 新版本就位成功：尽力清理暂存的旧版本（失败也无伤大雅，下次启动会清）。
    if let Some(old_dir) = retired {
        let _ = std::fs::remove_dir_all(&old_dir);
    }

    log::info!(
        "marketplace: 插件 {} v{} 已安装到 {}",
        entry.id,
        entry.version,
        dest.display()
    );
    Ok(())
}

/// 若目录已存在，追加数字后缀直到不冲突（用于 .old 暂存目录命名）。
fn ensure_unique_dir(base: &Path) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let parent = base.parent().unwrap_or_else(|| Path::new(""));
    let stem = base.file_name().and_then(|n| n.to_str()).unwrap_or("old");
    for i in 1..1000 {
        let candidate = parent.join(format!("{stem}.{i}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base.to_path_buf()
}

/// 清理安装目录下残留的 `<id>.old.<pid>/` 暂存目录。
///
/// 在应用启动时调用：上次安装若在「新版本就位后、旧目录删除前」退出，会留下 .old 目录。
/// 仅清理形如 `<前缀>.old.<纯数字>` 的目录，避免误删用户手动放置的合法插件目录。
pub fn retire_old_plugin_dirs(install_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(install_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // 形如 <id>.old.<pid>：split 出 `.old.` 后必须是非空纯数字后缀。
        let Some(suffix) = name.split(".old.").nth(1) else {
            continue;
        };
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            // 尽力清理；失败则保留（不影响功能，仅占少量磁盘）。
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// 在解压目录中查找 `<id>/` 顶层目录（须含 `plugin.json`）。
///
/// 支持两种布局：
/// - 直接：`<extract_dir>/<id>/plugin.json`
/// - 单层包裹：`<extract_dir>/<任意目录>/<id>/plugin.json`（发布脚本不应产生，但容错）
pub fn find_plugin_root_in_extracted(extract_dir: &Path, plugin_id: &str) -> Option<PathBuf> {
    // 直接在根目录查找 <id>/plugin.json
    let direct = extract_dir.join(plugin_id).join("plugin.json");
    if direct.exists() {
        return Some(extract_dir.join(plugin_id));
    }
    // 单层包裹容错
    if let Ok(entries) = std::fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join(plugin_id).join("plugin.json");
                if candidate.exists() {
                    return Some(path.join(plugin_id));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_REGISTRY: &str = r#"{
        "version": 1,
        "updated": "2026-07-02T04:47:12Z",
        "plugins": [
            {
                "id": "gcode-sender",
                "name": "G-code Sender",
                "version": "0.1.0",
                "api_version": "0.1",
                "description": "面向 Marlin 的 G-code 发送器",
                "author": "Tydwdh",
                "homepage": "https://github.com/Tydwdh/serial_tool",
                "repository": "https://github.com/Tydwdh/serial_tool",
                "license": "MIT",
                "category": "gcode",
                "icon": null,
                "permissions": ["bus", "log", "serial"],
                "download_url": "https://raw.githubusercontent.com/Tydwdh/serial_tool-plugins/main/plugins/gcode-sender/0.1.0/gcode-sender-0.1.0.zip",
                "sha256": "c05822f7ae52f42b5e0f2b55c51d0b9203fa355a49cd5a9b5b0cd3df5f27bc66",
                "size": 7940,
                "published": "2026-07-02T04:45:26Z"
            }
        ]
    }"#;

    #[test]
    fn parse_registry_sample() {
        let reg: Registry = serde_json::from_str(SAMPLE_REGISTRY).unwrap();
        assert_eq!(reg.version, 1);
        assert_eq!(reg.plugins.len(), 1);
        let p = &reg.plugins[0];
        assert_eq!(p.id, "gcode-sender");
        assert_eq!(p.name, "G-code Sender");
        assert_eq!(p.permissions, vec!["bus", "log", "serial"]);
        assert_eq!(p.size, 7940);
        assert!(p.icon.is_none());
    }

    #[test]
    fn parse_registry_tolerates_missing_optional_fields() {
        let minimal = r#"{
            "version": 1,
            "updated": "",
            "plugins": [{
                "id": "x", "name": "X", "version": "1", "api_version": "0.1",
                "download_url": "https://raw.githubusercontent.com/a/b/c.zip",
                "sha256": "abc"
            }]
        }"#;
        let reg: Registry = serde_json::from_str(minimal).unwrap();
        let p = &reg.plugins[0];
        assert!(p.description.is_none());
        assert!(p.permissions.is_empty());
        assert_eq!(p.size, 0);
    }

    #[test]
    fn find_plugin_root_direct() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("gcode-sender");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("plugin.json"), b"{}").unwrap();
        let found = find_plugin_root_in_extracted(dir.path(), "gcode-sender").unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn find_plugin_root_wrapped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wrapper").join("gcode-sender");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("plugin.json"), b"{}").unwrap();
        let found = find_plugin_root_in_extracted(dir.path(), "gcode-sender").unwrap();
        assert_eq!(found, root);
    }

    #[test]
    fn find_plugin_root_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("other")).unwrap();
        assert!(find_plugin_root_in_extracted(dir.path(), "gcode-sender").is_none());
    }

    #[test]
    fn validate_download_url_reused_from_updater() {
        // 复用 updater 的域白名单：github 三域通过，其他域拒绝。
        assert!(
            updater::validate_download_url(
                "https://raw.githubusercontent.com/Tydwdh/serial_tool-plugins/main/registry.json"
            )
            .is_ok()
        );
        assert!(updater::validate_download_url("https://evil.example.com/x.zip").is_err());
        assert!(updater::validate_download_url("http://github.com/x.zip").is_err());
    }

    #[test]
    fn validate_plugin_id_accepts_normal() {
        assert!(validate_plugin_id("gcode-sender").is_ok());
        assert!(validate_plugin_id("template.hello").is_ok());
        assert!(validate_plugin_id("a_b-c.d").is_ok());
    }

    #[test]
    fn validate_plugin_id_rejects_traversal() {
        assert!(validate_plugin_id("").is_err());
        assert!(validate_plugin_id("..").is_err());
        assert!(validate_plugin_id(".hidden").is_err());
        assert!(validate_plugin_id("a/b").is_err());
        assert!(validate_plugin_id("a\\b").is_err());
        assert!(validate_plugin_id("../evil").is_err());
        assert!(validate_plugin_id("C:evil").is_err());
        // 含空格 / 中文等非法字符
        assert!(validate_plugin_id("a b").is_err());
        assert!(validate_plugin_id("插件").is_err());
    }
}
