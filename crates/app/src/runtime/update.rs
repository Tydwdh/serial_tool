use crate::app::WorkbenchApp;
use crate::state::CheckResult;
use std::sync::Arc;

impl WorkbenchApp {
    /// 自动更新调度：启动检查、收割结果、启动下载、收割下载、处理重启。
    pub(super) fn tick_update(&mut self) {
        #[cfg(not(target_os = "linux"))]
        self.tick_update_desktop();
    }

    #[cfg(not(target_os = "linux"))]
    fn tick_update_desktop(&mut self) {
        // 从 Arc 读取下载进度
        if let Some(ref progress_arc) = self.update_state.download_progress_arc {
            let raw = progress_arc.load(std::sync::atomic::Ordering::Relaxed);
            self.update_state.download_progress = raw as f32 / 1000.0;
        }

        // 1. 用户点击"更新并重启"
        if self.update_state.want_restart {
            let Some((version, sha256)) = self
                .update_state
                .latest_version
                .as_ref()
                .zip(self.update_state.downloaded_sha256.as_ref())
                .map(|(v, s)| (v.clone(), s.clone()))
            else {
                self.update_state.want_restart = false;
                self.update_state.error = Some("更新信息不完整，请重新下载更新".into());
                return;
            };

            if let Err(e) = self.save_config() {
                log::warn!("save_config failed: {e}")
            };

            if let Err(e) = tool_updater::write_update_manifest(&version, &sha256) {
                log::error!("write_update_manifest failed: {e}");
                self.update_state.want_restart = false;
                self.update_state.error = Some(format!("写入更新标记失败：{e}"));
                return;
            }

            let exe_path = match std::env::current_exe() {
                Ok(path) => path,
                Err(e) => {
                    self.update_state.want_restart = false;
                    self.update_state.error = Some(format!("定位当前程序失败：{e}"));
                    return;
                }
            };

            if let Err(e) = tool_updater::launch_update_helper(&exe_path) {
                log::error!("launch_update_helper failed: {e}");
                self.update_state.want_restart = false;
                self.update_state.error = Some(format!("启动更新助手失败：{e}"));
                return;
            }

            std::process::exit(0);
        }

        // 2. 收割检查线程结果
        if let Some(handle) = self.update_state.check_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(Ok(result)) => {
                        self.update_state.checking = false;
                        if result.cached {
                            self.update_state.latest_version = Some(result.version.clone());
                        } else if result.download_url.is_empty() {
                            self.update_state.latest_version = Some(result.version.clone());
                            log::info!("updater: 已是最新版本");
                        } else {
                            self.update_state.latest_version = Some(result.version.clone());
                            self.update_state.changelog = result.changelog;
                            self.update_state.update_available = true;
                            self.update_state.download_url = Some(result.download_url);
                            self.update_state.error = None;
                            log::info!("updater: 发现新版本 v{}", result.version);
                        }
                    }
                    Ok(Err(e)) => {
                        self.update_state.checking = false;
                        self.update_state.error = Some(e);
                        log::warn!(
                            "updater: 检查更新失败：{}",
                            self.update_state.error.as_deref().unwrap_or("")
                        );
                    }
                    Err(_) => {
                        self.update_state.checking = false;
                        self.update_state.error = Some("检查更新线程异常退出".into());
                    }
                }
            } else {
                self.update_state.check_handle = Some(handle);
            }
        }

        // 3. 收割下载线程结果
        if let Some(handle) = self.update_state.download_handle.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(Ok(actual_sha256)) => {
                        self.update_state.downloading = false;
                        self.update_state.download_progress = 1.0;
                        self.update_state.downloaded = true;
                        self.update_state.downloaded_sha256 = Some(actual_sha256);
                        self.update_state.error = None;
                        log::info!("updater: 更新包下载完成");
                    }
                    Ok(Err(e)) => {
                        self.update_state.downloading = false;
                        self.update_state.error = Some(e);
                        log::warn!(
                            "updater: 下载更新失败：{}",
                            self.update_state.error.as_deref().unwrap_or("")
                        );
                    }
                    Err(_) => {
                        self.update_state.downloading = false;
                        self.update_state.error = Some("下载更新线程异常退出".into());
                    }
                }
            } else {
                self.update_state.download_handle = Some(handle);
            }
        }

        // 4. 首次自动检查（非强制时考虑 24h 缓存）
        if !self.update_state.checking
            && self.update_state.check_handle.is_none()
            && self.update_state.error.is_none()
            && self.update_state.latest_version.is_none()
            && !self.update_state.force_check
        {
            self.start_update_check(false);
        }

        // 5. 用户手动触发检查
        if self.update_state.force_check
            && !self.update_state.checking
            && self.update_state.check_handle.is_none()
        {
            self.start_update_check(true);
        }
    }

    /// 启动后台检查更新线程。
    /// `force` = true 时跳过 24h 缓存。
    fn start_update_check(&mut self, force: bool) {
        self.update_state.checking = true;
        self.update_state.force_check = false;
        self.update_state.error = None;
        let current_version = env!("CARGO_PKG_VERSION").to_owned();
        let network = tool_updater::NetworkSettings::with_proxy(
            (!self.network_proxy_url.trim().is_empty()).then(|| self.network_proxy_url.clone()),
        );

        self.update_state.check_handle = Some(std::thread::spawn(move || {
            // 先检查 24h 缓存（非强制时）
            if !force
                && let Some(cache) = tool_updater::read_check_cache()
                && tool_updater::is_cache_valid(&cache)
            {
                return Ok(CheckResult {
                    version: cache.latest_version.clone(),
                    download_url: String::new(),
                    changelog: Vec::new(),
                    cached: true,
                });
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;

            rt.block_on(async {
                let info = tool_updater::update_info::fetch_update_info_with_network_settings(
                    tool_updater::UPDATE_JSON_URL,
                    &network,
                )
                .await?;

                let had_update =
                    tool_updater::update_info::is_newer_version(&info.version, &current_version);

                // 写入缓存
                if let Err(e) = tool_updater::write_check_cache(&info.version, had_update) {
                    log::warn!("write_check_cache failed: {e}");
                }

                if !had_update {
                    return Ok(CheckResult {
                        version: info.version.clone(),
                        download_url: String::new(),
                        changelog: Vec::new(),
                        cached: false,
                    });
                }

                Ok(CheckResult {
                    version: info.version.clone(),
                    download_url: info.download_url.clone(),
                    changelog: info.changelog.clone(),
                    cached: false,
                })
            })
        }));
    }

    /// 启动后台下载更新线程。
    pub(crate) fn start_update_download(&mut self) {
        let url = match &self.update_state.download_url {
            Some(u) => u.clone(),
            None => {
                self.update_state.error = Some("无下载 URL".into());
                return;
            }
        };

        self.update_state.downloading = true;
        self.update_state.download_progress = 0.0;
        self.update_state.error = None;

        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let progress_clone = progress.clone();
        let network = tool_updater::NetworkSettings::with_proxy(
            (!self.network_proxy_url.trim().is_empty()).then(|| self.network_proxy_url.clone()),
        );

        self.update_state.download_handle = Some(std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;

            rt.block_on(async {
                tool_updater::download_update_with_network_settings(
                    &url,
                    &network,
                    move |downloaded, total| {
                        let pct = if total > 0 {
                            ((downloaded as f64 / total as f64) * 1000.0) as u64
                        } else {
                            0
                        };
                        progress_clone.store(pct, std::sync::atomic::Ordering::Relaxed);
                    },
                )
                .await
            })
        }));

        self.update_state.download_progress_arc = Some(progress);
    }

    /// 用户手动触发检查更新（跳过 24h 缓存）。
    pub(crate) fn force_check_update(&mut self) {
        self.update_state.force_check = true;
        self.update_state.error = None;
        self.update_state.latest_version = None;
        self.update_state.update_available = false;
        self.update_state.changelog.clear();
    }
}
