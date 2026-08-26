use std::collections::VecDeque;

use crate::{ReplayPolicyOption, ReplayUiCommand, design, replay_policy_ui, theme};
use egui::{Color32, ProgressBar, RichText, Sense, TextEdit};
use egui_material_icons::icons::{
    ICON_FOLDER_OPEN, ICON_PAUSE, ICON_PLAY_ARROW, ICON_REPLAY, ICON_SKIP_NEXT, ICON_SKIP_PREVIOUS,
    ICON_STOP, ICON_TUNE, ICON_UPLOAD_FILE,
};
use tool_application::replay::{
    ReplayBlockReasonView, ReplayPolicyView, ReplayStateView, ReplayStatusView,
};

/// Native/Web 共用的回放展示状态。
///
/// 回放数据和控制器归 Application 所有；Panel 只保存交互状态，并把用户
/// 意图放入 `ReplayUiCommand` 队列。这样 UI 不会再持有第二个 ReplayManager。
pub struct ReplayPanel {
    pub path: String,
    pub speed: f64,
    pub loop_playback: bool,
    pub step_size: usize,
    pub message: Option<String>,

    pub want_pick_file: bool,
    pub auto_load: bool,
    pub want_clear_on_play: bool,
    pub want_seek_replay: Option<u64>,
    pub want_step_backward: Option<usize>,
    pub want_run_analyzers: bool,
    pub want_cancel_analyzers: bool,
    pub analyzer_busy: bool,
    pub analyzer_logs: VecDeque<String>,

    load_pending: bool,
    commands: Vec<ReplayUiCommand>,
}

impl ReplayPanel {
    pub fn new() -> Self {
        Self {
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
            analyzer_logs: VecDeque::new(),
            load_pending: false,
            commands: Vec::new(),
        }
    }

    pub fn set_load_pending(&mut self, pending: bool) {
        self.load_pending = pending;
    }

    pub fn take_commands(&mut self) -> Vec<ReplayUiCommand> {
        std::mem::take(&mut self.commands)
    }

    pub fn set_analyzer_error(&mut self, error: String) {
        self.want_run_analyzers = false;
        self.message = Some(error);
    }

    pub fn set_analyzer_warning(&mut self, warning: String) {
        self.message = Some(warning);
    }

    pub fn clear_analyzer_error(&mut self) {
        self.message = None;
    }

    pub fn push_analyzer_log(&mut self, msg: impl Into<String>) {
        self.analyzer_logs.push_back(msg.into());
        const MAX_LOGS: usize = 200;
        while self.analyzer_logs.len() > MAX_LOGS {
            self.analyzer_logs.pop_front();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, status: &ReplayStatusView) {
        if self.auto_load {
            self.auto_load = false;
            self.commands.push(ReplayUiCommand::Load {
                path: self.path.clone(),
            });
        }

        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_FOLDER_OPEN, "回放文件");
            ui.separator();
            self.file_controls(ui);
        });

        ui.add_space(8.0);

        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_TUNE, "回放策略");
            ui.separator();
            self.policy_controls(ui, status);
        });

        ui.add_space(8.0);

        design::card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            design::section_header(ui, ICON_REPLAY, "播放控制");
            ui.separator();
            self.playback_controls(ui, status);
            self.progress_bar(ui, status);
            self.bookmark_controls(ui, status);
        });

        ui.add_space(4.0);
        self.status_line(ui, status);

        if let Some(message) = &self.message {
            ui.label(message);
        }

        if status.effective_policy == ReplayPolicyView::ReparseRaw
            || self.analyzer_busy
            || status.analyzer_cache_valid
        {
            ui.collapsing("Replay Analyzer", |ui| {
                if self.analyzer_busy {
                    ui.colored_label(theme::blue(), "Analyzer 正在运行");
                    if ui.button("取消").clicked() {
                        self.want_cancel_analyzers = true;
                    }
                } else if ui.button("运行 Analyzer").clicked() {
                    self.want_run_analyzers = true;
                }

                if let Some(error) = &status.analyzer_error {
                    ui.colored_label(theme::red(), error);
                }
                if let Some(warning) = &status.analyzer_warning {
                    ui.colored_label(theme::yellow(), warning);
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
    }

    fn file_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("文件");
            let path_width = (ui.available_width() - 120.0).clamp(140.0, 280.0);
            ui.add(
                TextEdit::singleline(&mut self.path)
                    .desired_width(path_width)
                    .hint_text("选择 JSONL 回放文件"),
            );
            if design::button(ui, ICON_FOLDER_OPEN, "浏览", design::ButtonKind::Secondary).clicked()
            {
                self.want_pick_file = true;
            }
            if ui
                .add_enabled(
                    !self.load_pending,
                    egui::Button::new(design::icon_text(ICON_UPLOAD_FILE, "加载")),
                )
                .clicked()
            {
                self.commands.push(ReplayUiCommand::Load {
                    path: self.path.clone(),
                });
            }
            if self.load_pending {
                ui.spinner();
                ui.label("后台解析中…");
                ui.ctx().request_repaint();
            }
        });
    }

    fn policy_controls(&mut self, ui: &mut egui::Ui, status: &ReplayStatusView) {
        let mut policy = replay_policy_option(status.policy);
        let effective = replay_policy_option(status.effective_policy);
        if replay_policy_ui(ui, &mut policy, Some(effective)) {
            self.commands
                .push(ReplayUiCommand::SetPolicy(policy_view(policy)));
        }
        if let Some(error) = &status.analyzer_error {
            ui.colored_label(theme::red(), format!("错误: {error}"));
        }
        if let Some(warning) = &status.analyzer_warning {
            ui.colored_label(theme::yellow(), warning);
        }
    }

    fn playback_controls(&mut self, ui: &mut egui::Ui, status: &ReplayStatusView) {
        let has_events = status.total_events > 0;
        let can_control = has_events && status.state != ReplayStateView::Empty;
        let can_play = status.can_play;
        let can_seek = status.can_seek;

        ui.horizontal_wrapped(|ui| {
            if status.state == ReplayStateView::Playing {
                if design::button(ui, ICON_PAUSE, "暂停", design::ButtonKind::Secondary).clicked()
                {
                    self.commands.push(ReplayUiCommand::Pause);
                }
            } else {
                let (icon, label) = match status.state {
                    ReplayStateView::Finished => (ICON_REPLAY, "重播"),
                    _ => (ICON_PLAY_ARROW, "播放"),
                };
                if ui
                    .add_enabled(can_play, egui::Button::new(design::icon_text(icon, label)))
                    .on_disabled_hover_text("当前回放策略需要先完成 Replay Analyzer")
                    .clicked()
                {
                    if status.state == ReplayStateView::Finished {
                        self.want_clear_on_play = true;
                        self.commands.push(ReplayUiCommand::Stop);
                    } else if status.position_ms == 0 {
                        self.want_clear_on_play = true;
                    }
                    self.commands.push(ReplayUiCommand::Play);
                }
            }

            if ui
                .add_enabled(
                    can_control,
                    egui::Button::new(design::icon_text(ICON_STOP, "停止")),
                )
                .clicked()
            {
                self.commands.push(ReplayUiCommand::Stop);
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
                .add_enabled(
                    can_seek && status.cursor > 0,
                    egui::Button::new(ICON_SKIP_PREVIOUS),
                )
                .on_hover_text("后退指定事件数")
                .clicked()
            {
                self.want_step_backward = Some(self.step_size);
            }
            if ui
                .add_enabled(
                    can_seek && status.cursor < status.total_events,
                    egui::Button::new(ICON_SKIP_NEXT),
                )
                .on_hover_text("前进指定事件数")
                .clicked()
            {
                self.commands.push(ReplayUiCommand::SeekCursorDataPhase {
                    target_cursor: (status.cursor + self.step_size).min(status.total_events),
                });
            }
            ui.label(format!("{}/{}", status.cursor, status.total_events));

            ui.separator();
            ui.checkbox(&mut self.loop_playback, "循环");
            ui.separator();
            ui.label("速度");

            let mut speed_log = (self.speed.ln() / 2_f64.ln()).clamp(-3.32, 4.0);
            let speed_resp = ui.add(
                egui::Slider::new(&mut speed_log, -3.32..=4.0)
                    .text(format!("{:.2}x", self.speed))
                    .step_by(0.01),
            );
            if speed_resp.changed() {
                self.speed = (2_f64.powf(speed_log) * 100.0).round() / 100.0;
                self.speed = self.speed.clamp(0.1, 16.0);
                self.commands.push(ReplayUiCommand::SetSpeed(self.speed));
            }
            if ui.small_button("1x").clicked() {
                self.speed = 1.0;
                self.commands.push(ReplayUiCommand::SetSpeed(1.0));
            }
            for &preset in &[0.5_f64, 2.0, 5.0, 10.0] {
                if ui.small_button(format!("{preset}x")).clicked() {
                    self.speed = preset;
                    self.commands.push(ReplayUiCommand::SetSpeed(preset));
                }
            }
            speed_resp.on_hover_text(format!("回放速度 {:.2}x  |  范围 0.1x ~ 16x", self.speed));
        });

        if let Some(reason) = &status.block_reason {
            ui.separator();
            match reason {
                ReplayBlockReasonView::NeedAnalyzer => {
                    ui.colored_label(theme::yellow(), "重新解析模式需要先运行 Replay Analyzer");
                    if ui.button("运行 Analyzer").clicked() {
                        self.want_run_analyzers = true;
                    }
                }
                ReplayBlockReasonView::AnalyzerFailed(error) => {
                    ui.colored_label(theme::red(), format!("Analyzer 失败：{error}"));
                    if ui.button("重试 Analyzer").clicked() {
                        self.want_run_analyzers = true;
                    }
                    if ui.button("切换到精确回放").clicked() {
                        self.commands
                            .push(ReplayUiCommand::SetPolicy(ReplayPolicyView::ExactRecorded));
                    }
                }
            }
        }
    }

    fn progress_bar(&mut self, ui: &mut egui::Ui, status: &ReplayStatusView) {
        let progress = if status.duration_ms == 0 {
            0.0
        } else {
            status.position_ms as f32 / status.duration_ms as f32
        };
        let bar_resp = ui.add(ProgressBar::new(progress.clamp(0.0, 1.0)).text(format!(
            "{} / {}",
            ms_to_hms(status.position_ms),
            ms_to_hms(status.duration_ms),
        )));
        if status.total_events == 0 || status.duration_ms == 0 || !status.can_seek {
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
            let fraction =
                ((mpos.x - bar_resp.rect.left()) / bar_resp.rect.width()).clamp(0.0, 1.0);
            self.want_seek_replay = Some((fraction * status.duration_ms as f32) as u64);
        }
    }

    fn bookmark_controls(&mut self, ui: &mut egui::Ui, status: &ReplayStatusView) {
        if status.bookmarks.is_empty() && status.total_events == 0 {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button("+书签")
                .on_hover_text("在当前时间点添加书签")
                .clicked()
            {
                self.commands
                    .push(ReplayUiCommand::AddReplayBookmark { name: None });
            }
            for bookmark in &status.bookmarks {
                let label = bookmark.name.as_deref().unwrap_or("");
                let display = if label.is_empty() {
                    ms_to_hms(bookmark.position_ms)
                } else {
                    format!("{} {}", ms_to_hms(bookmark.position_ms), label)
                };
                if ui
                    .add_enabled(status.can_seek, egui::Button::new(display))
                    .clicked()
                {
                    self.want_seek_replay = Some(bookmark.position_ms);
                }
                if ui.small_button("×").clicked() {
                    self.commands.push(ReplayUiCommand::RemoveReplayBookmark {
                        position_ms: bookmark.position_ms,
                    });
                }
            }
        });
    }

    fn status_line(&self, ui: &mut egui::Ui, status: &ReplayStatusView) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(state_label(status.state)).color(state_color(status.state)));
            ui.separator();
            ui.label(format!("事件 {}/{}", status.cursor, status.total_events));
            ui.separator();
            ui.label(format!("{:.1}x", status.speed));
            if let Some(path) = &status.path {
                ui.separator();
                ui.label(
                    RichText::new(
                        std::path::Path::new(path)
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.clone()),
                    )
                    .monospace()
                    .color(theme::text_primary()),
                );
            }
        });
        if let Some(report) = status.load_report.as_ref()
            && report.skipped > 0
        {
            ui.colored_label(
                theme::yellow(),
                format!("加载 {} 条，跳过 {} 条坏行", report.loaded, report.skipped),
            );
            if let Some(first) = report.first_errors.first() {
                ui.colored_label(theme::text_secondary(), first);
            }
        }
    }
}

impl Default for ReplayPanel {
    fn default() -> Self {
        Self::new()
    }
}

fn replay_policy_option(policy: ReplayPolicyView) -> ReplayPolicyOption {
    match policy {
        ReplayPolicyView::AutoPreferRecorded => ReplayPolicyOption::AutoPreferRecorded,
        ReplayPolicyView::ExactRecorded => ReplayPolicyOption::ExactRecorded,
        ReplayPolicyView::ReparseRaw => ReplayPolicyOption::ReparseRaw,
    }
}

fn policy_view(policy: ReplayPolicyOption) -> ReplayPolicyView {
    match policy {
        ReplayPolicyOption::AutoPreferRecorded => ReplayPolicyView::AutoPreferRecorded,
        ReplayPolicyOption::ExactRecorded => ReplayPolicyView::ExactRecorded,
        ReplayPolicyOption::ReparseRaw => ReplayPolicyView::ReparseRaw,
    }
}

fn state_label(state: ReplayStateView) -> &'static str {
    match state {
        ReplayStateView::Empty => "空",
        ReplayStateView::Loaded => "就绪",
        ReplayStateView::Playing => "播放中",
        ReplayStateView::Paused => "已暂停",
        ReplayStateView::Finished => "已完成",
    }
}

fn state_color(state: ReplayStateView) -> Color32 {
    match state {
        ReplayStateView::Empty => theme::text_secondary(),
        ReplayStateView::Loaded | ReplayStateView::Paused => theme::yellow(),
        ReplayStateView::Playing => theme::blue(),
        ReplayStateView::Finished => theme::green(),
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
