//! Shared recorder presentation.
//!
//! The recorder implementation is platform-specific (filesystem on Native,
//! Blob download on Web), but its user-facing state machine and card layout
//! are the same. This module keeps those two concerns separate.

use crate::{design, theme};
use egui::TextEdit;
use egui_material_icons::icons::{
    ICON_FIBER_MANUAL_RECORD, ICON_FOLDER_OPEN, ICON_PAUSE, ICON_PLAY_ARROW, ICON_STOP,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingMode {
    StandardReplay,
    RawSerial,
}

impl RecordingMode {
    pub const ALL: [Self; 2] = [Self::StandardReplay, Self::RawSerial];

    pub fn label(self) -> &'static str {
        match self {
            Self::StandardReplay => "标准回放",
            Self::RawSerial => "原始串口",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingAction {
    Browse,
    StartStop,
    PauseResume,
}

pub struct RecordingView<'a> {
    pub file_name: &'a mut String,
    pub mode: &'a mut RecordingMode,
    pub running: bool,
    pub stopping: bool,
    pub paused: bool,
    pub events_written: u64,
    pub bytes_written: Option<u64>,
    pub flush_elapsed_ms: Option<u64>,
    pub backlog_events: Option<u64>,
    pub backlog_bytes: Option<u64>,
    pub current_path: Option<&'a str>,
    pub last_error: Option<&'a str>,
    pub show_browse: bool,
}

/// Render the common recorder card and return platform-neutral user actions.
pub fn recording_ui(ui: &mut egui::Ui, view: &mut RecordingView<'_>) -> Vec<RecordingAction> {
    let mut actions = Vec::new();
    design::card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        design::section_header(ui, ICON_FIBER_MANUAL_RECORD, "录制");
        ui.separator();

        ui.horizontal_wrapped(|ui| {
            ui.label("路径");
            let path_width = (ui.available_width() - 150.0).clamp(140.0, 360.0);
            ui.add_enabled(
                !view.running,
                TextEdit::singleline(view.file_name).desired_width(path_width),
            );

            if view.show_browse
                && ui
                    .add_enabled(
                        !view.running,
                        egui::Button::new(design::icon_text(ICON_FOLDER_OPEN, "浏览")),
                    )
                    .on_hover_text(if view.running {
                        "录制中不能修改保存路径"
                    } else {
                        "选择录制保存路径"
                    })
                    .clicked()
            {
                actions.push(RecordingAction::Browse);
            }

            if view.stopping {
                ui.ctx().request_repaint();
                ui.spinner();
            }
            if ui
                .add_enabled(
                    !view.stopping,
                    egui::Button::new(design::icon_text(
                        if view.running {
                            ICON_STOP
                        } else {
                            ICON_FIBER_MANUAL_RECORD
                        },
                        if view.running { "停止" } else { "录制" },
                    )),
                )
                .on_disabled_hover_text("正在停止中...")
                .clicked()
            {
                actions.push(RecordingAction::StartStop);
            }

            if view.running
                && ui
                    .add_enabled(
                        !view.stopping,
                        egui::Button::new(design::icon_text(
                            if view.paused {
                                ICON_PLAY_ARROW
                            } else {
                                ICON_PAUSE
                            },
                            if view.paused { "继续" } else { "暂停" },
                        )),
                    )
                    .on_disabled_hover_text("正在停止中...")
                    .clicked()
            {
                actions.push(RecordingAction::PauseResume);
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("模式");
            ui.add_enabled_ui(!view.running, |ui| {
                egui::ComboBox::from_id_salt("shared-record-mode")
                    .width(160.0)
                    .selected_text(view.mode.label())
                    .show_ui(ui, |ui| {
                        for mode in RecordingMode::ALL {
                            ui.selectable_value(view.mode, mode, mode.label());
                        }
                    });
            });
        });

        if view.running || view.stopping {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if view.paused {
                    design::status_pill(ui, theme::yellow(), "已暂停，未写入新事件");
                } else if view.running {
                    design::status_pill(ui, theme::green(), "录制中");
                } else {
                    design::status_pill(ui, theme::yellow(), "正在停止");
                }
                ui.label(format!("事件 {}", view.events_written));
                if let Some(bytes) = view.bytes_written {
                    ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
                }
                if let Some(elapsed) = view.flush_elapsed_ms {
                    ui.label(format!("flush {} ms 前", elapsed));
                }
                if let Some(events) = view.backlog_events {
                    ui.label(format!("积压 {} 个", events));
                }
                if let Some(bytes) = view.backlog_bytes {
                    ui.label(format!("{} 字节", bytes));
                }
            });
            if let Some(path) = view.current_path {
                ui.label(format!("路径：{path}"));
            }
        }

        if let Some(error) = view.last_error {
            ui.colored_label(theme::red(), format!("录制错误：{error}"));
        }
    });
    actions
}
