use crate::theme;
use egui::{Color32, ProgressBar, RichText, Sense, TextEdit};
use tool_databus::DataBus;
use tool_recorder::{ReplayManager, ReplayState};

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
        }
    }

    pub fn try_load(&mut self) {
        match self.manager.load(&self.path) {
            Ok(count) => {
                self.message = Some(format!("已加载 {count} 个事件"));
                self.manager.set_speed(self.speed);
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

    /// 执行步进退后：调用方需要先清空终端/日志
    pub fn do_step_backward(&mut self, steps: usize) {
        let steps = steps.max(1);

        if let Some(position_ms) = self.manager.backward_position_by(steps) {
            self.manager.seek_with_replay(position_ms);
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

        self.playback_controls(ui, &status);

        // 控件可能修改了 manager 状态，这里重新取一次。
        status = self.manager.status();

        self.progress_bar(ui, &status);
        self.status_line(ui, &status);

        if let Some(message) = &self.message {
            ui.label(message);
        }
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

    fn playback_controls(&mut self, ui: &mut egui::Ui, status: &tool_recorder::ReplayStatus) {
        let has_events = status.total_events > 0;
        let can_control = has_events && status.state != ReplayState::Empty;
        let can_step = can_control && status.state != ReplayState::Playing;

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
                    .add_enabled(has_events, egui::Button::new(label))
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
                .add_enabled(can_step && status.cursor > 0, egui::Button::new("◀"))
                .on_hover_text("后退指定事件数")
                .clicked()
            {
                self.want_step_backward = Some(self.step_size);
            }

            if ui
                .add_enabled(
                    can_step && status.cursor < status.total_events,
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

fn state_color(state: ReplayState) -> Color32 {
    match state {
        ReplayState::Empty => theme::TEXT_SECONDARY,
        ReplayState::Loaded | ReplayState::Paused => theme::YELLOW,
        ReplayState::Playing => theme::BLUE,
        ReplayState::Finished => theme::GREEN,
    }
}
