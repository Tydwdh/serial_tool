use crate::theme;
use egui::{Color32, ProgressBar, RichText, Sense, TextEdit};
use tool_databus::DataBus;
use tool_recorder::{ReplayManager, ReplayState};

pub struct ReplayPanel {
    manager: ReplayManager,
    pub path: String,
    seek_ms: u64,
    speed: f64,
    loop_playback: bool,
    step_size: usize,
    message: Option<String>,
    pub want_pick_file: bool,
    pub auto_load: bool,
    pub want_clear_on_play: bool,
    /// Some(pos_ms): main.rs 清空终端/日志后重放到该位置
    pub want_seek_replay: Option<u64>,
    /// 步进退后：需要清空面板后回退一步
    pub want_step_backward: bool,
}

impl ReplayPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            manager: ReplayManager::new(bus.clone()),
            path: "logs/session.jsonl".to_owned(),
            seek_ms: 0,
            speed: 1.0,
            loop_playback: false,
            step_size: 1,
            message: None,
            want_pick_file: false,
            auto_load: false,
            want_clear_on_play: false,
            want_seek_replay: None,
            want_step_backward: false,
        }
    }

    pub fn try_load(&mut self) {
        match self.manager.load(&self.path) {
            Ok(count) => {
                self.seek_ms = 0;
                self.message = Some(format!("已加载 {count} 个事件"));
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    /// 执行回退重放：清空面板后回放
    pub fn do_seek_replay(&mut self, position_ms: u64) {
        self.seek_ms = position_ms;
        self.manager.seek_with_replay(position_ms);
    }

    /// 执行步进退后：调用方需先清空面板，然后从 0 重放到目标位置
    pub fn do_step_backward(&mut self) {
        if let Some(pos) = self.manager.backward_position() {
            self.seek_ms = pos;
            self.manager.seek_with_replay(pos);
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

        let status = self.manager.status();

        if self.loop_playback && status.state == ReplayState::Finished {
            self.want_clear_on_play = true;
            self.manager.seek_with_replay(0);
            self.manager.play();
        }

        ui.heading("会话回放");

        // === 文件 ===
        ui.horizontal(|ui| {
            ui.label("文件");
            ui.add(TextEdit::singleline(&mut self.path).desired_width(240.0));
            if ui.button("浏览").clicked() {
                self.want_pick_file = true;
            }
            if ui.button("加载").clicked() {
                self.try_load();
            }
        });

        ui.separator();

        // === 播放控制 ===
        ui.horizontal(|ui| {
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
                    .add_enabled(status.total_events > 0, egui::Button::new(label))
                    .clicked()
                {
                    if self.seek_ms == 0 || status.state == ReplayState::Finished {
                        self.want_clear_on_play = true;
                    }
                    self.manager.play();
                }
            }

            if ui
                .add_enabled(
                    status.total_events > 0 && status.state != ReplayState::Empty,
                    egui::Button::new("⏹ 停止"),
                )
                .clicked()
            {
                self.manager.stop();
                self.seek_ms = 0;
            }

            ui.separator();

            // 逐事件步进
            ui.label("步进");
            egui::ComboBox::from_id_salt("step-size")
                .width(48.0)
                .selected_text(format!("{}", self.step_size))
                .show_ui(ui, |ui| {
                    for &n in &[1, 5, 10, 50, 100] {
                        ui.selectable_value(&mut self.step_size, n, format!("{n}"));
                    }
                });
            if ui.button("◀").on_hover_text("后退").clicked() {
                self.want_step_backward = true;
            }
            if ui.button("▶").on_hover_text("前进").clicked() {
                for _ in 0..self.step_size {
                    self.manager.step_forward();
                }
                self.seek_ms = self.manager.status().position_ms;
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

        // === 可拖拽蓝色大进度条 ===
        let progress = if status.duration_ms == 0 {
            0.0
        } else {
            status.position_ms as f32 / status.duration_ms as f32
        };
        let bar_text = format!(
            "{} / {}",
            ms_to_hms(self.seek_ms),
            ms_to_hms(status.duration_ms),
        );
        let bar = ProgressBar::new(progress.clamp(0.0, 1.0)).text(bar_text);
        let bar_resp = ui.add(bar);
        // 透明可拖拽层覆盖在进度条上
        let drag = ui.interact(bar_resp.rect, ui.next_auto_id(), Sense::click_and_drag());
        if (drag.clicked() || drag.dragged())
            && let Some(mpos) = ui.ctx().pointer_latest_pos()
        {
            let click_frac =
                ((mpos.x - bar_resp.rect.left()) / bar_resp.rect.width()).clamp(0.0, 1.0);
            let target = (click_frac * status.duration_ms as f32) as u64;
            self.want_seek_replay = Some(target);
        }
        if status.state == ReplayState::Playing {
            self.seek_ms = status.position_ms;
        }

        // === 状态 ===
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(match status.state {
                    ReplayState::Empty => "空",
                    ReplayState::Loaded => "就绪",
                    ReplayState::Playing => "播放中",
                    ReplayState::Paused => "已暂停",
                    ReplayState::Finished => "已完成",
                })
                .color(state_color(status.state)),
            );
            ui.separator();
            ui.label(format!("事件 {}/{}", status.cursor, status.total_events));
            ui.separator();
            ui.label(format!("{:.1}x", status.speed));
            if let Some(p) = &status.path {
                ui.separator();
                ui.monospace(
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.display().to_string()),
                );
            }
        });

        if let Some(message) = &self.message {
            ui.label(message);
        }
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
