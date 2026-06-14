use crate::theme;
use egui::{Color32, ComboBox, ProgressBar, RichText, Sense, TextEdit};
use tool_databus::DataBus;
use tool_recorder::{ReplayBlockReason, ReplayManager, ReplayPolicy, ReplayState};

pub struct ReplayPanel {
    manager: ReplayManager,

    pub path: String,

    speed: f64,
    loop_playback: bool,
    step_size: usize,
    message: Option<String>,

    pub want_pick_file: bool,
    pub auto_load: bool,

    /// main.rs 清空终端/日志后开始播放
    pub want_clear_on_play: bool,

    /// Some(pos_ms): main.rs 清空终端/日志后重放到该位置
    pub want_seek_replay: Option<u64>,

    /// Some(steps): main.rs 清空终端/日志后回退指定事件数
    pub want_step_backward: Option<usize>,

    /// main.rs 消费后运行 replay analyzer
    pub want_run_analyzers: bool,

    /// main.rs 消费后取消正在运行的 analyzer
    pub want_cancel_analyzers: bool,

    pub analyzer_busy: bool,
    pub analyzer_logs: Vec<String>,
}

impl ReplayPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            manager: ReplayManager::new(bus.clone()),

            path: "logs/session.jsonl".to_owned(),

            speed: 1.0,
            loop_playback: false,
            step_size: 1,
            message: None,

            want_pick_file: false,
            auto_load: false,

            want_clear_on_play: false,
            want_seek_replay: None,
            want_step_backward: None,
            want_run_analyzers: false,
            want_cancel_analyzers: false,
            analyzer_busy: false,
            analyzer_logs: Vec::new(),
        }
    }

    pub fn try_load(&mut self) {
        match self.manager.load(&self.path) {
            Ok(count) => {
                let effective = self.manager.effective_policy();
                let mut msg = format!("已加载 {count} 个事件");
                if effective == ReplayPolicy::ReparseRaw {
                    msg.push_str(" (需要运行 analyzer)");
                }
                self.message = Some(msg);
                self.manager.set_speed(self.speed);
                // 加载新文件后，标记需要运行 analyzer
                self.want_run_analyzers = self.manager.needs_analyzer();
            }
            Err(error) => {
                self.message = Some(error.to_string());
            }
        }
    }

    /// 执行 seek 重放：调用方需要先清空终端/日志
    pub fn do_seek_replay(&mut self, position_ms: u64) {
        self.manager.seek_with_replay(position_ms);
    }

    /// 两阶段回放 - 阶段 1：只发布 ui.panel.create 事件。
    /// 返回发布的事件数。调用方应在此之后调用 dynamic_panels.ingest() 创建图表面板。
    pub fn do_seek_panel_phase(&mut self, position_ms: u64) -> usize {
        self.manager.seek_panel_phase(position_ms)
    }

    /// 两阶段回放 - 阶段 2：发布剩余事件 + analyzer cache。
    /// 返回发布的事件数。
    pub fn do_seek_data_phase(&mut self, position_ms: u64) -> usize {
        self.manager.seek_data_phase(position_ms)
    }

    /// 执行步进退后：调用方需要先清空终端/日志
    pub fn do_step_backward(&mut self, steps: usize) {
        let steps = steps.max(1);

        if let Some(position_ms) = self.manager.backward_position_by(steps) {
            self.manager.seek_with_replay(position_ms);
        }
    }

    // ── Coordinator accessors ──

    pub fn manager(&self) -> &ReplayManager {
        &self.manager
    }

    pub fn manager_mut(&mut self) -> &mut ReplayManager {
        &mut self.manager
    }

    pub fn set_analyzer_cache(&mut self, events: Vec<tool_core::Event>) {
        self.manager.set_analyzer_cache(events);
        self.want_run_analyzers = false;
    }

    pub fn set_analyzer_error(&mut self, error: String) {
        self.manager.set_analyzer_error(error);
        self.want_run_analyzers = false;
    }

    pub fn set_analyzer_warning(&mut self, warning: String) {
        self.manager.set_analyzer_warning(warning);
    }

    pub fn clear_analyzer_error(&mut self) {
        self.manager.clear_analyzer_error();
    }

    pub fn push_analyzer_log(&mut self, msg: impl Into<String>) {
        self.analyzer_logs.push(msg.into());
        const MAX_LOGS: usize = 200;
        if self.analyzer_logs.len() > MAX_LOGS {
            let excess = self.analyzer_logs.len() - MAX_LOGS;
            self.analyzer_logs.drain(0..excess);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.auto_load {
            self.auto_load = false;
            self.try_load();
        }

        let published = self.manager.tick();
        if published > 0 {
            self.message = Some(format!("回放中 ({published} 事件)"));
        }

        let mut status = self.manager.status();

        if self.loop_playback && status.state == ReplayState::Finished {
            self.want_clear_on_play = true;
            self.manager.stop();
            self.manager.play();
            status = self.manager.status();
        }

        ui.heading("会话回放");

        self.file_controls(ui);
        ui.separator();

        self.policy_controls(ui);
        ui.separator();

        self.playback_controls(ui, &status);

        // 控件可能修改了 manager 状态，这里重新取一次。
        status = self.manager.status();

        self.progress_bar(ui, &status);
        self.status_line(ui, &status);

        if let Some(message) = &self.message {
            ui.label(message);
        }

        ui.collapsing("Replay Analyzer", |ui| {
            if self.analyzer_busy {
                ui.colored_label(theme::BLUE, "Analyzer 正在运行");

                if ui.button("取消").clicked() {
                    self.want_cancel_analyzers = true;
                }
            } else {
                if ui.button("运行 Analyzer").clicked() {
                    self.want_run_analyzers = true;
                }
            }

            let status = self.manager.status();

            if let Some(error) = &status.analyzer_error {
                ui.colored_label(theme::RED, error);
            }

            if let Some(warning) = &status.analyzer_warning {
                ui.colored_label(theme::YELLOW, warning);
            }

            ui.label(format!(
                "Analyzer cache: {} events",
                status.analyzer_cache_entries
            ));

            egui::ScrollArea::vertical()
                .max_height(120.0)
                .show(ui, |ui| {
                    for line in &self.analyzer_logs {
                        ui.monospace(line);
                    }
                });
        });
    }

    fn file_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("文件");

            ui.add(
                TextEdit::singleline(&mut self.path)
                    .desired_width(280.0)
                    .hint_text("选择 JSONL 回放文件"),
            );

            if ui.button("浏览").clicked() {
                self.want_pick_file = true;
            }

            if ui.button("加载").clicked() {
                self.try_load();
            }
        });
    }

    fn policy_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("回放策略");

            let mut policy = self.manager.policy();
            let policy_changed = ComboBox::from_id_salt("replay-policy")
                .width(160.0)
                .selected_text(policy_label(policy))
                .show_ui(ui, |ui| {
                    let mut changed = false;
                    for &p in &[
                        ReplayPolicy::AutoPreferRecorded,
                        ReplayPolicy::ExactRecorded,
                        ReplayPolicy::ReparseRaw,
                    ] {
                        if ui
                            .selectable_value(&mut policy, p, policy_label(p))
                            .changed()
                        {
                            changed = true;
                        }
                    }
                    changed
                });

            if policy_changed.inner.unwrap_or(false) {
                self.manager.set_policy(policy);
                self.want_run_analyzers = self.manager.needs_analyzer();
            }
        });

        // 显示实际生效的策略
        let effective = self.manager.effective_policy();
        let status_text = match effective {
            ReplayPolicy::AutoPreferRecorded => "自动 (使用录制解析结果)",
            ReplayPolicy::ExactRecorded => "使用录制解析结果",
            ReplayPolicy::ReparseRaw => {
                if self.manager.analyzer_cache_valid() {
                    "使用 Replay Analyzer 重新解析 (已缓存)"
                } else if self.manager.analyzer_error().is_some() {
                    "Replay Analyzer 失败"
                } else {
                    "需要运行 Replay Analyzer"
                }
            }
        };
        ui.label(RichText::new(status_text).color(theme::TEXT_SECONDARY));

        if let Some(error) = self.manager.analyzer_error() {
            ui.colored_label(theme::RED, format!("错误: {error}"));
        }
        if let Some(warning) = self.manager.analyzer_warning() {
            ui.colored_label(theme::YELLOW, format!("{warning}"));
        }
    }

    fn playback_controls(&mut self, ui: &mut egui::Ui, status: &tool_recorder::ReplayStatus) {
        let has_events = status.total_events > 0;
        let can_play = self.manager.can_play();
        let can_seek = self.manager.can_seek();
        let can_control = has_events && status.state != ReplayState::Empty;
        let block_reason = self.manager.replay_block_reason();

        ui.horizontal_wrapped(|ui| {
            if status.state == ReplayState::Playing {
                if ui.button("⏸ 暂停").clicked() {
                    self.manager.pause();
                }
            } else {
                let label = match status.state {
                    ReplayState::Finished => "↻ 重播",
                    _ => "▶ 播放",
                };

                if ui
                    .add_enabled(can_play, egui::Button::new(label))
                    .on_disabled_hover_text("当前回放策略需要先完成 Replay Analyzer")
                    .clicked()
                {
                    match status.state {
                        ReplayState::Finished => {
                            self.want_clear_on_play = true;
                            self.manager.stop();
                            self.manager.play();
                        }
                        _ => {
                            if status.position_ms == 0 {
                                self.want_clear_on_play = true;
                            }
                            self.manager.play();
                        }
                    }
                }
            }

            if ui
                .add_enabled(can_control, egui::Button::new("⏹ 停止"))
                .clicked()
            {
                self.manager.stop();
                self.message = Some("已停止".to_owned());
            }

            ui.separator();

            ui.label("步进");

            egui::ComboBox::from_id_salt("step-size")
                .width(56.0)
                .selected_text(format!("{}", self.step_size))
                .show_ui(ui, |ui| {
                    for &n in &[1, 5, 10, 50, 100] {
                        ui.selectable_value(&mut self.step_size, n, format!("{n}"));
                    }
                });

            if ui
                .add_enabled(can_seek && status.cursor > 0, egui::Button::new("◀"))
                .on_hover_text("后退指定事件数")
                .clicked()
            {
                self.want_step_backward = Some(self.step_size);
            }

            if ui
                .add_enabled(
                    can_seek && status.cursor < status.total_events,
                    egui::Button::new("▶"),
                )
                .on_hover_text("前进指定事件数")
                .clicked()
            {
                for _ in 0..self.step_size {
                    self.manager.step_forward();

                    let current = self.manager.status();
                    if current.cursor >= current.total_events {
                        break;
                    }
                }
            }

            ui.label(format!("{}/{}", status.cursor, status.total_events));

            ui.separator();

            ui.checkbox(&mut self.loop_playback, "循环");

            ui.separator();

            ui.label("速度");

            let mut speed_log = (self.speed.ln() / 2_f64.ln()).clamp(-3.0, 4.0);

            let speed_resp = ui.add(
                egui::Slider::new(&mut speed_log, -3.0..=4.0)
                    .text(format!("{:.2}x", self.speed))
                    .step_by(0.01),
            );

            if speed_resp.changed() {
                let new_speed = (2_f64.powf(speed_log) * 100.0).round() / 100.0;
                self.speed = new_speed.clamp(0.1, 16.0);
                self.manager.set_speed(self.speed);
            }

            if ui
                .small_button("1x")
                .on_hover_text("重置为 1 倍速")
                .clicked()
            {
                self.speed = 1.0;
                self.manager.set_speed(1.0);
            }

            speed_resp.on_hover_text(format!("回放速度 {:.2}x  |  范围 0.1x ~ 16x", self.speed));
        });

        if let Some(reason) = block_reason {
            ui.separator();

            match reason {
                ReplayBlockReason::NeedAnalyzer => {
                    ui.colored_label(
                        theme::YELLOW,
                        "重新解析模式需要先运行 Replay Analyzer",
                    );

                    if ui.button("运行 Analyzer").clicked() {
                        self.want_run_analyzers = true;
                    }
                }

                ReplayBlockReason::AnalyzerFailed(ref error) => {
                    ui.colored_label(theme::RED, format!("Analyzer 失败：{error}"));

                    if ui.button("重试 Analyzer").clicked() {
                        self.want_run_analyzers = true;
                    }

                    if ui.button("切换到精确回放").clicked() {
                        self.manager.set_policy(ReplayPolicy::ExactRecorded);
                    }
                }
            }
        }
    }

    fn progress_bar(&mut self, ui: &mut egui::Ui, status: &tool_recorder::ReplayStatus) {
        let progress = if status.duration_ms == 0 {
            0.0
        } else {
            status.position_ms as f32 / status.duration_ms as f32
        };

        let bar_text = format!(
            "{} / {}",
            ms_to_hms(status.position_ms),
            ms_to_hms(status.duration_ms),
        );

        let bar = ProgressBar::new(progress.clamp(0.0, 1.0)).text(bar_text);
        let bar_resp = ui.add(bar);

        if status.total_events == 0 || status.duration_ms == 0 {
            return;
        }

        if !self.manager.can_seek() {
            bar_resp.on_hover_text("当前回放策略需要先完成 Replay Analyzer");
            return;
        }

        let drag = ui.interact(
            bar_resp.rect,
            ui.make_persistent_id("replay-progress-drag"),
            Sense::click_and_drag(),
        );

        if (drag.clicked() || drag.dragged())
            && let Some(mpos) = ui.ctx().pointer_latest_pos()
        {
            let click_frac =
                ((mpos.x - bar_resp.rect.left()) / bar_resp.rect.width()).clamp(0.0, 1.0);

            let target = (click_frac * status.duration_ms as f32) as u64;

            self.want_seek_replay = Some(target);
        }
    }

    fn status_line(&mut self, ui: &mut egui::Ui, status: &tool_recorder::ReplayStatus) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(state_label(status.state)).color(state_color(status.state)));

            ui.separator();

            ui.label(format!("事件 {}/{}", status.cursor, status.total_events));

            ui.separator();

            ui.label(format!("{:.1}x", status.speed));

            if let Some(path) = &status.path {
                ui.separator();

                ui.monospace(
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.display().to_string()),
                );
            }
        });

        // 加载报告：坏行警告
        if let Some(report) = status.load_report.as_ref() {
            if report.skipped > 0 {
                ui.colored_label(
                    theme::YELLOW,
                    format!("加载 {} 条，跳过 {} 条坏行", report.loaded, report.skipped),
                );
                if let Some(first) = report.first_errors.first() {
                    ui.colored_label(theme::TEXT_SECONDARY, first);
                }
            }
        }
    }
}

fn state_label(state: ReplayState) -> &'static str {
    match state {
        ReplayState::Empty => "空",
        ReplayState::Loaded => "就绪",
        ReplayState::Playing => "播放中",
        ReplayState::Paused => "已暂停",
        ReplayState::Finished => "已完成",
    }
}

fn ms_to_hms(ms: u64) -> String {
    let total_s = ms / 1000;
    let h = total_s / 3600;
    let m = (total_s % 3600) / 60;
    let s = total_s % 60;
    let ms_part = ms % 1000;

    if h > 0 {
        format!("{h}:{m:02}:{s:02}.{ms_part:03}")
    } else {
        format!("{m}:{s:02}.{ms_part:03}")
    }
}

fn policy_label(policy: ReplayPolicy) -> &'static str {
    match policy {
        ReplayPolicy::AutoPreferRecorded => "自动",
        ReplayPolicy::ExactRecorded => "精确回放",
        ReplayPolicy::ReparseRaw => "重新解析",
    }
}

fn state_color(state: ReplayState) -> Color32 {
    match state {
        ReplayState::Empty => theme::TEXT_SECONDARY,
        ReplayState::Loaded | ReplayState::Paused => theme::YELLOW,
        ReplayState::Playing => theme::BLUE,
        ReplayState::Finished => theme::GREEN,
    }
}
