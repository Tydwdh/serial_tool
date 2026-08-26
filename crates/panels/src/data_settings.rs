//! Shared terminal/log data settings presentation.

use crate::theme;

pub struct DataSettingsView<'a> {
    pub merge_window_ms: &'a mut u64,
    pub terminal_max_entries: &'a mut usize,
    pub log_max_entries: &'a mut usize,
}

/// Render the settings shared by the Native and Web composition roots.
/// Returns true when one of the values changed.
pub fn data_settings_ui(ui: &mut egui::Ui, view: &mut DataSettingsView<'_>) -> bool {
    let mut changed = false;

    ui.horizontal_wrapped(|ui| {
        ui.label("终端空闲结束阈值");
        let mut merge_window = *view.merge_window_ms as f32;
        if ui
            .add(
                egui::Slider::new(&mut merge_window, 0.0..=100.0)
                    .step_by(5.0)
                    .suffix(" ms"),
            )
            .on_hover_text(
                "展示块超过此毫秒没有新数据就暂时封存；换行和展示分段会直接封存。\
                 这只是展示边界，不代表协议帧。",
            )
            .changed()
        {
            *view.merge_window_ms = merge_window.round() as u64;
            changed = true;
        }
    });

    entry_limit_row(ui, "终端保留条数", view.terminal_max_entries, &mut changed);
    entry_limit_row(ui, "日志保留条数", view.log_max_entries, &mut changed);
    changed
}

fn entry_limit_row(ui: &mut egui::Ui, label: &str, value: &mut usize, changed: &mut bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(label);
        let mut number = (*value).clamp(500, 200_000);
        let drag_changed = ui
            .add(
                egui::DragValue::new(&mut number)
                    .range(500..=200_000)
                    .speed(500),
            )
            .changed();
        let slider_changed = ui
            .add(egui::Slider::new(&mut number, 500..=200_000).step_by(500.0))
            .changed();
        if drag_changed || slider_changed {
            *value = number;
            *changed = true;
        }
    });
    ui.label(
        egui::RichText::new("超出后丢弃最旧条目")
            .small()
            .color(theme::text_secondary()),
    );
}
