use crate::app::WorkbenchApp;
use crate::bootstrap::app_dir;
use crate::state::StatusLevel;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tool_core::LogLevel;
use tool_marketplace::{Registry, RegistryPlugin};

/// 市场插件安装后台任务句柄。
///
/// 参考 `ReplayAnalyzerJob`：generation 用于区分并发任务，cancel 当前未使用
/// （下载/解压本身不可中断，但可在 Drop 时 detach 线程）。progress 共享给 UI 轮询。
pub(crate) struct MarketplaceInstallJob {
    pub(crate) plugin_id: String,
    pub(crate) progress: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Drop for MarketplaceInstallJob {
    fn drop(&mut self) {
        // 线程不可中断；直接 detach，不在主线程 busy-wait join。
        // 下载/解压线程受 reqwest 180s 总超时约束，最终会自行结束，不泄漏。
        // 此前曾在 drop 里 sleep 轮询 2s 等 join，但若线程卡在慢下载会阻塞 GUI 主线程，
        // 故改为纯 detach（与正常运行期 tick 把未完成 job 放回的路径不冲突）。
        if let Some(handle) = self.handle.take() {
            std::mem::forget(handle);
        }
    }
}

impl WorkbenchApp {
    /// 每帧把已发现的插件 id 集合回填给市场 UI，用于显示「已安装」标记。
    pub(super) fn sync_marketplace_installed_ids(&mut self) {
        let ids: std::collections::BTreeSet<String> =
            self.plugin_manager.plugin_ids().into_iter().collect();
        self.plugins_panel.set_installed_ids(ids);
    }

    /// 市场调度：每帧回收刷新/安装线程结果，回填 UI 状态。
    pub(super) fn tick_marketplace(&mut self) {
        // ── 回收刷新线程 ──
        if let Some(handle) = self.marketplace_refresh_job.take() {
            if handle.is_finished() {
                match handle.join() {
                    Ok(Ok(registry)) => {
                        self.plugins_panel.set_market_registry(registry);
                        self.set_status(StatusLevel::Info, "市场索引已刷新");
                    }
                    Ok(Err(e)) => {
                        self.plugins_panel.set_market_error(e.clone());
                        self.set_status(StatusLevel::Error, e);
                    }
                    Err(_) => {
                        let msg = "刷新市场索引线程异常退出".to_owned();
                        self.plugins_panel.set_market_error(msg.clone());
                        self.set_status(StatusLevel::Error, msg);
                    }
                }
            } else {
                self.marketplace_refresh_job = Some(handle);
            }
        }

        // ── 回收安装线程 ──
        if let Some(mut job) = self.marketplace_install_job.take() {
            // 实时回填进度（0..1000 → 0.0..1.0）
            let raw = job.progress.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
            self.plugins_panel.set_install_progress(&job.plugin_id, raw);

            if let Some(handle) = job.handle.take() {
                if handle.is_finished() {
                    let id = job.plugin_id.clone();
                    match handle.join() {
                        Ok(Ok(())) => {
                            // 安装成功：重新扫描插件目录，新插件应出现在已安装 tab。
                            self.refresh_plugin_discovery();
                            self.plugins_panel.clear_installing(&id);
                            self.set_status(
                                StatusLevel::Info,
                                format!("插件 {id} 安装成功，可在「已安装」tab 启用"),
                            );
                        }
                        Ok(Err(e)) => {
                            self.plugins_panel.clear_installing(&id);
                            self.set_status(StatusLevel::Error, format!("安装 {id} 失败：{e}"));
                        }
                        Err(_) => {
                            self.plugins_panel.clear_installing(&id);
                            self.set_status(StatusLevel::Error, format!("安装 {id} 线程异常退出"));
                        }
                    }
                } else {
                    // 仍在运行：放回 job，下帧继续轮询。
                    job.handle = Some(handle);
                    self.marketplace_install_job = Some(job);
                    // 安装中持续请求重绘以刷新进度条。
                }
            }
        }
    }

    /// 启动后台刷新市场索引线程。
    pub(crate) fn start_marketplace_refresh(&mut self) {
        if self.marketplace_refresh_job.is_some() {
            return; // 已有刷新在进行
        }
        self.plugins_panel.set_market_refreshing(true);
        let url = self
            .marketplace_url
            .clone()
            .unwrap_or_else(|| tool_marketplace::DEFAULT_REGISTRY_URL.to_owned());

        self.marketplace_refresh_job = Some(std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;
            rt.block_on(async { tool_marketplace::fetch_registry(&url).await })
        }));
    }

    /// 启动后台安装一个市场插件。
    pub(crate) fn start_marketplace_install(&mut self, entry: RegistryPlugin) {
        if self.marketplace_install_job.is_some() {
            self.set_status(StatusLevel::Warn, "已有插件正在安装，请稍候");
            return;
        }
        let id = entry.id.clone();

        // 重装场景：若该插件已启用/运行，先 disable，避免 Windows 下旧文件被占用
        // 导致替换失败，也保证重装后用户重新启用才加载新代码。
        let was_active = matches!(
            self.plugin_manager.plugin_state(&id),
            Some(tool_extension::PluginState::Running)
                | Some(tool_extension::PluginState::Enabled)
                | Some(tool_extension::PluginState::Finished)
        );
        if was_active && let Err(e) = self.plugin_manager.disable(&id) {
            log::warn!("marketplace: 重装前禁用 {id} 失败（继续安装）：{e}");
        }
        // disable 后的动态面板/资源清理由 tick_plugin_lifecycle 经
        // take_recently_disabled() 统一处理，此处不重复。

        self.plugins_panel.mark_installing(&id);

        let install_dir = app_dir().join("plugins");
        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let progress_clone = progress.clone();

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("创建 tokio runtime 失败：{e}"))?;
            rt.block_on(async {
                tool_marketplace::install_plugin(&entry, &install_dir, move |downloaded, total| {
                    let pct = if total > 0 {
                        ((downloaded as f64 / total as f64) * 1000.0) as u64
                    } else {
                        0
                    };
                    progress_clone.store(pct, std::sync::atomic::Ordering::Relaxed);
                })
                .await
            })
        });

        self.marketplace_install_job = Some(MarketplaceInstallJob {
            plugin_id: id,
            progress,
            handle: Some(handle),
        });
    }

    /// 重新扫描插件目录（安装成功后调用）。
    fn refresh_plugin_discovery(&mut self) {
        let plugin_dir = app_dir().join("plugins");
        if let Err(e) = self.plugin_manager.discover_roots([plugin_dir]) {
            self.log(LogLevel::Warn, format!("安装后重新扫描插件失败：{e}"));
        }
    }

    /// 卸载一个已安装插件：disable → 删除/暂存目录 → 重新扫描。
    ///
    /// 流程：
    /// 1. 若插件处于 Running/Enabled，先 disable（释放运行时、发面板移除事件）。
    /// 2. 删除 `plugins/<id>/`。若 Windows 文件被短暂占用导致 remove_dir_all 失败，
    ///    fallback 为同卷 rename 到 `<id>.old.<pid>/`（启动时 retire_old_plugin_dirs 会清）。
    /// 3. discover_roots：refresh 会因目录不存在而移除该插件 record。
    /// 4. 清理该插件关联的动态面板/文件授权（经 recently_disabled 走 tick_plugin_lifecycle）。
    pub(crate) fn uninstall_plugin(&mut self, plugin_id: &str) {
        let plugin_dir = app_dir().join("plugins");

        // 1. 先 disable（若活跃）
        let was_active = matches!(
            self.plugin_manager.plugin_state(plugin_id),
            Some(tool_extension::PluginState::Running)
                | Some(tool_extension::PluginState::Enabled)
                | Some(tool_extension::PluginState::Finished)
                | Some(tool_extension::PluginState::Failed)
        );
        if was_active {
            if let Err(e) = self.plugin_manager.disable(plugin_id) {
                log::warn!("marketplace: 卸载前禁用 {plugin_id} 失败（继续卸载）：{e}");
            }
            // disable 后的动态面板/资源清理由 tick_plugin_lifecycle 经
            // take_recently_disabled() 处理，这里把它入队。
            self.plugins_panel
                .recently_disabled_push(plugin_id.to_owned());
        }

        // 2. 删除插件目录（fallback rename）
        let target = plugin_dir.join(plugin_id);
        if target.exists() {
            match std::fs::remove_dir_all(&target) {
                Ok(()) => {}
                Err(e) => {
                    log::warn!("marketplace: 直接删除 {plugin_id} 失败（{e}），改用 rename 暂存");
                    let old_dir =
                        plugin_dir.join(format!("{plugin_id}.old.{}", std::process::id()));
                    let old_dir = self.ensure_unique_dir(&old_dir);
                    if let Err(e) = std::fs::rename(&target, &old_dir) {
                        self.set_status(
                            StatusLevel::Error,
                            format!(
                                "卸载 {plugin_id} 失败：无法删除或暂存目录（{e}）。请先禁用该插件后重试"
                            ),
                        );
                        return;
                    }
                    // 暂存成功：尽力删，失败留待启动清理。
                    let _ = std::fs::remove_dir_all(&old_dir);
                }
            }
        }

        // 3. 重新扫描：refresh 会移除已不存在的插件 record
        if let Err(e) = self.plugin_manager.discover_roots([plugin_dir]) {
            self.log(LogLevel::Warn, format!("卸载后重新扫描插件失败：{e}"));
        }

        self.set_status(StatusLevel::Info, format!("插件 {plugin_id} 已卸载"));
    }

    /// 生成不冲突的暂存目录名（与 marketplace::ensure_unique_dir 同语义，避免跨 crate 暴露）。
    fn ensure_unique_dir(&self, base: &Path) -> PathBuf {
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
}

/// 占位：保留 Registry 类型引用，便于未来在 app 层缓存。
#[allow(dead_code)]
type _RegistryRef = Registry;
