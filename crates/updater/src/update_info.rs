//! 静态 update.json 解析：从远端获取最新版本信息。
//!
//! 使用静态 JSON 文件而非 GitHub Releases API，避免 API 限流问题。

use serde::Deserialize;

/// 远端 update.json 的结构。
#[derive(Debug, Deserialize, Clone)]
pub struct UpdateInfo {
    /// 最新版本号，如 "0.3.0"
    pub version: String,
    /// 发布日期，如 "2026-06-25"
    pub date: String,
    /// 下载 URL（指向 GitHub Release 的 zip）
    pub download_url: String,
    /// 发布产物的 SHA256。**必填**：更新包字节必须与这个外置固定值一致，
    /// 否则校验退化成"拿下载内容跟下载内容比"，只约束发布仓库写权限之外的人。
    pub sha256: String,
    /// 更新日志
    #[serde(default)]
    pub changelog: Vec<String>,
}

/// 是否为可信的 SHA256 固定值：恰好 64 个十六进制字符。
pub fn is_pinned_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 从远端获取 update.json。
pub async fn fetch_update_info(url: &str) -> Result<UpdateInfo, String> {
    fetch_update_info_with_network_settings(url, &crate::NetworkSettings::default()).await
}

pub async fn fetch_update_info_with_network_settings(
    url: &str,
    network: &crate::NetworkSettings,
) -> Result<UpdateInfo, String> {
    let resp = crate::send_update_get_with_network_settings(url, network)
        .await
        .map_err(|e| format!("请求更新信息失败：{e}"))?
        .response;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("获取更新信息返回 {status}：{body}"));
    }

    let info = resp
        .json::<UpdateInfo>()
        .await
        .map_err(|e| format!("解析更新信息失败：{}", crate::describe_reqwest_error(&e)))?;

    if !is_pinned_sha256(&info.sha256) {
        return Err(format!(
            "update.json 的 sha256 不是合法的 64 位十六进制摘要（得到 {:?}），拒绝更新",
            info.sha256
        ));
    }

    Ok(info)
}

/// 比较两个语义版本号。返回 `true` 表示 `remote` 比 `local` 新。
pub fn is_newer_version(remote: &str, local: &str) -> bool {
    let parse_parts =
        |v: &str| -> Vec<u32> { v.split('.').filter_map(|s| s.parse::<u32>().ok()).collect() };
    let rp = parse_parts(remote);
    let lp = parse_parts(local);
    for i in 0..rp.len().max(lp.len()) {
        let r = rp.get(i).copied().unwrap_or(0);
        let l = lp.get(i).copied().unwrap_or(0);
        if r != l {
            return r > l;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_version_basic() {
        assert!(is_newer_version("0.3.0", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.3.0"));
        assert!(!is_newer_version("0.2.0", "0.2.0"));
    }

    #[test]
    fn is_newer_version_minor() {
        assert!(is_newer_version("0.2.1", "0.2.0"));
        assert!(!is_newer_version("0.2.0", "0.2.1"));
    }

    #[test]
    fn is_newer_version_major() {
        assert!(is_newer_version("1.0.0", "0.9.9"));
        assert!(!is_newer_version("0.9.9", "1.0.0"));
    }

    #[test]
    fn is_newer_version_different_length() {
        assert!(is_newer_version("0.2.0.1", "0.2.0"));
        assert!(!is_newer_version("0.2", "0.2.0"));
    }

    #[test]
    fn parse_update_info() {
        let json = r#"{
            "version": "0.3.0",
            "date": "2026-06-25",
            "download_url": "https://github.com/Tydwdh/serial_tool/releases/download/v0.3.0/hardware-workbench-app.zip",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "changelog": ["修复若干问题", "新增自动更新功能"]
        }"#;
        let info: UpdateInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.version, "0.3.0");
        assert_eq!(info.date, "2026-06-25");
        assert!(info.download_url.contains("v0.3.0"));
        assert_eq!(
            info.sha256,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert!(is_pinned_sha256(&info.sha256));
        assert_eq!(info.changelog.len(), 2);
    }

    #[test]
    fn parse_update_info_empty_changelog() {
        let json = r#"{
            "version": "0.3.0",
            "date": "2026-06-25",
            "download_url": "https://example.com/app.zip",
            "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        }"#;
        let info: UpdateInfo = serde_json::from_str(json).unwrap();
        assert!(info.changelog.is_empty());
    }

    #[test]
    fn update_info_requires_sha256_field() {
        // 缺 sha256 必须硬失败：静默接受 = 自我循环校验的旧行为回来了。
        let missing = r#"{"version":"1.3.0","date":"2026-09-19","download_url":"https://github.com/Tydwdh/serial_tool/releases/download/v1.3.0/hardware-workbench-app.zip","changelog":[]}"#;
        let error = serde_json::from_str::<UpdateInfo>(missing)
            .expect_err("缺少 sha256 字段的 update.json 必须解析失败");
        assert!(
            error.to_string().contains("sha256"),
            "错误信息应点名缺失字段，实际：{error}"
        );
    }

    #[test]
    fn update_info_rejects_malformed_sha256() {
        for bad in ["", "deadbeef", &"a".repeat(63), &"g".repeat(64)] {
            assert!(
                !is_pinned_sha256(bad),
                "{bad:?} 不是合法的 64 位十六进制摘要"
            );
        }
        assert!(is_pinned_sha256(&"a".repeat(64)));
        assert!(
            is_pinned_sha256(&("0123456789ABCDEF".to_owned() + &"f".repeat(48))),
            "大写十六进制同样应视为合法固定值"
        );
    }
}
