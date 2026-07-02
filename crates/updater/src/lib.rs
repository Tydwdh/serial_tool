//! 自动更新核心逻辑：检查、下载、校验、替换。
//!
//! 工作流程：
//! 1. 启动时调用 `apply_pending_update`：兼容旧版待更新包
//! 2. 运行时后台请求远端 update.json，发现新版本后下载到 update 目录
//! 3. 下载完成后用户点击"更新并重启"，写入标记并启动临时 helper
//! 4. helper 等主程序退出后替换 exe/resources 并重启主程序

pub mod update_info;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{self, Read as _, Write as _};
use std::net::{SocketAddr, ToSocketAddrs as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// 远端 update.json 的 URL。
pub const UPDATE_JSON_URL: &str =
    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json";
/// 应用 exe 文件名（zip 内顶层）。
pub const APP_EXE_NAME: &str = "hardware-workbench-app.exe";
const UPDATE_USER_AGENT: &str = "HardwareWorkbench-Updater";
const UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const UPDATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const UPDATE_HELPER_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const UPDATE_HELPER_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const UPDATE_HELPER_LAUNCH_TIMEOUT: Duration = Duration::from_secs(2);
const UPDATE_HELPER_LAUNCH_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const GITHUB_UPDATE_HOSTS: &[&str] = &["raw.githubusercontent.com", "github.com"];

fn update_http_client_with_proxy(proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
    // 安全：自定义重定向策略——每次跳转前重新校验目标 host 在下载白名单内。
    // 默认 Policy 会无差别跟随最多 10 次重定向，可被「初始 URL 过白名单 + 302 跳到攻击者域」绕过。
    // 此处放行 GitHub raw→objects.githubusercontent.com 这类正常跳转，拒绝跳到白名单外域。
    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        let host = attempt.url().host_str().unwrap_or("");
        if is_allowed_download_host(host) {
            attempt.follow()
        } else {
            log::warn!("updater: 拒绝重定向到非白名单域 {host}");
            attempt.stop()
        }
    });

    let mut builder = reqwest::Client::builder()
        .user_agent(UPDATE_USER_AGENT)
        .connect_timeout(UPDATE_CONNECT_TIMEOUT)
        .timeout(UPDATE_REQUEST_TIMEOUT)
        .redirect(redirect_policy);

    if let Some(proxy_url) = proxy_url {
        log::info!("updater: 使用代理 {}", redact_proxy_url(proxy_url));
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| format!("解析代理地址失败：{}", describe_reqwest_error(&e)))?;
        builder = builder.proxy(proxy);
    } else {
        builder = apply_github_ipv4_dns_overrides(builder.no_proxy());
    }

    builder
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{}", describe_reqwest_error(&e)))
}

/// 判断 host 是否在下载白名单内（供 redirect Policy 与 validate_download_url 共用）。
fn is_allowed_download_host(host: &str) -> bool {
    DOWNLOAD_HOST_WHITELIST.contains(&host)
}

pub async fn send_update_get(url: &str) -> Result<reqwest::Response, String> {
    let configured_proxy = explicit_proxy_url();
    let client = update_http_client_with_proxy(configured_proxy.as_deref())?;

    match client.get(url).send().await {
        Ok(resp) => Ok(resp),
        Err(primary_error) => {
            let primary_message = describe_reqwest_error(&primary_error);
            if configured_proxy.is_some() {
                return Err(primary_message);
            }

            let Some(proxy_url) = fallback_proxy_url() else {
                return Err(primary_message);
            };

            log::warn!(
                "updater: 直连失败，改用备用代理 {} 重试：{}",
                redact_proxy_url(proxy_url.as_str()),
                primary_message
            );
            let fallback_client = update_http_client_with_proxy(Some(proxy_url.as_str()))?;
            fallback_client.get(url).send().await.map_err(|fallback| {
                format!(
                    "{}；备用代理 {} 也失败：{}",
                    primary_message,
                    redact_proxy_url(proxy_url.as_str()),
                    describe_reqwest_error(&fallback)
                )
            })
        }
    }
}

fn apply_github_ipv4_dns_overrides(mut builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    for host in GITHUB_UPDATE_HOSTS {
        let addrs = resolve_ipv4_socket_addrs(host, 443);
        if addrs.is_empty() {
            continue;
        }
        log::debug!("updater: {host} IPv4 DNS override: {addrs:?}");
        builder = builder.resolve_to_addrs(host, &addrs);
    }
    builder
}

fn resolve_ipv4_socket_addrs(host: &str, port: u16) -> Vec<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .map(|addrs| addrs.filter(|addr| addr.is_ipv4()).collect())
        .unwrap_or_default()
}

pub(crate) fn describe_reqwest_error(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(err) = source {
        message.push_str("；原因：");
        message.push_str(&err.to_string());
        source = err.source();
    }
    message
}

fn explicit_proxy_url() -> Option<String> {
    [
        "HW_UPDATER_PROXY",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok())
    .map(|value| value.trim().to_owned())
    .find(|value| !value.is_empty())
    .map(normalize_proxy_url)
}

fn fallback_proxy_url() -> Option<String> {
    windows_internet_settings_proxy_url()
}

#[cfg(windows)]
fn windows_internet_settings_proxy_url() -> Option<String> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$p=(Get-ItemProperty -Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings').ProxyServer; if ($p) { [Console]::Out.Write($p) }",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let proxy = String::from_utf8_lossy(&output.stdout);
    proxy_server_value_to_url(proxy.trim())
}

#[cfg(not(windows))]
fn windows_internet_settings_proxy_url() -> Option<String> {
    None
}

fn proxy_server_value_to_url(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    for prefix in ["https=", "http="] {
        if let Some(proxy) = value.split(';').find_map(|part| part.strip_prefix(prefix)) {
            return Some(normalize_proxy_url(proxy.to_owned()));
        }
    }

    if !value.contains('=') {
        return Some(normalize_proxy_url(value.to_owned()));
    }

    value
        .split(';')
        .find_map(|part| part.split_once('=').map(|(_, proxy)| proxy))
        .filter(|proxy| !proxy.trim().is_empty())
        .map(|proxy| normalize_proxy_url(proxy.trim().to_owned()))
}

fn normalize_proxy_url(proxy: String) -> String {
    if proxy.contains("://") {
        proxy
    } else {
        format!("http://{proxy}")
    }
}

fn redact_proxy_url(proxy: &str) -> String {
    let Some((scheme, rest)) = proxy.split_once("://") else {
        return proxy.to_owned();
    };
    let Some(at) = rest.rfind('@') else {
        return proxy.to_owned();
    };
    format!("{scheme}://***@{}", &rest[at + 1..])
}

// ── update.json（待更新标记，本地） ──

/// 待更新标记文件，存储在 update 目录下。
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// 待更新版本号
    pub version: String,
    /// 下载的 zip 文件的 SHA256
    pub sha256: String,
    /// 下载完成时间（Unix 时间戳，毫秒）
    pub downloaded_at: u64,
}

// ── 24 小时检查缓存 ──

/// 本地缓存：记录上次检查时间和结果，24 小时内不重复请求。
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckCache {
    /// 上次检查时间（Unix 时间戳，毫秒）
    pub last_check_time: u64,
    /// 上次检查到的最新版本号
    pub latest_version: String,
    /// 上次检查时是否有更新
    pub had_update: bool,
}

/// 缓存有效期：24 小时（毫秒）。
const CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// 返回缓存文件路径。
pub fn check_cache_path() -> PathBuf {
    update_dir().join("check_cache.json")
}

/// 读取缓存。返回 None 表示无缓存或缓存无效。
pub fn read_check_cache() -> Option<CheckCache> {
    let path = check_cache_path();
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// 检查缓存是否仍然有效（24 小时内）。
pub fn is_cache_valid(cache: &CheckCache) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    now.saturating_sub(cache.last_check_time) < CACHE_TTL_MS
}

/// 写入缓存。
pub fn write_check_cache(latest_version: &str, had_update: bool) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cache = CheckCache {
        last_check_time: now,
        latest_version: latest_version.to_owned(),
        had_update,
    };
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let data = serde_json::to_string_pretty(&cache).map_err(|e| format!("序列化缓存失败：{e}"))?;
    std::fs::write(check_cache_path(), data).map_err(|e| format!("写入缓存失败：{e}"))?;
    Ok(())
}

// ── 目录与路径 ──

/// 返回更新工作目录：`%APPDATA%/HardwareWorkbench/update/`
pub fn update_dir() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("HardwareWorkbench")
        .join("update")
}

/// 返回待更新标记文件路径。
pub fn update_manifest_path() -> PathBuf {
    update_dir().join("update.json")
}

/// 返回下载的 zip 文件路径。
pub fn downloaded_zip_path() -> PathBuf {
    update_dir().join("hardware-workbench-app.zip")
}

fn cleanup_partial_download(part_path: &Path) {
    let _ = std::fs::remove_file(part_path);
}

/// 返回临时 helper 目录。helper 不放在 update 目录里，避免更新清理时删到自身。
pub fn update_helper_dir() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("HardwareWorkbench")
        .join("updater")
}

/// 返回 helper 日志路径。
pub fn update_helper_log_path() -> PathBuf {
    update_helper_dir().join("updater.log")
}

fn append_update_helper_log(message: impl AsRef<str>) {
    let dir = update_helper_dir();
    let _ = std::fs::create_dir_all(&dir);
    let line = format!(
        "[{}] {}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        message.as_ref()
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(update_helper_log_path())
    {
        let _ = file.write_all(line.as_bytes());
    }
}

fn cleanup_old_update_helpers(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("hardware-workbench-updater-") && name.ends_with(".exe") {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(windows)]
fn launch_update_helper_elevated(helper_path: &Path, target_exe: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |value: &std::ffi::OsStr| {
        value
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let verb = wide(std::ffi::OsStr::new("runas"));
    let helper = wide(helper_path.as_os_str());
    let parameters = format!("--apply-pending-update \"{}\"", target_exe.display());
    let parameters = wide(std::ffi::OsStr::new(&parameters));

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            helper.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result > 32 {
        append_update_helper_log("elevated helper launch requested");
        Ok(())
    } else {
        Err(format!(
            "请求管理员权限启动 updater helper 失败（ShellExecuteW={result}）：{}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(windows))]
fn launch_update_helper_elevated(_helper_path: &Path, _target_exe: &Path) -> Result<(), String> {
    Err("当前平台不支持请求管理员权限".into())
}

/// 复制当前 exe 为临时 helper，并启动 helper 负责替换目标 exe。
pub fn launch_update_helper(target_exe: &Path) -> Result<(), String> {
    let helper_dir = update_helper_dir();
    std::fs::create_dir_all(&helper_dir).map_err(|e| format!("创建 updater 目录失败：{e}"))?;
    cleanup_old_update_helpers(&helper_dir);

    let launch_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let helper_path = helper_dir.join(format!(
        "hardware-workbench-updater-{}-{launch_id}.exe",
        std::process::id(),
    ));
    std::fs::copy(target_exe, &helper_path).map_err(|e| {
        format!(
            "复制 updater helper 失败（{} → {}）：{e}",
            target_exe.display(),
            helper_path.display()
        )
    })?;

    append_update_helper_log(format!(
        "launch helper {} for target {}",
        helper_path.display(),
        target_exe.display()
    ));

    let deadline = Instant::now() + UPDATE_HELPER_LAUNCH_TIMEOUT;
    loop {
        match Command::new(&helper_path)
            .arg("--apply-pending-update")
            .arg(target_exe)
            .spawn()
        {
            Ok(mut child) => {
                std::thread::sleep(UPDATE_HELPER_LAUNCH_RETRY_INTERVAL);
                match child.try_wait() {
                    Ok(None) => {
                        append_update_helper_log(format!(
                            "helper launched with pid {}",
                            child.id()
                        ));
                        break;
                    }
                    Ok(Some(status)) => {
                        let error = format!("updater helper 启动后立即退出：{status}");
                        append_update_helper_log(&error);
                        if Instant::now() >= deadline {
                            append_update_helper_log(
                                "normal helper launch failed; requesting elevation",
                            );
                            return launch_update_helper_elevated(&helper_path, target_exe)
                                .map_err(|e| {
                                    format!(
                                        "{error}；{e}。日志：{}",
                                        update_helper_log_path().display()
                                    )
                                });
                        }
                    }
                    Err(error) => {
                        append_update_helper_log(format!("检查 helper 状态失败：{error}"));
                        break;
                    }
                }
            }
            Err(error) => {
                append_update_helper_log(format!("launch helper failed: {error}"));
                if Instant::now() >= deadline {
                    append_update_helper_log("normal helper launch failed; requesting elevation");
                    return launch_update_helper_elevated(&helper_path, target_exe).map_err(|e| {
                        format!(
                            "普通权限启动 updater helper 失败：{error}；{e}。日志：{}",
                            update_helper_log_path().display()
                        )
                    });
                }
            }
        }

        std::thread::sleep(UPDATE_HELPER_LAUNCH_RETRY_INTERVAL);
    }

    Ok(())
}

/// 临时 helper 入口：等待主程序退出后替换目标 exe，并重启目标程序。
pub fn run_update_helper(target_exe: &Path) -> Result<bool, String> {
    append_update_helper_log(format!("helper started for {}", target_exe.display()));
    std::thread::sleep(Duration::from_millis(800));

    let deadline = Instant::now() + UPDATE_HELPER_WAIT_TIMEOUT;

    let error = loop {
        match apply_pending_update_impl(target_exe, None) {
            Ok(true) => {
                append_update_helper_log("update applied");
                Command::new(target_exe)
                    .spawn()
                    .map_err(|e| format!("重启应用失败：{e}"))?;
                append_update_helper_log("target restarted");
                return Ok(true);
            }
            Ok(false) => {
                append_update_helper_log("no pending update");
                return Ok(false);
            }
            Err(error) => {
                if Instant::now() >= deadline {
                    break error;
                }
                std::thread::sleep(UPDATE_HELPER_RETRY_INTERVAL);
            }
        }
    };

    append_update_helper_log(format!("update failed: {error}"));
    Err(format!(
        "等待主程序退出并替换更新失败：{error}。日志：{}",
        update_helper_log_path().display()
    ))
}

// ── 启动时替换 ──

/// 启动时检查并应用待更新。
///
/// 返回 `Ok(true)` 表示已应用更新，调用方应重启自身后退出。
/// 返回 `Ok(false)` 表示无待更新。
/// 返回 `Err` 表示替换过程出错（不应阻止正常启动）。
pub fn apply_pending_update(exe_path: &Path) -> Result<bool, String> {
    apply_pending_update_impl(exe_path, Some(env!("CARGO_PKG_VERSION")))
}

fn apply_pending_update_impl(
    exe_path: &Path,
    current_version: Option<&str>,
) -> Result<bool, String> {
    let manifest_path = update_manifest_path();
    if !manifest_path.exists() {
        return Ok(false);
    }

    // 读取标记
    let manifest_data = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读取 update.json 失败：{e}"))?;
    let manifest: UpdateManifest =
        serde_json::from_str(&manifest_data).map_err(|e| format!("解析 update.json 失败：{e}"))?;

    // 兼容旧版启动时更新：仅当远程版本比当前新时才替换。
    // 临时 helper 已由主程序确认是新版本更新，因此可跳过此检查。
    if let Some(current_version) = current_version
        && !update_info::is_newer_version(&manifest.version, current_version)
    {
        log::info!(
            "updater: 待更新版本 {} 不比当前 {} 新，跳过",
            manifest.version,
            current_version
        );
        cleanup_update_dir();
        return Ok(false);
    }

    let zip_path = downloaded_zip_path();
    if !zip_path.exists() {
        log::warn!("updater: update.json 存在但 zip 文件缺失，清理标记");
        cleanup_update_dir();
        return Ok(false);
    }

    // 校验 SHA256
    let actual_sha256 = sha256_file(&zip_path).map_err(|e| format!("计算 zip SHA256 失败：{e}"))?;
    if !actual_sha256.eq_ignore_ascii_case(&manifest.sha256) {
        log::warn!(
            "updater: SHA256 不匹配（期望 {}，实际 {}），清理更新文件",
            manifest.sha256,
            actual_sha256
        );
        cleanup_update_dir();
        return Err(format!(
            "更新包校验失败：SHA256 不匹配（期望 {}，实际 {}）",
            manifest.sha256, actual_sha256
        ));
    }

    log::info!("updater: 开始应用更新 v{}", manifest.version);

    // 解压 zip 到临时目录
    let temp_dir = update_dir().join("extract_tmp");
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).map_err(|e| format!("删除旧临时目录失败：{e}"))?;
    }
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败：{e}"))?;

    // RAII guard：无论后续步骤成功或失败，都清理 temp_dir，避免残留含 exe 的解压文件。
    // 成功路径会在 cleanup_update_dir() 之前 drop（此时 temp_dir 已可删）。
    struct TempDirGuard<'a>(&'a Path);
    impl Drop for TempDirGuard<'_> {
        fn drop(&mut self) {
            if self.0.exists() {
                let _ = std::fs::remove_dir_all(self.0);
            }
        }
    }
    let _temp_guard = TempDirGuard(&temp_dir);

    extract_zip(&zip_path, &temp_dir).map_err(|e| format!("解压更新包失败：{e}"))?;

    // 查找新 exe
    let new_exe = find_exe_in_extracted(&temp_dir)
        .ok_or_else(|| "更新包中未找到 hardware-workbench-app.exe".to_owned())?;

    // 备份当前 exe
    let backup_path = exe_path.with_extension("exe.bak");
    if exe_path.exists() {
        std::fs::copy(exe_path, &backup_path).map_err(|e| format!("备份当前 exe 失败：{e}"))?;
    }

    // 覆盖当前 exe（此时 exe 尚未被锁定，可以覆盖）
    std::fs::copy(&new_exe, exe_path).map_err(|e| {
        // 恢复备份
        if backup_path.exists() {
            let _ = std::fs::copy(&backup_path, exe_path);
        }
        format!("替换 exe 失败：{e}")
    })?;

    // 同时更新 assets/ 等资源
    copy_updated_resources(&temp_dir, exe_path.parent().unwrap_or(Path::new(".")));

    // 删除备份
    let _ = std::fs::remove_file(&backup_path);

    // 清理更新目录（temp_dir 由 _temp_guard 在作用域结束时清理，此处清理其余文件）
    cleanup_update_dir();

    log::info!("updater: 更新 v{} 已应用", manifest.version);
    Ok(true)
}

/// 解压 zip 文件到指定目录。
pub fn extract_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开 zip 失败：{e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取 zip 失败：{e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取 zip 条目 {i} 失败：{e}"))?;

        let out_path = match entry.enclosed_name() {
            Some(path) => dest.join(path),
            None => continue,
        };

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("创建目录 {} 失败：{e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建父目录 {} 失败：{e}", parent.display()))?;
            }
            let mut outfile = std::fs::File::create(&out_path)
                .map_err(|e| format!("创建文件 {} 失败：{e}", out_path.display()))?;
            io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("写入文件 {} 失败：{e}", out_path.display()))?;
        }
    }
    Ok(())
}

/// 解压 zip 并在落盘前拒绝危险可执行扩展名（纵深防御）。
///
/// 与 [`extract_zip`] 的区别：每个文件写入前调用 [`is_unsafe_resource_extension`]，
/// 防止 zip 投递 dll/exe 等可被侧加载的文件。供 marketplace 安装第三方插件时复用。
pub fn extract_zip_filtered(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| format!("打开 zip 失败：{e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("读取 zip 失败：{e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取 zip 条目 {i} 失败：{e}"))?;

        let out_path = match entry.enclosed_name() {
            Some(path) => dest.join(path),
            None => continue,
        };

        if !entry.is_dir() && is_unsafe_resource_extension(&out_path) {
            log::warn!(
                "updater: 解压时跳过可疑可执行文件 {}（扩展名被拒）",
                out_path.display()
            );
            continue;
        }

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("创建目录 {} 失败：{e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建父目录 {} 失败：{e}", parent.display()))?;
            }
            let mut outfile = std::fs::File::create(&out_path)
                .map_err(|e| format!("创建文件 {} 失败：{e}", out_path.display()))?;
            io::copy(&mut entry, &mut outfile)
                .map_err(|e| format!("写入文件 {} 失败：{e}", out_path.display()))?;
        }
    }
    Ok(())
}

/// 在解压后的目录中查找 exe。
fn find_exe_in_extracted(dir: &Path) -> Option<PathBuf> {
    // 直接在根目录查找
    let direct = dir.join(APP_EXE_NAME);
    if direct.exists() {
        return Some(direct);
    }
    // 在子目录中查找（zip 可能包含顶层目录）
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let candidate = path.join(APP_EXE_NAME);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// 从解压目录复制更新的资源文件到安装目录。
fn copy_updated_resources(src_dir: &Path, dest_dir: &Path) {
    let resource_root = if src_dir.join(APP_EXE_NAME).exists() {
        src_dir.to_path_buf()
    } else if let Ok(entries) = std::fs::read_dir(src_dir) {
        let mut found = None;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join(APP_EXE_NAME).exists() {
                found = Some(path);
                break;
            }
        }
        found.unwrap_or_else(|| src_dir.to_path_buf())
    } else {
        src_dir.to_path_buf()
    };

    if let Ok(entries) = std::fs::read_dir(&resource_root) {
        for entry in entries.flatten() {
            let src_path = entry.path();
            let file_name = src_path.file_name().unwrap_or_default();
            if file_name == APP_EXE_NAME || file_name == "logs" || file_name == "extract_tmp" {
                continue;
            }
            // 安全：拒绝可执行扩展名，防止 DLL 侧加载植入（version.dll/winhttp.dll 等）。
            // exe 本身由 helper 替换流程处理，不在此处复制。
            if is_unsafe_resource_extension(&src_path) {
                log::warn!(
                    "updater: 跳过可疑可执行资源 {}（扩展名被拒）",
                    src_path.display()
                );
                continue;
            }
            let dest_path = dest_dir.join(file_name);
            if src_path.is_dir() {
                let _ = copy_dir_recursive(&src_path, &dest_path);
            } else {
                let _ = std::fs::copy(&src_path, &dest_path);
            }
        }
    }
}

/// 判断文件扩展名是否为不应通过更新 zip 投递的可执行类型。
///
/// Windows DLL 搜索顺序优先应用目录，若放行 .dll 等可被侧加载。
/// 根本解是二进制签名校验；此处为纵深防御。
fn is_unsafe_resource_extension(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_ascii_lowercase().as_str(),
            // 原生可执行映像 / 驱动
            "dll" | "exe" | "sys" | "cpl" | "ocx" | "drv" | "scr" | "com" | "pif"
            // 脚本宿主（WSH / mshta）可加载执行
            | "bat" | "cmd" | "ps1" | "vbs" | "hta" | "js" | "jse" | "wsf" | "wsh"
            // 快捷方式 / URL 文件可侧加载
            | "lnk" | "url" | "scf"
        ),
        None => false,
    }
}

/// 递归复制目录。
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), io::Error> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            // 安全：递归复制同样拒绝可执行扩展名，防止把 dll 藏在子目录绕过拦截。
            if is_unsafe_resource_extension(&src_path) {
                log::warn!(
                    "updater: 跳过可疑可执行资源 {}（扩展名被拒）",
                    src_path.display()
                );
                continue;
            }
            std::fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

/// 清理更新目录中的所有文件。
fn cleanup_update_dir() {
    let dir = update_dir();
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

// ── 下载 ──

/// 下载更新 zip 到 update 目录。
///
/// `on_progress` 回调接收 (已下载字节, 总字节) 参数。
/// 返回下载文件的 SHA256 哈希值。
/// 下载 URL 必须满足的安全约束：https 且 host 在白名单。
///
/// 防止被篡改的远端 update.json 把下载指向攻击者控制的域（即使 zip 的 SHA256
/// 与 manifest 一致，manifest 本身也来自同一被篡改的 update.json，无法提供保护）。
/// 这是最廉价的一层劫持面拦截；真正的端到端保护需要二进制签名校验。
const DOWNLOAD_HOST_WHITELIST: &[&str] = &[
    "raw.githubusercontent.com",
    "github.com",
    "objects.githubusercontent.com", // GitHub release 附件实际下载域
];

pub fn validate_download_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| format!("下载 URL 格式无效：{url}"))?;
    if parsed.scheme() != "https" {
        return Err(format!(
            "下载 URL 必须为 https，实际为 {}：{url}",
            parsed.scheme()
        ));
    }
    let host = parsed.host_str().unwrap_or("");
    if !is_allowed_download_host(host) {
        return Err(format!(
            "下载 URL 的域名 {host} 不在允许列表内，疑似被篡改的更新源"
        ));
    }
    Ok(())
}

pub async fn download_update(url: &str, on_progress: impl Fn(u64, u64)) -> Result<String, String> {
    // 安全：强制 https 且 host 在白名单，防止被篡改的 update.json 把下载指向任意域。
    validate_download_url(url)?;

    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;

    let zip_path = downloaded_zip_path();

    // 旧版本可能留下已下载 zip 或半截 .part。下载新版本前先清掉，
    // 避免把旧 zip 的 hash 写进新版本 manifest。
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_file(zip_path.with_extension("zip.part"));

    let hash = download_to_file(url, &zip_path, on_progress).await?;

    // download_to_file 内部已通过 .part 原子 rename 到 zip_path。
    Ok(hash)
}

/// 通用下载：把 URL 内容下载到 `dest_path`，流式写入并同步计算 SHA256，返回哈希值。
///
/// - 安全：调用前应自行调用 `validate_download_url`（本函数不重复校验，供已校验场景复用）。
/// - 原子性：先写 `dest_path + ".part"`，完成后 rename 到 `dest_path`。
/// - 进度：`on_progress(downloaded, total)`，total 为 0 时表示未知长度。
///
/// marketplace 与 updater 共用此实现，避免重复大依赖（reqwest/tokio）与代理/DNS 逻辑。
pub async fn download_to_file(
    url: &str,
    dest_path: &Path,
    on_progress: impl Fn(u64, u64),
) -> Result<String, String> {
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建下载目录失败：{e}"))?;
    }

    let part_path = dest_path.with_extension("zip.part");
    // 清理可能残留的半截 .part。
    let _ = std::fs::remove_file(&part_path);

    let mut resp = send_update_get(url)
        .await
        .map_err(|e| format!("下载失败：{e}"))?;

    if !resp.status().is_success() {
        return Err(format!("下载返回状态码 {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();

    let mut part_file =
        std::fs::File::create(&part_path).map_err(|e| format!("创建临时下载文件失败：{e}"))?;

    let mut last_reported: u64 = 0;
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(e) => {
                drop(part_file);
                cleanup_partial_download(&part_path);
                return Err(format!("下载读取数据失败：{}", describe_reqwest_error(&e)));
            }
        };
        if let Err(e) = part_file.write_all(&chunk) {
            drop(part_file);
            cleanup_partial_download(&part_path);
            return Err(format!("写入下载数据失败：{e}"));
        }
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;

        // 每 1% 或 100KB 回报一次进度，避免过于频繁
        let pct = (downloaded * 100).checked_div(total).unwrap_or(0);
        if pct > last_reported || downloaded.saturating_sub(last_reported) > 100_000 {
            on_progress(downloaded, total);
            last_reported = pct;
        }
    }
    on_progress(downloaded, total);

    drop(part_file);

    // 重命名 .part → 最终文件名
    if let Err(e) = std::fs::rename(&part_path, dest_path) {
        cleanup_partial_download(&part_path);
        return Err(format!("重命名下载文件失败：{e}"));
    }

    let hash = format!("{:x}", hasher.finalize());
    Ok(hash)
}

/// 写入 update.json 标记文件（待更新标记，用于启动时替换）。
///
/// 原子写入：先写 .tmp 再 rename，避免崩溃留下损坏 manifest。
/// 与 ConfigStore 的 atomic_write_json 模式一致。
pub fn write_update_manifest(version: &str, sha256: &str) -> Result<(), String> {
    let manifest = UpdateManifest {
        version: version.to_owned(),
        sha256: sha256.to_owned(),
        downloaded_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };
    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;
    let data = serde_json::to_string_pretty(&manifest).map_err(|e| format!("序列化失败：{e}"))?;
    let manifest_path = update_manifest_path();
    let tmp_path = manifest_path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &data).map_err(|e| format!("写入临时文件失败：{e}"))?;
    // 同目录 rename 是原子的（同一卷）。
    std::fs::rename(&tmp_path, &manifest_path).map_err(|e| format!("重命名 manifest 失败：{e}"))?;
    Ok(())
}

// ── 工具函数 ──

/// 计算文件的 SHA256 哈希值。
pub fn sha256_file(path: &Path) -> Result<String, io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_dir_under_config() {
        let dir = update_dir();
        assert!(dir.to_string_lossy().contains("HardwareWorkbench"));
        assert!(dir.to_string_lossy().contains("update"));
    }

    #[test]
    fn sha256_file_computes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();
        let hash = sha256_file(&file_path).unwrap();
        assert_eq!(hash.len(), 64);
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn normalize_proxy_url_adds_http_scheme() {
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7890".to_owned()),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:7890".to_owned()),
            "socks5://127.0.0.1:7890"
        );
    }

    #[test]
    fn redact_proxy_url_hides_credentials() {
        assert_eq!(
            redact_proxy_url("http://user:pass@127.0.0.1:7890"),
            "http://***@127.0.0.1:7890"
        );
        assert_eq!(
            redact_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
    }

    #[test]
    fn proxy_server_value_to_url_handles_plain_host_port() {
        assert_eq!(
            proxy_server_value_to_url("172.18.88.90:3128").as_deref(),
            Some("http://172.18.88.90:3128")
        );
    }

    #[test]
    fn proxy_server_value_to_url_prefers_https_mapping() {
        assert_eq!(
            proxy_server_value_to_url("http=proxy-a:8080;https=proxy-b:8443").as_deref(),
            Some("http://proxy-b:8443")
        );
    }

    #[test]
    fn find_exe_in_extracted_direct() {
        let dir = tempfile::tempdir().unwrap();
        let exe_path = dir.path().join(APP_EXE_NAME);
        std::fs::write(&exe_path, b"fake exe").unwrap();
        assert_eq!(find_exe_in_extracted(dir.path()), Some(exe_path));
    }

    #[test]
    fn find_exe_in_extracted_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("hardware-workbench-app");
        std::fs::create_dir_all(&subdir).unwrap();
        let exe_path = subdir.join(APP_EXE_NAME);
        std::fs::write(&exe_path, b"fake exe").unwrap();
        assert_eq!(find_exe_in_extracted(dir.path()), Some(exe_path));
    }

    #[test]
    fn find_exe_in_extracted_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("other.txt"), b"not an exe").unwrap();
        assert_eq!(find_exe_in_extracted(dir.path()), None);
    }

    #[test]
    fn check_cache_serialization() {
        let cache = CheckCache {
            last_check_time: 1719300000000,
            latest_version: "0.3.0".into(),
            had_update: true,
        };
        let json = serde_json::to_string_pretty(&cache).unwrap();
        let parsed: CheckCache = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_check_time, 1719300000000);
        assert_eq!(parsed.latest_version, "0.3.0");
        assert!(parsed.had_update);
    }

    #[test]
    fn is_cache_valid_within_24h() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cache = CheckCache {
            last_check_time: now - 1000, // 1 秒前
            latest_version: "0.3.0".into(),
            had_update: false,
        };
        assert!(is_cache_valid(&cache));
    }

    #[test]
    fn is_cache_valid_expired() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cache = CheckCache {
            last_check_time: now - CACHE_TTL_MS - 1, // 过期 1ms
            latest_version: "0.3.0".into(),
            had_update: false,
        };
        assert!(!is_cache_valid(&cache));
    }

    // ── #2: 下载 URL 域白名单 + https 强制 ──
    #[test]
    fn validate_download_url_accepts_github_https() {
        assert!(
            validate_download_url(
                "https://github.com/Tydwdh/serial_tool/releases/download/v0.4.2/app.zip"
            )
            .is_ok()
        );
        assert!(validate_download_url("https://objects.githubusercontent.com/abc/pkg.zip").is_ok());
        assert!(
            validate_download_url(
                "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json"
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_download_url_rejects_http() {
        let err = validate_download_url("http://github.com/x.zip").unwrap_err();
        assert!(err.contains("https"), "got: {err}");
    }

    #[test]
    fn validate_download_url_rejects_off_domain() {
        assert!(validate_download_url("https://evil.example.com/x.zip").is_err());
        assert!(validate_download_url("https://github.com.evil.com/x.zip").is_err());
    }

    #[test]
    fn validate_download_url_rejects_malformed() {
        assert!(validate_download_url("not a url").is_err());
        assert!(validate_download_url("").is_err());
    }

    // ── #15: 资源扩展名过滤 ──
    #[test]
    fn is_unsafe_resource_extension_flags_executables() {
        assert!(is_unsafe_resource_extension(Path::new("version.dll")));
        assert!(is_unsafe_resource_extension(Path::new("winhttp.DLL")));
        assert!(is_unsafe_resource_extension(Path::new("evil.exe")));
        assert!(is_unsafe_resource_extension(Path::new("evil.SYS")));
        assert!(is_unsafe_resource_extension(Path::new("run.bat")));
    }

    #[test]
    fn is_unsafe_resource_extension_allows_safe() {
        assert!(!is_unsafe_resource_extension(Path::new("assets/icon.png")));
        assert!(!is_unsafe_resource_extension(Path::new("config.json")));
        assert!(!is_unsafe_resource_extension(Path::new("README")));
        assert!(!is_unsafe_resource_extension(Path::new("font.ttf")));
    }
}
