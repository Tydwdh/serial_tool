use egui::{Color32, Ui};

use crate::design;

/// Actions emitted by the shared status bar. The composition root supplies the
/// connected port and dispatches the platform-specific command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusBarAction {
    SetDtr { value: bool },
    SetRts { value: bool },
}

/// Platform-neutral values needed by the common status bar presentation.
pub struct StatusBarView {
    pub serial_color: Color32,
    pub serial_label: String,
    pub recording_color: Color32,
    pub recording_label: String,
    pub signals: Option<StatusSignalView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusSignalView {
    pub dtr: bool,
    pub rts: bool,
}

/// Render the shared left-to-right status content without opening a nested
/// layout. Native and Web can append their notification/update affordances in
/// the same parent row after this function returns.
pub fn status_bar_contents_ui(ui: &mut Ui, view: &StatusBarView) -> Vec<StatusBarAction> {
    let mut actions = Vec::new();
    design::status_pill(ui, view.serial_color, view.serial_label.clone());
    design::status_pill(ui, view.recording_color, view.recording_label.clone());

    if let Some(signals) = view.signals {
        ui.separator();
        if ui
            .add(egui::Button::new(if signals.dtr { "DTR 高" } else { "DTR 低" }).small())
            .on_hover_text("切换 DTR")
            .clicked()
        {
            actions.push(StatusBarAction::SetDtr {
                value: !signals.dtr,
            });
        }
        if ui
            .add(egui::Button::new(if signals.rts { "RTS 高" } else { "RTS 低" }).small())
            .on_hover_text("切换 RTS")
            .clicked()
        {
            actions.push(StatusBarAction::SetRts {
                value: !signals.rts,
            });
        }
    }

    actions
}
