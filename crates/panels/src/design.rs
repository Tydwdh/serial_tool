//! Hardware Workbench 的通用视觉组件。
//!
//! 业务面板只表达状态与操作，颜色、圆角、间距和按钮层级集中在这里，
//! 避免不同页面各自拼出一套视觉语言。

use egui::{Color32, Frame, Response, RichText, Stroke, WidgetText};
use egui_material_icons::MaterialIcon;

use crate::theme;

pub const CONTROL_HEIGHT: f32 = 30.0;
pub const ICON_BUTTON_SIZE: f32 = 30.0;
pub const CARD_RADIUS: f32 = 8.0;
pub const SECTION_GAP: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

pub fn icon_text(icon: MaterialIcon, label: impl AsRef<str>) -> WidgetText {
    format!("{} {}", icon.codepoint, label.as_ref()).into()
}

pub fn icon_only(icon: MaterialIcon, color: Color32, size: f32) -> RichText {
    icon.rich_text().color(color).size(size)
}

pub fn button(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    label: impl AsRef<str>,
    kind: ButtonKind,
) -> Response {
    let (fill, stroke, foreground) = match kind {
        ButtonKind::Primary => (
            theme::blue(),
            Stroke::new(1.0, theme::blue()),
            Color32::WHITE,
        ),
        ButtonKind::Secondary => (
            theme::bg_tertiary(),
            Stroke::new(1.0, theme::border_light()),
            theme::text_primary(),
        ),
        ButtonKind::Ghost => (
            Color32::TRANSPARENT,
            Stroke::new(1.0, Color32::TRANSPARENT),
            theme::text_secondary(),
        ),
        ButtonKind::Danger => (
            theme::red().gamma_multiply(0.18),
            Stroke::new(1.0, theme::red().gamma_multiply(0.7)),
            theme::red(),
        ),
    };
    ui.add(
        egui::Button::new(
            RichText::new(format!("{} {}", icon.codepoint, label.as_ref())).color(foreground),
        )
        .fill(fill)
        .stroke(stroke)
        .corner_radius(6.0)
        .min_size(egui::vec2(0.0, CONTROL_HEIGHT)),
    )
}

pub fn icon_button(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    tooltip: impl Into<WidgetText>,
) -> Response {
    ui.add_sized(
        egui::vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE),
        egui::Button::new(icon_only(icon, theme::text_secondary(), 18.0))
            .frame(false)
            .corner_radius(6.0),
    )
    .on_hover_text(tooltip)
}

pub fn card() -> Frame {
    Frame::new()
        .fill(theme::bg_card())
        .stroke(Stroke::new(1.0, theme::border()))
        .corner_radius(CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(14, 12))
}

pub fn elevated_card() -> Frame {
    card().fill(theme::bg_secondary()).shadow(egui::Shadow {
        offset: [0, 2],
        blur: 8,
        spread: 0,
        color: Color32::BLACK.gamma_multiply(0.18),
    })
}

pub fn section_header(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    title: impl AsRef<str>,
    subtitle: Option<&str>,
) {
    ui.horizontal(|ui| {
        ui.label(icon_only(icon, theme::blue(), 19.0));
        ui.vertical(|ui| {
            ui.label(
                RichText::new(title.as_ref())
                    .size(16.0)
                    .strong()
                    .color(theme::text_white()),
            );
            if let Some(subtitle) = subtitle {
                ui.label(
                    RichText::new(subtitle)
                        .small()
                        .color(theme::text_secondary()),
                );
            }
        });
    });
}

pub fn badge(ui: &mut egui::Ui, text: impl AsRef<str>, color: Color32) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.42)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text.as_ref()).small().color(color))
        })
        .inner
}

pub fn status_pill(ui: &mut egui::Ui, color: Color32, text: impl AsRef<str>) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.11))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.5, color);
                ui.label(
                    RichText::new(text.as_ref())
                        .small()
                        .color(theme::text_primary()),
                );
            })
            .response
        })
        .inner
}

pub fn empty_state(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    title: impl AsRef<str>,
    detail: impl AsRef<str>,
) {
    ui.with_layout(
        egui::Layout::top_down(egui::Align::Center).with_cross_justify(true),
        |ui| {
            ui.add_space(28.0);
            ui.label(icon_only(icon, theme::text_dimmed(), 34.0));
            ui.add_space(6.0);
            ui.label(
                RichText::new(title.as_ref())
                    .strong()
                    .color(theme::text_primary()),
            );
            ui.label(
                RichText::new(detail.as_ref())
                    .small()
                    .color(theme::text_secondary()),
            );
            ui.add_space(28.0);
        },
    );
}

pub fn segmented_toggle(
    ui: &mut egui::Ui,
    selected: &mut bool,
    off_label: &str,
    on_label: &str,
) -> Response {
    let label = if *selected { on_label } else { off_label };
    let (fill, stroke, foreground) = if *selected {
        (
            theme::toggle_selected_bg(),
            Stroke::new(1.0, theme::toggle_selected_border()),
            theme::toggle_selected_text(),
        )
    } else {
        (
            theme::bg_input(),
            Stroke::new(1.0, theme::border()),
            theme::text_secondary(),
        )
    };
    let mut text = RichText::new(label).color(foreground);
    if *selected {
        text = text.strong();
    }
    let response = ui.add(
        egui::Button::selectable(*selected, text)
            .fill(fill)
            .stroke(stroke)
            .corner_radius(6.0)
            .min_size(egui::vec2(54.0, 28.0)),
    );
    if response.clicked() {
        *selected = !*selected;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};
    use egui_material_icons::icons::ICON_CHECK;

    #[test]
    fn primary_button_is_accessible_by_label() {
        let mut harness = Harness::new_ui(|ui| {
            button(ui, ICON_CHECK, "应用", ButtonKind::Primary);
        });
        harness.run();
        assert!(harness.query_by_label_contains("应用").is_some());
    }

    #[test]
    fn compact_toolbar_keeps_every_action_accessible() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 120.0))
            .build_ui(|ui| {
                egui_material_icons::initialize(ui.ctx());
                ui.horizontal_wrapped(|ui| {
                    button(ui, ICON_CHECK, "应用", ButtonKind::Primary);
                    button(ui, ICON_CHECK, "次要操作", ButtonKind::Secondary);
                    button(ui, ICON_CHECK, "更多", ButtonKind::Ghost);
                });
            });
        harness.run();

        for label in ["应用", "次要操作", "更多"] {
            assert!(
                harness.query_by_label_contains(label).is_some(),
                "compact toolbar lost action {label}"
            );
        }
    }
}
