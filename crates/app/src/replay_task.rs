use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tool_application::tool_core::LogLevel;
use tool_application::tool_lua_host::{LuaReplayConfig, run_replay_analyzer_with_cancel};

use crate::app::{ReplayAnalyzerJob, ReplayAnalyzerResult, WorkbenchApp};
use crate::state::StatusLevel;

/// 回放 analyzer 后台任务的运行时状态。
pub(crate) struct ReplayAnalyzerState {
    pub(crate) job: Option<ReplayAnalyzerJob>,
    pub(crate) generation: u64,
}

#[allow(clippy::derivable_impls)]
impl Default for ReplayAnalyzerState {
    fn default() -> Self {
        Self {
            job: None,
            generation: 0,
        }
    }
}

impl WorkbenchApp {
    pub(crate) fn launch_replay_analyzer_background(&mut self) {
        self.replay_panel.want_run_analyzers = false;

        if let Some(ref job) = self.replay_analyzer.job
            && !job.handle.as_ref().map(|h| h.is_finished()).unwrap_or(true)
        {
            self.set_status(StatusLevel::Warn, "回放：analyzer 正在运行中，请等待完成");
            return;
        }

        let entries = self.workbench.plugin_manager.replay_analyzer_entries();
        if entries.is_empty() {
            self.replay_panel
                .set_analyzer_error("没有可用的 replay analyzer".to_owned());
            self.set_status(StatusLevel::Error, "回放：没有可用的 replay analyzer");
            return;
        }

        let raw_events = self.replay_panel.manager().raw_serial_events();
        if raw_events.is_empty() {
            self.replay_panel
                .set_analyzer_error("录制文件中没有原始串口事件".to_owned());
            self.set_status(StatusLevel::Error, "回放：录制文件中没有原始串口事件");
            return;
        }

        let total_entries = entries.len();
        let generation = self.replay_analyzer.generation.wrapping_add(1);
        self.replay_analyzer.generation = generation;
        let source_path = self.replay_panel.path.clone();
        self.replay_panel.analyzer_logs.clear();
        self.replay_panel
            .push_analyzer_log(format!("启动 {total_entries} 个 analyzer ..."));
        self.set_status(
            StatusLevel::Info,
            format!("回放：正在运行 {total_entries} 个 analyzer ..."),
        );

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = Arc::clone(&cancel);

        let handle = std::thread::spawn(move || {
            let mut all_derived = Vec::new();
            let mut errors = Vec::new();
            let mut logs = Vec::new();
            let mut succeeded = 0usize;
            let mut failed = 0usize;

            for entry in &entries {
                if cancel_thread.load(Ordering::Relaxed) {
                    logs.push("Analyzer 已取消".to_owned());
                    break;
                }
                let replay_config = match &entry.manifest.replay {
                    Some(cfg) => cfg,
                    None => {
                        failed += 1;
                        continue;
                    }
                };

                let script_path = entry.root.join(&replay_config.main);
                let script = match std::fs::read_to_string(&script_path) {
                    Ok(s) => s,
                    Err(e) => {
                        failed += 1;
                        errors.push(format!("读取 {} 失败: {e}", script_path.display()));
                        continue;
                    }
                };

                let config = LuaReplayConfig {
                    script_name: format!("replay:{}:{}", entry.plugin_id, replay_config.main),
                    plugin_id: entry.plugin_id.clone(),
                    plugin_version: entry.manifest.version.clone(),
                    subscriptions: replay_config.subscriptions.clone(),
                    outputs: replay_config.outputs.clone(),
                    context: serde_json::json!({
                        "id": entry.manifest.id,
                        "name": entry.manifest.name,
                        "version": entry.manifest.version,
                    }),
                    plugin_root: Some(entry.root.clone()),
                };

                match run_replay_analyzer_with_cancel(
                    script,
                    config,
                    &raw_events,
                    Arc::clone(&cancel_thread),
                ) {
                    Ok(output) => {
                        succeeded += 1;
                        logs.push(format!(
                            "analyzer {} 产生了 {} 个事件",
                            entry.plugin_id,
                            output.events.len()
                        ));
                        all_derived.extend(output.events);
                    }
                    Err(e) => {
                        failed += 1;
                        errors.push(format!("analyzer {} 失败: {e}", entry.plugin_id));
                    }
                }
            }

            all_derived.sort_by_key(|e| (e.timestamp_ms, e.id));
            ReplayAnalyzerResult {
                total: total_entries,
                succeeded,
                failed,
                derived_events: all_derived,
                errors,
                logs,
            }
        });

        self.replay_analyzer.job = Some(ReplayAnalyzerJob {
            generation,
            source_path,
            cancel,
            handle: Some(handle),
        });
    }

    pub(crate) fn poll_replay_analyzer_result(&mut self) {
        let Some(mut job) = self.replay_analyzer.job.take() else {
            return;
        };
        if !job.handle.as_ref().map(|h| h.is_finished()).unwrap_or(true) {
            self.replay_analyzer.job = Some(job);
            return;
        }
        // 取出 handle 进行 join（用 take 而非 unwrap，因 ReplayAnalyzerJob 实现了 Drop，
        // 不能 partial move；take 后 Drop 不会重复 join）。handle 理论上始终为 Some
        //（构造时置 Some，仅在此消费路径 take），None 时按 panic 兜底处理。
        let result = match job.handle.take() {
            Some(handle) => handle.join().unwrap_or(ReplayAnalyzerResult {
                total: 0,
                succeeded: 0,
                failed: 1,
                derived_events: vec![],
                errors: vec!["analyzer thread panicked".into()],
                logs: vec![],
            }),
            None => ReplayAnalyzerResult {
                total: 0,
                succeeded: 0,
                failed: 1,
                derived_events: vec![],
                errors: vec!["analyzer handle already consumed".into()],
                logs: vec![],
            },
        };

        // 忽略过期 generation 的结果（用户已重新触发）
        if job.generation < self.replay_analyzer.generation {
            return;
        }
        // 忽略回放文件已改变的结果
        if job.source_path != self.replay_panel.path {
            self.set_status(
                StatusLevel::Warn,
                "回放：忽略过期 analyzer 结果，录制文件已改变",
            );
            return;
        }

        for msg in &result.logs {
            self.log(LogLevel::Info, msg);
            self.replay_panel.push_analyzer_log(msg.clone());
        }

        for err in &result.errors {
            self.replay_panel.push_analyzer_log(format!("ERROR: {err}"));
        }

        if result.derived_events.is_empty() && result.succeeded == 0 {
            let msg = if result.errors.is_empty() {
                "所有 analyzer 运行完成但未生成派生事件".to_owned()
            } else {
                format!(
                    "{} 个 analyzer 全部失败: {}",
                    result.total,
                    result
                        .errors
                        .first()
                        .map(|e| e.as_str())
                        .unwrap_or("未知错误")
                )
            };
            self.replay_panel.set_analyzer_error(msg.clone());
            self.set_status(StatusLevel::Error, format!("回放：{msg}"));
        } else if result.derived_events.is_empty() && result.failed == 0 {
            // 成功运行但 0 输出：降级为 Warn
            let msg = format!(
                "{} 个 analyzer 运行成功但未生成任何派生事件",
                result.succeeded
            );
            self.replay_panel.set_analyzer_warning(msg.clone());
            self.set_status(StatusLevel::Warn, format!("回放：{msg}"));
        } else {
            // 先设缓存，再用 warning 显示提示（不清缓存）
            self.replay_panel
                .set_analyzer_cache(result.derived_events.clone());
            let summary = format!(
                "{} 个派生事件，{} 成功",
                result.derived_events.len(),
                result.succeeded
            );
            if result.failed > 0 {
                let err_detail = result.errors.join("; ");
                self.replay_panel.set_analyzer_warning(format!(
                    "{summary}，{} 失败: {err_detail}",
                    result.failed
                ));
                self.set_status(
                    StatusLevel::Warn,
                    format!("回放：{summary}，{} 失败", result.failed),
                );
            } else {
                self.replay_panel.clear_analyzer_error();
                self.set_status(StatusLevel::Info, format!("回放：{summary}"));
            }
        }
    }
}
