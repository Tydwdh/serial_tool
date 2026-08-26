//! Shared shortcut settings presentation.

use crate::{design, theme};

/// One command row supplied by a platform's command registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapEntry {
    pub id: String,
    pub title: String,
    pub bindings: String,
    pub recording: bool,
}

/// Actions that need to be applied to the platform-specific keymap state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapAction {
    Record(String),
    Clear(String),
    RestoreDefaults,
}

/// Render shortcut rows identically on Native and Web.
pub fn keymap_ui(ui: &mut egui::Ui, entries: &[KeymapEntry]) -> Vec<KeymapAction> {
    let mut actions = Vec::new();
    for entry in entries {
        ui.horizontal_wrapped(|ui| {
            ui.add_sized(
                egui::vec2(180.0, ui.spacing().interact_size.y),
                egui::Label::new(&entry.title),
            );
            if entry.bindings.is_empty() {
                ui.colored_label(theme::text_dimmed(), "未绑定");
            } else {
                design::badge(ui, &entry.bindings, theme::cyan());
            }
            if entry.recording {
                design::status_pill(ui, theme::yellow(), "按下按键…");
            } else if design::button(
                ui,
                egui_material_icons::icons::ICON_KEYBOARD,
                "录制",
                design::ButtonKind::Ghost,
            )
            .clicked()
            {
                actions.push(KeymapAction::Record(entry.id.clone()));
            }
            if !entry.bindings.is_empty()
                && design::button(
                    ui,
                    egui_material_icons::icons::ICON_RESTART_ALT,
                    "清除",
                    design::ButtonKind::Ghost,
                )
                .clicked()
            {
                actions.push(KeymapAction::Clear(entry.id.clone()));
            }
        });
        ui.separator();
    }
    ui.horizontal_wrapped(|ui| {
        if design::button(
            ui,
            egui_material_icons::icons::ICON_RESTART_ALT,
            "恢复默认快捷键",
            design::ButtonKind::Secondary,
        )
        .clicked()
        {
            actions.push(KeymapAction::RestoreDefaults);
        }
    });
    actions
}
