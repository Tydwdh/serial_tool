use eframe::egui;
use tool_panels::theme;

#[derive(Debug, Clone, Copy)]
pub(crate) enum LayoutButtonKind {
    Menu,
    ActivityBar,
    BottomPanel,
    RightDock,
}

pub(crate) fn layout_icon_button(
    ui: &mut egui::Ui,
    kind: LayoutButtonKind,
    active: bool,
    tooltip: &str,
) -> egui::Response {
    let size = egui::vec2(28.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let bg = if active {
        theme::WIDGET_ACTIVE_WEAK
    } else if response.hovered() {
        theme::WIDGET_HOVER
    } else {
        egui::Color32::TRANSPARENT
    };

    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, bg);
    }

    let stroke = egui::Stroke::new(
        1.4,
        if active {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );

    let icon = rect.shrink2(egui::vec2(5.0, 4.0));

    match kind {
        LayoutButtonKind::Menu => paint_layout_menu_icon(ui, icon, stroke),
        LayoutButtonKind::ActivityBar => paint_activity_bar_icon(ui, icon, stroke),
        LayoutButtonKind::BottomPanel => paint_bottom_panel_icon(ui, icon, stroke),
        LayoutButtonKind::RightDock => paint_right_dock_icon(ui, icon, stroke),
    }

    response.on_hover_text(tooltip)
}

fn paint_outer(ui: &egui::Ui, rect: egui::Rect, stroke: egui::Stroke) {
    ui.painter().rect_stroke(
        rect,
        2.0,
        stroke,
        egui::StrokeKind::Inside,
    );
}

fn paint_layout_menu_icon(ui: &egui::Ui, rect: egui::Rect, stroke: egui::Stroke) {
    let w = rect.width();
    let h = rect.height();

    let r1 = egui::Rect::from_min_size(
        rect.left_top(),
        egui::vec2(w * 0.35, h * 0.45),
    );
    let r2 = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.bottom() - h * 0.35),
        egui::vec2(w * 0.35, h * 0.35),
    );
    let r3 = egui::Rect::from_min_size(
        egui::pos2(rect.right() - w * 0.45, rect.top()),
        egui::vec2(w * 0.45, h),
    );

    paint_outer(ui, r1, stroke);
    paint_outer(ui, r2, stroke);
    paint_outer(ui, r3, stroke);
}

fn paint_activity_bar_icon(ui: &egui::Ui, rect: egui::Rect, stroke: egui::Stroke) {
    paint_outer(ui, rect, stroke);

    let left = egui::Rect::from_min_max(
        rect.left_top(),
        egui::pos2(rect.left() + rect.width() * 0.28, rect.bottom()),
    );

    ui.painter()
        .rect_filled(left.shrink(1.5), 1.0, stroke.color);
}

fn paint_bottom_panel_icon(ui: &egui::Ui, rect: egui::Rect, stroke: egui::Stroke) {
    paint_outer(ui, rect, stroke);

    let bottom = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - rect.height() * 0.32),
        rect.right_bottom(),
    );

    ui.painter()
        .rect_filled(bottom.shrink(1.5), 1.0, stroke.color);
}

fn paint_right_dock_icon(ui: &egui::Ui, rect: egui::Rect, stroke: egui::Stroke) {
    paint_outer(ui, rect, stroke);

    let right = egui::Rect::from_min_max(
        egui::pos2(rect.right() - rect.width() * 0.30, rect.top()),
        rect.right_bottom(),
    );

    ui.painter()
        .rect_filled(right.shrink(1.5), 1.0, stroke.color);
}
