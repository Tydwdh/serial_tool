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
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use update_info::is_pinned_sha256;

/// 远端 update.json 的 URL。
pub const UPDATE_JSON_URL: &str =
    "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json";
/// 应用 exe 文件名（zip 内顶层）。
pub const APP_EXE_NAME: &str = "hardware-workbench-app.exe";
const UPDATE_USER_AGENT: &str = "HardwareWorkbench-Updater";
const UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const UPDATE_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const NETWORK_RETRY_COUNT: u8 = 2;
const NETWORK_RETRY_DELAY: Duration = Duration::from_millis(350);
const UPDATE_HELPER_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const UPDATE_HELPER_RETRY_INTERVAL: Duration = Duration::from_millis(500);
const UPDATE_HELPER_LAUNCH_TIMEOUT: Duration = Duration::from_secs(2);
const UPDATE_HELPER_LAUNCH_RETRY_INTERVAL: Duration = Duration::from_millis(100);
/// 网络设置。`proxy_url` 非空时强制使用该代理；为空时使用环境与系统代理探测，
/// 但回环/字面 IP 目标一律直连（见 `NetworkSettings::route_for_url`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkSettings {
    pub proxy_url: Option<String>,
}

impl NetworkSettings {
    pub fn with_proxy(proxy_url: Option<String>) -> Self {
        Self {
            proxy_url: proxy_url
                .filter(|value| !value.trim().is_empty())
                .map(normalize_proxy_url),
        }
    }

    /// 该 URL 实际走哪条网络路径。
    fn route_for_url(&self, url: &str) -> ProxyRoute {
        self.route_for_url_with_env_proxy(url, explicit_proxy_url())
    }

    /// [`NetworkSettings::route_for_url`] 的纯函数部分：环境代理的探测结果由参数注入。
    ///
    /// 拆开是为了能单测 —— edition 2024 下 `std::env::set_var` 是 `unsafe`，
    /// 而测试默认并行跑，用例无法可靠地把 `HTTP_PROXY` 摆成想要的样子。
    fn route_for_url_with_env_proxy(&self, url: &str, env_proxy: Option<String>) -> ProxyRoute {
        // 用户在设置里手填的代理是显式决定，任何目标都照走。
        if let Some(proxy_url) = &self.proxy_url {
            return ProxyRoute {
                proxy_url: Some(proxy_url.clone()),
                force_direct: false,
            };
        }
        // 回环 / 字面 IP 目标交给环境代理基本只会失败：代理进程不一定转发 loopback，
        // 而这里一旦显式装上 `Proxy::all`，reqwest 连 `NO_PROXY` 都不再看，
        // 设了例外也救不回来。所以这类目标必须真直连（见 `update_http_client`）。
        if is_direct_route_url(url) {
            return ProxyRoute {
                proxy_url: None,
                force_direct: true,
            };
        }
        ProxyRoute {
            proxy_url: env_proxy,
            force_direct: false,
        }
    }

    fn route_label(&self, route: &ProxyRoute) -> &'static str {
        if self.proxy_url.is_some() {
            "自定义代理"
        } else if route.force_direct {
            "直连"
        } else if route.proxy_url.is_some() {
            "环境代理"
        } else {
            "系统代理或直连"
        }
    }
}

/// 一次请求的代理决策：用哪个代理，以及是否必须直连。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ProxyRoute {
    proxy_url: Option<String>,
    /// `true` 表示目标是回环/字面 IP：连 reqwest 自己的环境/系统代理探测一起关掉。
    force_direct: bool,
}

/// 目标是不是「该直连」的地址：`localhost`、回环地址或字面 IP。
///
/// 这两类都不属于"要翻墙/要出网"的范畴，环境代理（`HTTP_PROXY` 等）是为公网域名
/// 设的，套到它们身上只会把请求送到一个不认识目标的代理进程里。
/// URL 解析失败时返回 `false`：判定不吞掉任何真实请求的代理行为，
/// 报错的活儿留给各调用点（`validate_download_url` 等）。
fn is_direct_route_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    // IPv6 在 URL 里带方括号，`parse::<IpAddr>()` 只认裸地址。
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok()
}

/// 一次成功请求使用的网络路径，用于 UI 状态与日志诊断。
#[derive(Clone, Debug)]
pub struct NetworkDiagnostics {
    route: &'static str,
    tls: &'static str,
    http: &'static str,
    attempts: u8,
}

impl NetworkDiagnostics {
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · {} · 第 {} 次尝试",
            self.route, self.tls, self.http, self.attempts
        )
    }
}

pub struct NetworkResponse {
    pub response: reqwest::Response,
    pub diagnostics: NetworkDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum TlsBackend {
    Native,
    Rustls,
}

impl TlsBackend {
    fn label(self) -> &'static str {
        match self {
            Self::Native => "Windows TLS",
            Self::Rustls => "Rustls TLS",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ClientKey {
    proxy_url: Option<String>,
    tls: TlsBackend,
    /// 目标为回环/字面 IP：必须直连，见 `NetworkSettings::route_for_url`。
    force_direct: bool,
}

static HTTP_CLIENTS: OnceLock<Mutex<std::collections::HashMap<ClientKey, reqwest::Client>>> =
    OnceLock::new();

fn update_http_client(key: &ClientKey) -> Result<reqwest::Client, String> {
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

    builder = match key.tls {
        TlsBackend::Native => builder.use_native_tls(),
        TlsBackend::Rustls => builder.use_rustls_tls(),
    };

    if key.force_direct {
        // 光是"不传代理"还不够：`proxy_url = None` 时 reqwest 会自己再挂一个
        // 环境/系统代理探测器，而它只认 `NO_PROXY`。`no_proxy()` 同时清空代理列表
        // 并关掉那个自动探测，是保证真的直连本地端口的唯一写法。
        builder = builder.no_proxy();
    } else if let Some(proxy_url) = &key.proxy_url {
        log::info!("updater: 使用代理 {}", redact_proxy_url(proxy_url));
        let proxy = reqwest::Proxy::all(proxy_url)
            .map_err(|e| format!("解析代理地址失败：{}", describe_reqwest_error(&e)))?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{}", describe_reqwest_error(&e)))
}

fn shared_http_client(key: ClientKey) -> Result<reqwest::Client, String> {
    let clients = HTTP_CLIENTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut clients = clients
        .lock()
        .map_err(|_| "HTTP 客户端缓存锁已损坏".to_owned())?;
    if let Some(client) = clients.get(&key) {
        return Ok(client.clone());
    }
    let client = update_http_client(&key)?;
    clients.insert(key, client.clone());
    Ok(client)
}

/// 判断 host 是否在下载白名单内（供 redirect Policy 与 validate_download_url 共用）。
fn is_allowed_download_host(host: &str) -> bool {
    DOWNLOAD_HOST_WHITELIST.contains(&host)
}

pub async fn send_update_get(url: &str) -> Result<reqwest::Response, String> {
    Ok(
        send_update_get_with_network_settings(url, &NetworkSettings::default())
            .await?
            .response,
    )
}

/// 按 `settings` 与目标 URL 决定的代理路径，用 native TLS 与 Rustls TLS 依次尝试请求。
///
/// 代理是**逐 URL** 决定的：回环/字面 IP 目标直连，不吃环境代理
/// （`NetworkSettings::route_for_url`）。
/// 不强制 IPv4，保留系统 DNS 的正常地址选择与回退能力。
pub async fn send_update_get_with_network_settings(
    url: &str,
    settings: &NetworkSettings,
) -> Result<NetworkResponse, String> {
    let route = settings.route_for_url(url);
    let mut failures = Vec::new();

    for tls in [TlsBackend::Native, TlsBackend::Rustls] {
        let client = shared_http_client(ClientKey {
            proxy_url: route.proxy_url.clone(),
            tls,
            force_direct: route.force_direct,
        })?;
        for attempt in 1..=NETWORK_RETRY_COUNT {
            match client.get(url).send().await {
                Ok(response) => {
                    let http = match response.version() {
                        reqwest::Version::HTTP_2 => "HTTP/2",
                        reqwest::Version::HTTP_3 => "HTTP/3",
                        _ => "HTTP/1.1",
                    };
                    return Ok(NetworkResponse {
                        response,
                        diagnostics: NetworkDiagnostics {
                            route: settings.route_label(&route),
                            tls: tls.label(),
                            http,
                            attempts: attempt,
                        },
                    });
                }
                Err(error) => {
                    let retryable = error.is_connect() || error.is_timeout();
                    failures.push(format!(
                        "{} 第 {} 次：{}",
                        tls.label(),
                        attempt,
                        describe_reqwest_error(&error)
                    ));
                    if retryable && attempt < NETWORK_RETRY_COUNT {
                        tokio::time::sleep(NETWORK_RETRY_DELAY * u32::from(attempt)).await;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    Err(format!(
        "网络请求失败（{}）：{}",
        settings.route_label(&route),
        failures.join("；")
    ))
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
    /// 上次检查时 `update.json` 携带的 pinned 摘要。
    ///
    /// 缓存是本机可丢弃数据、不是信任根，故允许 `default`：老缓存或被人改空的
    /// 记录会拿到 `""`，调用方据此当作未命中并重新拉取 `update.json`。
    #[serde(default)]
    pub sha256: String,
}

/// 缓存有效期：24 小时（毫秒）。
const CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// 返回缓存文件路径。
pub fn check_cache_path() -> PathBuf {
    update_dir().join("check_cache.json")
}

/// 读取缓存记录本身。**不做时效判断**：读不到文件或反序列化失败才返回 `None`，
/// 返回 `Some` 只说明"磁盘上有一条能解析的记录"，可能已经过期。
/// 时效由 [`is_cache_valid`] 单独判定，调用方两步都要走。
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

/// 写入缓存。`sha256` 为本次 `update.json` 的 pinned 摘要，供缓存命中时复用。
pub fn write_check_cache(
    latest_version: &str,
    had_update: bool,
    sha256: &str,
) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cache = CheckCache {
        last_check_time: now,
        latest_version: latest_version.to_owned(),
        had_update,
        sha256: sha256.to_owned(),
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

/// 探测目标 exe 所在目录当前用户是否有写权限。
///
/// 用于在启动 helper 前判断是否需要提权：安装到 Program Files 时普通用户对
/// 该目录无写权限，helper（普通权限）替换 exe（需在目录内重命名/创建文件）
/// 会失败。直接对运行中的 exe 做 open(write) 在 Windows 会因共享违例失败，
/// 即使有权限也判否，故改为在 exe 同目录尝试创建临时文件来探测目录可写性。
/// 不可写 ⇒ 需要 elevated。
fn exe_is_writable(target_exe: &Path) -> bool {
    let Some(dir) = target_exe.parent() else {
        return false;
    };
    let probe = dir.join(format!(
        ".hw_update_probe_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
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

    // 提前探测目标 exe 是否可写。安装到 Program Files 时，普通用户无写权限，
    // helper（普通权限）反复 std::fs::copy 替换 exe 必然失败、重试 30s 后退出，
    // 而主程序早已 exit 无法转 elevated。故此处不可写时直接走 elevated（runas），
    // 让 helper 以管理员权限运行，一次成功。可写时走普通启动路径。
    if !exe_is_writable(target_exe) {
        append_update_helper_log(
            "target exe not writable by current user; requesting elevation upfront",
        );
        return launch_update_helper_elevated(&helper_path, target_exe).map_err(|e| {
            format!(
                "请求管理员权限启动 updater helper 失败：{e}。日志：{}",
                update_helper_log_path().display()
            )
        });
    }

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

pub async fn download_update(
    url: &str,
    on_progress: impl Fn(u64, u64),
    expected_sha256: &str,
) -> Result<String, String> {
    download_update_with_network_settings(
        url,
        &NetworkSettings::default(),
        on_progress,
        expected_sha256,
    )
    .await
}

/// 下载更新包，并与 `update.json` 里的外置 pinned 摘要比对。
///
/// `expected_sha256` 必须来自更新清单，**不能**来自本次下载：拿下载内容跟下载
/// 内容比只能发现磁盘损坏，不能约束下载到了什么。
pub async fn download_update_with_network_settings(
    url: &str,
    network: &NetworkSettings,
    on_progress: impl Fn(u64, u64),
    expected_sha256: &str,
) -> Result<String, String> {
    // 安全：强制 https 且 host 在白名单，防止被篡改的 update.json 把下载指向任意域。
    validate_download_url(url)?;

    let dir = update_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败：{e}"))?;

    let zip_path = downloaded_zip_path();

    // 旧版本可能留下已下载 zip 或半截 .part。下载新版本前先清掉，
    // 避免把旧 zip 的 hash 写进新版本 manifest。
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_file(zip_path.with_extension("zip.part"));

    let hash =
        download_verified_update_package(url, &zip_path, network, on_progress, expected_sha256)
            .await?;

    // download_verified_update_package 已在校验通过后才做 .part → zip_path 原子 rename。
    Ok(hash)
}

/// 更新包专用入口：把清单的 pinned 摘要交给共用的下载实现。
///
/// 参数刻意是**非 Option** 的 `&str`：[`download_to_file_verified`] 的
/// `Option<&str>` 是给无固定值的调用方（marketplace）留的口子，而更新路径一旦在
/// 这里传 `None`，校验就静默消失，且旧行为（"拿下载内容跟下载内容比"）会带着全绿
/// 的测试回来。收窄成 `&str` 后，这个决定只可能显式写在下面这次调用里，而它由
/// `tests::update_download_*` 用真实 HTTP 响应直接覆盖。
async fn download_verified_update_package(
    url: &str,
    dest_path: &Path,
    network: &NetworkSettings,
    on_progress: impl Fn(u64, u64),
    expected_sha256: &str,
) -> Result<String, String> {
    download_to_file_verified(url, dest_path, network, on_progress, Some(expected_sha256)).await
}

/// 通用下载：把 URL 内容下载到 `dest_path`，流式写入并同步计算 SHA256，返回哈希值。
///
/// - 安全：调用前应自行调用 `validate_download_url`（本函数不重复校验，供已校验场景复用）。
/// - 原子性：先写 `dest_path + ".part"`，完成后 rename 到 `dest_path`。
/// - 进度：`on_progress(downloaded, total)`，total 为 0 时表示未知长度。
///
/// 本函数只是"用默认网络设置"的便捷壳，**本工作区当前没有调用者**。真正被共用的
/// 是它下面的 `download_to_file_verified`，两条路径各自传不传固定值：
/// marketplace 走 `download_to_file_with_network_settings`（`expected_sha256 = None`，
/// 它拿 registry 里的 `sha256` 自行比对），更新包走 `download_verified_update_package`
/// （`Some(pin)`，比对发生在 rename 之前，不匹配就不留下产物）。
pub async fn download_to_file(
    url: &str,
    dest_path: &Path,
    on_progress: impl Fn(u64, u64),
) -> Result<String, String> {
    download_to_file_with_network_settings(url, dest_path, &NetworkSettings::default(), on_progress)
        .await
}

pub async fn download_to_file_with_network_settings(
    url: &str,
    dest_path: &Path,
    network: &NetworkSettings,
    on_progress: impl Fn(u64, u64),
) -> Result<String, String> {
    download_to_file_verified(url, dest_path, network, on_progress, None).await
}

/// [`download_to_file_with_network_settings`] 的实现体。
///
/// `expected_sha256 = Some(pin)` 时，流式哈希必须在 `.part → dest_path` 的 rename
/// **之前**与该外置固定值一致：不匹配就删掉半截文件并报错，绝不留下未通过校验的产物。
async fn download_to_file_verified(
    url: &str,
    dest_path: &Path,
    network: &NetworkSettings,
    on_progress: impl Fn(u64, u64),
    expected_sha256: Option<&str>,
) -> Result<String, String> {
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建下载目录失败：{e}"))?;
    }

    let part_path = dest_path.with_extension("zip.part");
    // 清理可能残留的半截 .part。
    let _ = std::fs::remove_file(&part_path);

    let mut resp = send_update_get_with_network_settings(url, network)
        .await
        .map_err(|e| format!("下载失败：{e}"))?
        .response;

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

    let hash = format!("{:x}", hasher.finalize());
    if let Some(expected) = expected_sha256 {
        // 先比对 rename：不匹配就不留下任何可被 apply 读到的文件。
        if let Err(e) = verify_stream_sha256(&hash, expected) {
            cleanup_partial_download(&part_path);
            return Err(e);
        }
    }

    // 重命名 .part → 最终文件名
    if let Err(e) = std::fs::rename(&part_path, dest_path) {
        cleanup_partial_download(&part_path);
        return Err(format!("重命名下载文件失败：{e}"));
    }

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

/// 下载流哈希与外置 pinned 值的唯一比对点。
/// 独立成函数是为了让"不匹配必须拒绝"这一条可被单测直接命中。
pub fn verify_stream_sha256(actual: &str, expected: &str) -> Result<(), String> {
    if !is_pinned_sha256(expected) {
        return Err(format!(
            "拒绝校验：更新清单的 sha256 不合法（{expected:?}）"
        ));
    }
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "更新包校验失败：SHA256 不匹配（清单声明 {expected}，实际下载 {actual}）"
        ));
    }
    Ok(())
}

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
    fn verify_stream_sha256_rejects_mismatch_and_malformed_pin() {
        let pin = "a".repeat(64);
        assert!(verify_stream_sha256(&pin, &pin).is_ok());
        assert!(verify_stream_sha256(&"b".repeat(64), &pin).is_err());
        assert!(
            verify_stream_sha256(&pin, &pin.to_uppercase()).is_ok(),
            "比对须大小写无关"
        );
        // 关键：pinned 值本身不合法时不得放行 —— 空串/短串都不能当通行证。
        assert!(verify_stream_sha256(&pin, "").is_err());
        assert!(verify_stream_sha256(&pin, "deadbeef").is_err());
    }

    // ── 下载收尾的**接线**回归测试 ──
    //
    // 上面那条只证明"比对函数会拒绝"，而原漏洞的成因是"校验存在但位置不对"：
    // 把 verify 与 rename 换个顺序，或把更新路径传给共用实现的 `Some(pin)` 改成
    // `None`，两处改动都能让全量测试保持绿色。下面两条用例用真实 HTTP 响应跑
    // `download_verified_update_package`（更新路径实际调用的那个函数），把接线钉住。
    //
    // 反向自检（必须能失败，改完请还原）：
    //   A. 把 `download_to_file_verified` 里的 rename 挪到 `verify_stream_sha256`
    //      之前 → `update_download_rejects_mismatch_before_renaming_part` 变红。
    //   B. 把 `download_verified_update_package` 的 `Some(expected_sha256)` 改成
    //      `None` → 同一条用例变红（拿不到 Err，且最终文件已落盘）。
    //
    // 走 127.0.0.1 明文端口不削弱被测点：https/域名白名单是
    // `download_update_with_network_settings` 的前置门，本用例测的是它下面的收尾。
    // 本用例对环境变量不敏感：回环目标一律直连，`HTTP_PROXY` / `ALL_PROXY` 之类
    // 不会把这条请求交给代理（`NetworkSettings::route_for_url`，由
    // `env_proxy_never_applies_to_loopback_or_literal_ip_targets` 钉住）。
    // 注意 `NO_PROXY` 从来不是可行的补救手段 —— 一旦显式装上 `Proxy::all`，
    // reqwest 就不再看它。

    /// 被测载荷，以及**独立**算出的 SHA256（`printf '%s' <载荷> | sha256sum`）。
    /// 摘要刻意写死而不在测试里现算：自算自比正是这次修掉的自我循环。
    const UPDATE_PAYLOAD: &[u8] = b"pinned-update-payload-for-wiring-regression-test";
    const UPDATE_PAYLOAD_SHA256: &str =
        "e9dff2af24a338fe8b1d29cdd4e9b7eb88188603806951d73f4708b8e144d790";

    /// 一次性本地 HTTP 服务器：`TcpListener` + 手写 200 响应，不引入任何 HTTP 依赖。
    struct LocalPayloadServer {
        url: String,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }

    impl LocalPayloadServer {
        fn start() -> Self {
            // 端口写 0、再读回真实端口：不猜端口，就不会和并行跑的其它测试撞车。
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("绑定一次性本地 HTTP 端口");
            let port = listener.local_addr().expect("读回分配的端口").port();
            listener
                .set_nonblocking(true)
                .expect("本地监听套接字设为非阻塞");
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let worker_stop = stop.clone();
            let worker = std::thread::spawn(move || {
                // 非阻塞 accept + 硬超时：线程一定会自行退出，不留悬挂线程或占着端口。
                let deadline = Instant::now() + Duration::from_secs(10);
                while !worker_stop.load(std::sync::atomic::Ordering::Relaxed)
                    && Instant::now() < deadline
                {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // 监听套接字是非阻塞的，而 Linux 上 accept 出来的套接字会
                            // 继承 O_NONBLOCK：那时 read 只会立刻返回 WouldBlock（请求还没
                            // 缓冲齐），write 也可能只吐出半截响应。先改回阻塞模式，
                            // 再用读写超时兜底，线程就不会卡死。
                            let _ = stream.set_nonblocking(false);
                            let half_second = Some(Duration::from_millis(500));
                            let _ = stream.set_read_timeout(half_second);
                            let _ = stream.set_write_timeout(half_second);
                            // 把请求头读完整（到 \r\n\r\n）再回响应：只 read 一次会将对端
                            // 已发出的字节留在接收队列里，收尾 close 时内核可能直接 RST。
                            let mut request = [0u8; 1024];
                            let mut filled = 0usize;
                            while filled < request.len() && Instant::now() < deadline {
                                match stream.read(&mut request[filled..]) {
                                    Ok(0) => break,
                                    Ok(read) => {
                                        filled += read;
                                        if request[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            let header = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                UPDATE_PAYLOAD.len()
                            );
                            let _ = stream.write_all(header.as_bytes());
                            let _ = stream.write_all(UPDATE_PAYLOAD);
                            let _ = stream.flush();
                            // 显式 shutdown 发 FIN：直接 drop 在 Windows 上可能 RST 掉响应体。
                            // 只关**写**半边 —— 保留读半边，别把对端的请求字节变成 RST 理由。
                            let _ = stream.shutdown(std::net::Shutdown::Write);
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}/payload.zip"),
                stop,
                worker: Some(worker),
            }
        }
    }

    impl Drop for LocalPayloadServer {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }

    /// `reqwest` 需要 tokio 的反应堆上下文，这里按生产代码同样的方式建一个当前线程 runtime。
    fn update_test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("构建下载测试用的 tokio runtime")
    }

    #[test]
    fn update_download_rejects_mismatch_before_renaming_part() {
        let dir = tempfile::tempdir().expect("临时下载目录");
        let dest = dir.path().join("payload.zip");
        let part = dest.with_extension("zip.part");
        let server = LocalPayloadServer::start();
        // 形状合法但值不对：失败必须来自"比对不通过"，而不是"清单没给摘要"。
        let wrong_pin = "0".repeat(64);

        let error = update_test_runtime()
            .block_on(download_verified_update_package(
                &server.url,
                &dest,
                &NetworkSettings::default(),
                |_, _| {},
                &wrong_pin,
            ))
            .expect_err("下载内容与 pinned 摘要不一致时必须失败");

        assert!(
            error.contains("不匹配"),
            "失败原因必须是 SHA256 比对未通过（而不是压根没跑到比对），实际：{error}"
        );
        assert!(
            !dest.exists(),
            "校验还没通过，文件却已拿到最终名字，apply 会读到未经固定的包：{dest:?}"
        );
        assert!(
            !part.exists(),
            "校验失败后必须清掉 .part，不留半截下载：{part:?}"
        );
    }

    #[test]
    fn update_download_renames_part_only_after_pinned_match() {
        let dir = tempfile::tempdir().expect("临时下载目录");
        let dest = dir.path().join("payload.zip");
        let part = dest.with_extension("zip.part");
        let server = LocalPayloadServer::start();

        let hash = update_test_runtime()
            .block_on(download_verified_update_package(
                &server.url,
                &dest,
                &NetworkSettings::default(),
                |_, _| {},
                UPDATE_PAYLOAD_SHA256,
            ))
            .expect("流式哈希与 pinned 摘要一致时下载应当成功");

        assert_eq!(
            hash, UPDATE_PAYLOAD_SHA256,
            "返回的自算摘要应与清单固定值一致"
        );
        assert!(
            !part.exists(),
            ".part 应已 rename 成最终文件，而不是残留：{part:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("校验通过后最终文件必须落盘"),
            UPDATE_PAYLOAD,
            "最终文件的内容必须就是下载到的字节"
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
    fn network_settings_normalizes_custom_proxy() {
        assert_eq!(
            NetworkSettings::with_proxy(Some("172.18.88.90:3128".to_owned()))
                .proxy_url
                .as_deref(),
            Some("http://172.18.88.90:3128")
        );
    }

    // ── 代理的逐 URL 判定 ──
    //
    // 反向自检（必须能失败，改完请还原）：把 `route_for_url_with_env_proxy` 里
    // `if is_direct_route_url(url) { ... }` 那一段删掉（= 回环目标重新吃环境代理）
    // → `env_proxy_never_applies_to_loopback_or_literal_ip_targets` 变红。
    //
    // 环境代理的探测结果一律用参数注入：edition 2024 下 `std::env::set_var` 是
    // unsafe，测试又并行跑，改进程环境既不安全也不可靠。

    #[test]
    fn env_proxy_never_applies_to_loopback_or_literal_ip_targets() {
        let env_proxy = "http://proxy.invalid:7890".to_owned();
        let direct = NetworkSettings::default();

        for url in [
            "http://127.0.0.1:4567/payload.zip",
            "http://localhost:4567/payload.zip",
            "http://LOCALHOST:4567/payload.zip",
            "http://[::1]:4567/payload.zip",
            "https://192.168.1.10/api",
            "http://172.18.88.90:3128/probe",
        ] {
            let route = direct.route_for_url_with_env_proxy(url, Some(env_proxy.clone()));
            assert!(
                route.force_direct,
                "{url} 是回环/字面 IP 目标，必须走强制直连"
            );
            assert_eq!(
                route.proxy_url, None,
                "{url} 不该吃到环境代理 {env_proxy}（那会让本地请求被代理吞掉）"
            );
        }

        // 真实域名：保持原有语义，环境代理照旧生效。
        for url in [
            "https://raw.githubusercontent.com/Tydwdh/serial_tool/main/update.json",
            "https://github.com/Tydwdh/serial_tool/releases/download/v1.3.0/a.zip",
        ] {
            let route = direct.route_for_url_with_env_proxy(url, Some(env_proxy.clone()));
            assert!(!route.force_direct, "{url} 是公网域名，不该被改成强制直连");
            assert_eq!(
                route.proxy_url.as_deref(),
                Some(env_proxy.as_str()),
                "{url} 应继续走环境代理"
            );
        }

        // 没有环境代理时，回环目标依然是强制直连（reqwest 自己的探测器也要被关掉）。
        let route = direct.route_for_url_with_env_proxy("http://127.0.0.1:9/x", None);
        assert!(route.force_direct);
        assert_eq!(route.proxy_url, None);

        // 诊断标签不得对着直连请求谎报"环境代理"。
        assert_eq!(direct.route_label(&route), "直连");
        let public = direct
            .route_for_url_with_env_proxy("https://github.com/x/y.zip", Some(env_proxy.clone()));
        assert_eq!(direct.route_label(&public), "环境代理");
        assert_eq!(
            direct.route_label(
                &direct.route_for_url_with_env_proxy("https://github.com/x/y.zip", None)
            ),
            "系统代理或直连"
        );
        let custom = NetworkSettings::with_proxy(Some("10.0.0.1:3128".to_owned()));
        assert_eq!(
            custom.route_label(&custom.route_for_url_with_env_proxy("http://127.0.0.1:9/x", None)),
            "自定义代理"
        );
    }

    #[test]
    fn explicit_proxy_setting_still_applies_to_loopback_targets() {
        // 用户在设置里手填的代理是显式决定，新判定不吞掉它（哪怕目标是本机端口）。
        let settings = NetworkSettings::with_proxy(Some("172.18.88.90:3128".to_owned()));
        let route = settings.route_for_url_with_env_proxy("http://127.0.0.1:9/x", None);
        assert_eq!(route.proxy_url.as_deref(), Some("http://172.18.88.90:3128"));
        assert!(!route.force_direct);
    }

    #[test]
    fn unparsable_url_keeps_the_env_proxy_route() {
        // 判定失败时保持原行为：不去改这条请求的代理路径，报错交给各调用点。
        let route = NetworkSettings::default()
            .route_for_url_with_env_proxy("not a url", Some("http://p:1".to_owned()));
        assert!(!route.force_direct);
        assert_eq!(route.proxy_url.as_deref(), Some("http://p:1"));
        assert!(!is_direct_route_url(""));
        assert!(!is_direct_route_url(
            "https://raw.githubusercontent.com/a/b"
        ));
        assert!(is_direct_route_url("http://127.0.0.1:1/"));
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
            sha256: "0123456789abcdef".repeat(4),
        };
        let json = serde_json::to_string_pretty(&cache).unwrap();
        let parsed: CheckCache = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_check_time, 1719300000000);
        assert_eq!(parsed.latest_version, "0.3.0");
        assert!(parsed.had_update);
        assert!(is_pinned_sha256(&parsed.sha256));

        // 旧缓存文件没有 sha256 字段：必须仍能解析，且空值不被当作 pinned 值，
        // 调用方据此重新拉取 update.json（缓存命中不得放行未固定的下载）。
        let legacy: CheckCache = serde_json::from_str(
            r#"{"last_check_time":1719300000000,"latest_version":"0.3.0","had_update":true}"#,
        )
        .unwrap();
        assert!(legacy.sha256.is_empty());
        assert!(!is_pinned_sha256(&legacy.sha256));
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
            sha256: String::new(),
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
            sha256: String::new(),
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
