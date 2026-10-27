//! 浏览器回放域：JSONL 回放文件的选取与提交，以及 Web Replay Analyzer 的
//! 分批调度（按插件排队、逐批喂给 Lua 回调、把派生事件写回 analyzer 缓存）。
//!
//! `replay_policy_view_option` / `replay_policy_option_view` 两个 DTO 映射仍留在
//! `web/mod.rs`：它们唯一的调用方是设置的加载与持久化。

use super::{
    WebReplayAnalyzerState, web_file_input, web_file_input_from_event, web_open_file_picker,
};
use crate::web_plugin_host::event_to_plugin_value;
use crate::workbench_app::WorkbenchApp;
use eframe::egui;
use std::collections::BTreeMap;
use tool_application::AppCommand;
use tool_application::replay::{ReplayPolicyView, ReplayStateView, ReplayStatusView};
use tool_application::web::WebRuntime;
use tool_core::{Direction, Event, Payload, topic_matches};
use tool_panels::ReplayUiCommand;
use tool_plugin_api::PluginValue;
use tool_plugin_runtime::WebReplayOutput;
use wasm_bindgen_futures::JsFuture;

impl WorkbenchApp {
    fn request_web_replay_file(&mut self, _ctx: &egui::Context) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            self.replay_panel.message = Some("浏览器文档不可用".to_owned());
            return;
        };
        let Some(input) = web_file_input(&document) else {
            self.replay_panel.message = Some("无法创建浏览器文件选择器".to_owned());
            return;
        };
        input.set_accept(".jsonl,.ndjson,application/json");
        let Some(runtime) = self.runtime.clone() else {
            self.replay_panel.message = Some("当前浏览器没有可用的异步任务运行时".to_owned());
            return;
        };
        let opened = web_open_file_picker(&input, move |event: web_sys::Event| {
            let Some(input) = web_file_input_from_event(&event) else {
                return;
            };
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            let name = file.name();
            let future = async move {
                JsFuture::from(file.text())
                    .await
                    .map_err(|error| format!("读取回放文件失败：{error:?}"))
                    .and_then(|value| {
                        value
                            .as_string()
                            .ok_or_else(|| "回放文件不是有效文本".to_owned())
                    })
            };
            let _ = runtime.load_text("replay_load", name, future);
        });
        if !opened {
            self.replay_panel.message = Some("无法监听文件选择事件".to_owned());
        }
    }

    fn cancel_web_replay_analyzer(&mut self, reason: &str) {
        if !self.replay_analyzer.running {
            return;
        }
        if let Some(task_id) = self.replay_analyzer.task_id.take()
            && let Some(runtime) = self.runtime.as_ref()
        {
            runtime.cancel_task(task_id);
        }
        self.replay_analyzer.running = false;
        self.replay_analyzer.error = Some(reason.to_owned());
        self.replay_panel.analyzer_busy = false;
        self.replay_panel.message = Some(format!("Web Replay Analyzer 已取消：{reason}"));
    }

    fn start_web_replay_analyzer(&mut self, ctx: &egui::Context) {
        let input_events = self
            .runtime
            .as_ref()
            .map(WebRuntime::replay_raw_serial_events)
            .unwrap_or_default();
        if input_events.is_empty() {
            self.replay_panel.message = Some("请先选择并加载 JSONL 回放文件".to_owned());
            return;
        }
        if self.replay_analyzer.running {
            return;
        }
        let plugin_indices = self
            .plugins
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.persisted.enabled
                    && record.replay_instance.is_some()
                    && record.persisted.manifest.replay.is_some()
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if plugin_indices.is_empty() {
            self.replay_analyzer.error = Some(
                "没有可用的 Web Replay Analyzer；请安装并启用包含 replay 配置的 Web 插件。"
                    .to_owned(),
            );
            self.replay_panel.message = self.replay_analyzer.error.clone();
            return;
        }

        let task_id = self.runtime.as_ref().map(|runtime| {
            runtime.begin_task(
                tool_application::TaskKind::ReplayAnalyzer,
                "正在启动 Web Replay Analyzer",
            )
        });
        self.replay_analyzer = WebReplayAnalyzerState {
            running: true,
            task_id,
            plugin_indices,
            input_events,
            next_plugin: 0,
            input_indices: Vec::new(),
            input_offset: 0,
            derived_events: Vec::new(),
            logs: Vec::new(),
            error: None,
        };
        self.replay_panel.analyzer_busy = true;
        self.replay_panel.analyzer_logs.clear();
        self.replay_panel.message = Some("正在运行 Web Replay Analyzer…".to_owned());
        self.send_next_web_replay_analyzer(ctx);
    }

    /// analyzer 当前要处理的插件下标；队列走完时返回 None。
    /// 推进插件与推进批次两个阶段必须共用这个下标来源，否则它们会各自跳过不同的插件。
    fn current_web_replay_analyzer_plugin(&self) -> Option<usize> {
        let analyzer = &self.replay_analyzer;
        analyzer.plugin_indices.get(analyzer.next_plugin).copied()
    }

    /// analyzer 失败退出：只记录错误文案，收尾（写回面板、结束任务）仍由
    /// `finish_web_replay_analyzer` 统一完成。
    fn fail_web_replay_analyzer(&mut self, error: String, ctx: &egui::Context) {
        self.replay_analyzer.error = Some(error);
        self.finish_web_replay_analyzer(ctx);
    }

    fn send_next_web_replay_analyzer(&mut self, ctx: &egui::Context) {
        let Some(index) = self.current_web_replay_analyzer_plugin() else {
            self.finish_web_replay_analyzer(ctx);
            return;
        };
        let Some(record) = self.plugins.records.get(index) else {
            self.fail_web_replay_analyzer("Web Replay Analyzer 插件已不存在".to_owned(), ctx);
            return;
        };
        let Some(replay_instance) = record.replay_instance else {
            let error = format!(
                "Web Replay Analyzer {} 尚未完成加载",
                record.persisted.manifest.name
            );
            self.fail_web_replay_analyzer(error, ctx);
            return;
        };
        let Some(replay) = record.persisted.manifest.replay.as_ref() else {
            self.replay_analyzer.next_plugin += 1;
            self.send_next_web_replay_analyzer(ctx);
            return;
        };

        let input_indices = self
            .replay_analyzer
            .input_events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                replay.subscriptions.is_empty()
                    || replay
                        .subscriptions
                        .iter()
                        .any(|pattern| topic_matches(pattern, &event.topic))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let session = serde_json::json!({
            "start_ms": self
                .replay_analyzer
                .input_events
                .first()
                .map(|event| event.timestamp_ms)
                .unwrap_or(0),
            "end_ms": self
                .replay_analyzer
                .input_events
                .last()
                .map(|event| event.timestamp_ms)
                .unwrap_or(0),
            "event_count": self.replay_analyzer.input_events.len(),
        });
        self.replay_analyzer.input_indices = input_indices;
        self.replay_analyzer.input_offset = 0;
        if let Err(error) = self
            .web_lua
            .replay_begin(replay_instance, PluginValue::from_json(&session))
        {
            self.fail_web_replay_analyzer(error.to_string(), ctx);
            return;
        }
        self.replay_panel.message = Some(format!(
            "正在运行 Web Replay Analyzer：{}（{} 个输入事件）",
            record.persisted.manifest.name,
            self.replay_analyzer.input_indices.len()
        ));
        self.send_next_web_replay_analyzer_batch(ctx);
    }

    fn send_next_web_replay_analyzer_batch(&mut self, ctx: &egui::Context) {
        const ANALYZER_BATCH: usize = 256;
        let Some(index) = self.current_web_replay_analyzer_plugin() else {
            return;
        };
        let Some(replay_instance) = self
            .plugins
            .records
            .get(index)
            .and_then(|record| record.replay_instance)
        else {
            self.fail_web_replay_analyzer("Web Replay Analyzer Lua 实例已退出".to_owned(), ctx);
            return;
        };
        let start = self.replay_analyzer.input_offset;
        let end = (start + ANALYZER_BATCH).min(self.replay_analyzer.input_indices.len());
        if start < end {
            let events = self.replay_analyzer.input_indices[start..end]
                .iter()
                .filter_map(|index| self.replay_analyzer.input_events.get(*index))
                .cloned()
                .collect::<Vec<_>>();
            self.replay_analyzer.input_offset = end;
            for event in events {
                if let Err(error) = self
                    .web_lua
                    .replay_event(replay_instance, event_to_plugin_value(&event))
                {
                    self.fail_web_replay_analyzer(error.to_string(), ctx);
                    return;
                }
            }
            self.replay_panel.message = Some(format!(
                "Web Replay Analyzer：处理 {}/{} 个输入事件",
                end,
                self.replay_analyzer.input_indices.len()
            ));
        } else {
            match self.web_lua.replay_end(replay_instance) {
                Ok(WebReplayOutput { events, logs }) => {
                    self.replay_analyzer.logs.extend(logs);
                    for value in events {
                        match plugin_event_to_core_event(value) {
                            Ok(event) => self.replay_analyzer.derived_events.push(event),
                            Err(error) => self.replay_analyzer.logs.push(error),
                        }
                    }
                    self.finish_web_replay_analyzer_plugin(index, ctx);
                    return;
                }
                Err(error) => {
                    self.fail_web_replay_analyzer(error.to_string(), ctx);
                    return;
                }
            }
        }
        ctx.request_repaint();
    }

    fn finish_web_replay_analyzer_plugin(&mut self, index: usize, ctx: &egui::Context) {
        if !self.replay_analyzer.running
            || self
                .replay_analyzer
                .plugin_indices
                .get(self.replay_analyzer.next_plugin)
                != Some(&index)
        {
            return;
        }
        self.replay_analyzer.next_plugin += 1;
        self.send_next_web_replay_analyzer(ctx);
    }

    fn finish_web_replay_analyzer(&mut self, ctx: &egui::Context) {
        if !self.replay_analyzer.running {
            return;
        }
        self.replay_analyzer.running = false;
        self.replay_panel.analyzer_busy = false;
        self.mirror_web_replay_analyzer_logs();
        if let Some(runtime) = self.runtime.as_ref() {
            if let Some(error) = self.replay_analyzer.error.clone() {
                runtime.replay_set_analyzer_error(error);
            } else {
                runtime.replay_set_analyzer_cache(self.replay_analyzer.derived_events.clone());
            }
        }
        let derived = self.replay_analyzer.derived_events.len();
        let message = match self.replay_analyzer.error.as_deref() {
            Some(error) => format!("Web Replay Analyzer 失败：{error}"),
            None => format!("Web Replay Analyzer 完成：生成 {derived} 个派生事件"),
        };
        if let Some(task_id) = self.replay_analyzer.task_id.take()
            && let Some(runtime) = self.runtime.as_ref()
        {
            if self.replay_analyzer.error.is_some() {
                runtime.fail_task(task_id, message.clone());
            } else {
                runtime.complete_task(task_id, message.clone());
            }
        }
        self.replay_panel.message = Some(message);
        ctx.request_repaint();
    }

    /// 把 analyzer 日志镜像进共享面板：只保留最近 200 条。
    /// 先 `rev()` 再 `take(200)` 再 `rev()`，是为了截取尾部同时保持时间顺序。
    fn mirror_web_replay_analyzer_logs(&mut self) {
        self.replay_panel.analyzer_logs.clear();
        for line in self.replay_analyzer.logs.iter().rev().take(200).rev() {
            self.replay_panel.push_analyzer_log(line.clone());
        }
    }

    pub(crate) fn web_replay_status(&self) -> ReplayStatusView {
        self.runtime
            .as_ref()
            .map(WebRuntime::query_replay)
            .unwrap_or_else(empty_replay_status)
    }

    pub(crate) fn web_replay_panel_ui(&mut self, ui: &mut egui::Ui) {
        let status = self.web_replay_status();
        if let Some(path) = status.path.as_deref() {
            self.replay_panel.path = path.to_owned();
        }
        // The analyzer executes in Web Workers, but its lifecycle is owned by
        // the composition root. Mirror only its presentation DTO into the
        // shared panel so replay controls remain identical to Native.
        self.replay_panel.analyzer_busy = self.replay_analyzer.running;
        self.mirror_web_replay_analyzer_logs();
        self.replay_panel.ui(ui, &status);

        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            self.request_web_replay_file(ui.ctx());
        }
        if self.replay_panel.want_run_analyzers {
            self.replay_panel.want_run_analyzers = false;
            self.start_web_replay_analyzer(ui.ctx());
        }
        if self.replay_panel.want_cancel_analyzers {
            self.replay_panel.want_cancel_analyzers = false;
            self.cancel_web_replay_analyzer("用户取消");
        }
        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.clear_web_replay_views();
        }
        if let Some(position_ms) = self.replay_panel.want_seek_replay.take() {
            self.dispatch_web_replay(AppCommand::ReplaySeek { position_ms }, true);
        }
        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.dispatch_web_replay_step_backward(steps);
        }

        for command in self.replay_panel.take_commands() {
            match command {
                ReplayUiCommand::PickFile => {
                    self.request_web_replay_file(ui.ctx());
                }
                ReplayUiCommand::Load { .. } => {
                    self.replay_panel.message =
                        Some("Web 回放请使用“浏览”选择本地 JSONL 文件".to_owned());
                }
                ReplayUiCommand::Play => {
                    self.dispatch_web_replay(AppCommand::ReplayPlay, false);
                }
                ReplayUiCommand::Pause => {
                    self.dispatch_web_replay(AppCommand::ReplayPause, false);
                }
                ReplayUiCommand::Stop => {
                    self.dispatch_web_replay(AppCommand::ReplayStop, true);
                }
                ReplayUiCommand::Seek { position_ms }
                | ReplayUiCommand::SeekPanelPhase { position_ms }
                | ReplayUiCommand::SeekDataPhase { position_ms } => {
                    self.dispatch_web_replay(AppCommand::ReplaySeek { position_ms }, true);
                }
                ReplayUiCommand::SeekCursorPanelPhase { target_cursor }
                | ReplayUiCommand::SeekCursorDataPhase { target_cursor } => {
                    let current = self.web_replay_status().cursor;
                    let delta = target_cursor as i64 - current as i64;
                    if delta != 0 {
                        self.dispatch_web_replay(
                            AppCommand::ReplayStep {
                                delta: delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                            },
                            true,
                        );
                    }
                }
                ReplayUiCommand::StepBackward { steps } => {
                    self.dispatch_web_replay_step_backward(steps);
                }
                ReplayUiCommand::SetSpeed(speed) => {
                    self.dispatch_web_replay(AppCommand::SetReplaySpeed { speed }, false);
                }
                ReplayUiCommand::SetPolicy(policy) => {
                    self.dispatch_web_replay(AppCommand::SetReplayPolicy { policy }, true);
                    self.persist_settings();
                }
                ReplayUiCommand::AddReplayBookmark { name } => {
                    self.dispatch_web_replay(AppCommand::AddReplayBookmark { name }, false);
                }
                ReplayUiCommand::RemoveReplayBookmark { position_ms } => {
                    self.dispatch_web_replay(
                        AppCommand::RemoveReplayBookmark { position_ms },
                        false,
                    );
                }
                ReplayUiCommand::SetLoop(value) => {
                    self.replay_panel.loop_playback = value;
                }
                ReplayUiCommand::SetStepSize(_) => {}
                ReplayUiCommand::SetAnalyzerCache(_)
                | ReplayUiCommand::SetAnalyzerError(_)
                | ReplayUiCommand::SetAnalyzerWarning(_)
                | ReplayUiCommand::ClearAnalyzerError
                | ReplayUiCommand::PushAnalyzerLog(_) => {}
            }
        }
        let status = self.web_replay_status();
        if self.replay_panel.loop_playback && status.state == ReplayStateView::Finished {
            self.clear_web_replay_views();
            self.dispatch_web_replay(AppCommand::ReplayStop, false);
            self.dispatch_web_replay(AppCommand::ReplayPlay, false);
        }
    }

    fn clear_web_replay_views(&mut self) {
        self.terminal_panel.clear();
        self.bottom_log_panel.clear();
        self.dynamic_panels.clear_charts();
    }

    pub(crate) fn dispatch_web_replay(&mut self, command: AppCommand, rebuild: bool) {
        if rebuild {
            self.clear_web_replay_views();
        }
        let Some(runtime) = self.runtime.as_ref() else {
            self.replay_panel.message = Some("Application 运行时不可用".to_owned());
            return;
        };
        if let Err(error) = runtime.dispatch(command) {
            self.replay_panel.message = Some(format!("回放操作失败：{error}"));
            self.serial.borrow_mut().status = format!("回放操作失败：{error}");
        }
    }

    /// 后退 N 步。面板给的是 `usize`，而 `ReplayStep::delta` 是 `i32`：
    /// 先夹到 `i32::MAX` 再取负，否则巨大步数会溢出成反向的正 delta。
    fn dispatch_web_replay_step_backward(&mut self, steps: usize) {
        let delta = -(steps.min(i32::MAX as usize) as i32);
        self.dispatch_web_replay(AppCommand::ReplayStep { delta }, true);
    }
}

fn empty_replay_status() -> ReplayStatusView {
    ReplayStatusView {
        state: ReplayStateView::Empty,
        path: None,
        total_events: 0,
        cursor: 0,
        speed: 1.0,
        position_ms: 0,
        duration_ms: 0,
        policy: ReplayPolicyView::AutoPreferRecorded,
        effective_policy: ReplayPolicyView::AutoPreferRecorded,
        has_recorded_protocol: false,
        analyzer_cache_entries: 0,
        analyzer_cache_valid: false,
        analyzer_error: None,
        analyzer_warning: None,
        can_play: false,
        can_seek: false,
        block_reason: None,
        bookmarks: Vec::new(),
        load_report: None,
    }
}

fn plugin_event_to_core_event(value: PluginValue) -> Result<Event, String> {
    let PluginValue::Object(mut object) = value else {
        return Err("Web Replay Analyzer 输出不是事件对象".to_owned());
    };
    let timestamp_ms = match object.remove("timestamp_ms") {
        Some(PluginValue::Integer(value)) if value >= 0 => value as u64,
        _ => return Err("Web Replay Analyzer 输出缺少有效 timestamp_ms".to_owned()),
    };
    let topic = match object.remove("topic") {
        Some(PluginValue::String(value)) if !value.is_empty() => value,
        _ => return Err("Web Replay Analyzer 输出缺少 topic".to_owned()),
    };
    let source = match object.remove("source") {
        Some(PluginValue::String(value)) => value,
        _ => "replay-analyzer:web".to_owned(),
    };
    let payload = match object.remove("payload").unwrap_or(PluginValue::Null) {
        PluginValue::Null => Payload::Empty,
        PluginValue::String(value) => Payload::Text(value),
        value => Payload::Json(
            value
                .to_json()
                .map_err(|error| format!("Web Replay Analyzer payload 无效：{error}"))?,
        ),
    };
    let metadata = object
        .remove("metadata")
        .unwrap_or(PluginValue::Object(BTreeMap::new()))
        .to_json()
        .map_err(|error| format!("Web Replay Analyzer metadata 无效：{error}"))?;
    Ok(Event {
        id: 0,
        timestamp_ms,
        topic,
        source,
        direction: Direction::Internal,
        payload,
        metadata,
    })
}
