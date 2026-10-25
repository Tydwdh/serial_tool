//! 行级布局原语。
//!
//! 这里只放“一行怎么排”的无状态规则：标签列、弹性值列、右侧动作列，以及
//! 面板工具栏的统一高度。dock/面板树由 `PanelManager` / `egui_tiles` 管，
//! 不在这里重复。

use egui::{Align, Color32, FontId, Label, Response, RichText, Ui, containers::Sides};

use super::{CONTROL_HEIGHT, ICON_BUTTON_SIZE};

/// 设置页/表单行的默认标签列宽。
pub const FIELD_LABEL_WIDTH: f32 = 96.0;

/// 一行里值列至少保留的宽度；低于这个宽度就改为上下两行。
pub const FIELD_MIN_VALUE_WIDTH: f32 = 96.0;

/// 一行配置/表单行的返回值。动作的返回类型由调用方决定（通常是若干 `Response`）。
pub struct FieldRowResponse<V, A> {
    pub label: Response,
    pub value: V,
    pub actions: A,
}

/// 标签 + 弹性值 + 右侧动作的行构造器。
///
/// 右侧动作先布局，值列拿剩余宽度；因此“打开目录”这类尾部按钮不会被长路径
/// 或长值挤出面板。宽度不足时自动降级为「标签一行，值 + 动作一行」。
pub struct FieldRow<'a> {
    label: &'a str,
    label_width: f32,
    min_value_width: f32,
    action_count: usize,
    min_actions_width: Option<f32>,
}

impl<'a> FieldRow<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            label_width: FIELD_LABEL_WIDTH,
            min_value_width: FIELD_MIN_VALUE_WIDTH,
            action_count: 0,
            min_actions_width: None,
        }
    }

    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = width;
        self
    }

    pub fn min_value_width(mut self, width: f32) -> Self {
        self.min_value_width = width;
        self
    }

    /// 右侧动作数量。默认按 `ICON_BUTTON_SIZE` 估算；文字按钮请用
    /// [`Self::min_actions_width`] 覆盖。
    pub fn actions(mut self, count: usize) -> Self {
        self.action_count = count;
        self
    }

    pub fn min_actions_width(mut self, width: f32) -> Self {
        self.min_actions_width = Some(width);
        self
    }

    /// 有右侧动作的行。`actions` 闭包收到一个从左到右的 Ui。
    pub fn show<V, A>(
        self,
        ui: &mut Ui,
        value: impl FnOnce(&mut Ui) -> V,
        actions: impl FnOnce(&mut Ui) -> A,
    ) -> FieldRowResponse<V, A> {
        let spacing = ui.spacing().item_spacing.x;
        let actions_width = self.actions_width(spacing);
        let one_line_min =
            self.label_width + spacing + self.min_value_width + spacing + actions_width;
        if ui.available_width() >= one_line_min {
            let (left, actions) = Sides::new()
                .height(CONTROL_HEIGHT)
                .spacing(spacing)
                .shrink_left()
                .show(
                    ui,
                    |ui| {
                        let label = label_cell(ui, self.label, self.label_width);
                        let value = value(ui);
                        (label, value)
                    },
                    |ui| actions_ui(ui, actions_width, actions),
                );
            FieldRowResponse {
                label: left.0,
                value: left.1,
                actions,
            }
        } else {
            ui.vertical(|ui| {
                let label = ui.label(self.label);
                let (value, actions) = Sides::new()
                    .height(CONTROL_HEIGHT)
                    .spacing(spacing)
                    .shrink_left()
                    .show(ui, value, |ui| actions_ui(ui, actions_width, actions));
                FieldRowResponse {
                    label,
                    value,
                    actions,
                }
            })
            .inner
        }
    }

    /// 没有右侧动作的行。避免为空的动作列额外留一段间距。
    pub fn show_value<V>(
        self,
        ui: &mut Ui,
        value: impl FnOnce(&mut Ui) -> V,
    ) -> FieldRowResponse<V, ()> {
        let spacing = ui.spacing().item_spacing.x;
        let one_line_min = self.label_width + spacing + self.min_value_width;
        if ui.available_width() >= one_line_min {
            let (label, value) = ui
                .horizontal(|ui| {
                    let label = label_cell(ui, self.label, self.label_width);
                    let value = value(ui);
                    (label, value)
                })
                .inner;
            FieldRowResponse {
                label,
                value,
                actions: (),
            }
        } else {
            let label = ui.label(self.label);
            let value = value(ui);
            FieldRowResponse {
                label,
                value,
                actions: (),
            }
        }
    }

    fn actions_width(&self, spacing: f32) -> f32 {
        if self.action_count == 0 {
            0.0
        } else if let Some(width) = self.min_actions_width {
            width
        } else {
            self.action_count as f32 * ICON_BUTTON_SIZE + (self.action_count - 1) as f32 * spacing
        }
    }
}

/// 右侧动作固定按从左到右排列。`Sides` 的右槽本身是 right-to-left，
/// 若直接把动作加进去，调用方写的顺序会被镜像（复制在右、打开在左）。
fn actions_ui<A>(ui: &mut Ui, width: f32, actions: impl FnOnce(&mut Ui) -> A) -> A {
    // `Sides` 的右槽是 right-to-left。用一个「占满预估宽度、内部 left-to-right」
    // 的子 Ui 包住动作：右槽会把整块贴到右边，而复制/打开仍按调用方顺序排列。
    ui.allocate_ui_with_layout(
        egui::vec2(width, CONTROL_HEIGHT),
        egui::Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, CONTROL_HEIGHT));
            actions(ui)
        },
    )
    .inner
}

fn label_cell(ui: &mut Ui, label: &str, width: f32) -> Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, CONTROL_HEIGHT),
        egui::Layout::left_to_right(Align::Center),
        |ui| {
            // `allocate_ui_with_layout` 只限制最大尺寸；内容比标签列窄时父游标
            // 会按内容宽度前进，导致相邻两行起点漂移。显式撑满列宽。
            ui.set_min_size(egui::vec2(width, CONTROL_HEIGHT));
            ui.add(Label::new(label).truncate().halign(Align::LEFT))
                .on_hover_text(label)
        },
    )
    .inner
}

/// 按当前可用宽度做中间省略的等宽/比例文本；悬停显示完整内容。
pub fn fitted_label(ui: &mut Ui, text: &str, font: FontId, color: Color32) -> Response {
    let width = ui.available_width();
    let glyph_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M')).max(1.0);
    let max_chars = ((width / glyph_width).floor() as usize).max(4);
    let display = crate::compact_middle(text, max_chars);
    ui.add(
        Label::new(RichText::new(display).font(font).color(color))
            .truncate()
            .halign(Align::LEFT),
    )
    .on_hover_text(text)
}

/// 面板工具栏的统一入口：固定最小高度，保留 `horizontal_wrapped` 的换行行为。
///
/// 只统一“行”的高度与换行策略；具体控件仍由调用方表达。
pub fn toolbar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_wrapped(|ui| {
        ui.set_min_height(CONTROL_HEIGHT);
        add(ui)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_material_icons::icons::{ICON_CONTENT_COPY, ICON_FOLDER_OPEN};

    fn test_style(ctx: &egui::Context) {
        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.spacing.interact_size = egui::vec2(44.0, 30.0);
        ctx.set_global_style(style);
    }

    #[test]
    fn field_row_keeps_actions_inside_and_aligns_columns() {
        for width in [484.0, 360.0, 280.0] {
            let ctx = egui::Context::default();
            egui_material_icons::initialize(&ctx);
            test_style(&ctx);
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 300.0),
                )),
                ..Default::default()
            };
            let mut content_right = 0.0;
            let mut rows = None;
            let _ = ctx.run_ui(raw, |ui| {
                super::super::card().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    content_right = ui.max_rect().right();
                    let first = FieldRow::new("工作区配置").actions(2).show(
                        ui,
                        |ui| {
                            fitted_label(
                                ui,
                                r"C:\Users\tyd27\AppData\Roaming\HardwareWorkbench\workspace.json",
                                egui::TextStyle::Monospace.resolve(ui.style()),
                                Color32::WHITE,
                            )
                        },
                        |ui| {
                            (
                                super::super::icon_button(ui, ICON_CONTENT_COPY, "复制路径"),
                                super::super::icon_button(ui, ICON_FOLDER_OPEN, "打开所在目录"),
                            )
                        },
                    );
                    let second = FieldRow::new("插件配置").actions(2).show(
                        ui,
                        |ui| {
                            fitted_label(
                                ui,
                                r"C:\Users\tyd27\AppData\Roaming\HardwareWorkbench\plugin-config",
                                egui::TextStyle::Monospace.resolve(ui.style()),
                                Color32::WHITE,
                            )
                        },
                        |ui| {
                            (
                                super::super::icon_button(ui, ICON_CONTENT_COPY, "复制路径"),
                                super::super::icon_button(ui, ICON_FOLDER_OPEN, "打开所在目录"),
                            )
                        },
                    );
                    rows = Some((first, second));
                });
            });
            let (first, second) = rows.expect("两行都要渲染");
            for row in [&first, &second] {
                assert!(
                    row.actions.1.rect.right() <= content_right + 0.5,
                    "width={width}: 尾部动作被裁到卡片外：{:?} > {content_right}",
                    row.actions.1.rect
                );
                assert!(
                    row.value.rect.right() <= row.actions.0.rect.left() + 0.5,
                    "width={width}: 值列压到动作列 value={:?} copy={:?} open={:?} label={:?}",
                    row.value.rect,
                    row.actions.0.rect,
                    row.actions.1.rect,
                    row.label.rect
                );
                assert!(
                    (row.actions.0.rect.width() - ICON_BUTTON_SIZE).abs() <= 2.0,
                    "width={width}: 图标按钮宽度 {} 偏离 {}",
                    row.actions.0.rect.width(),
                    ICON_BUTTON_SIZE
                );
            }
            assert!(
                (first.label.rect.left() - second.label.rect.left()).abs() < 0.5,
                "width={width}: 两行标签起点不一致"
            );
            assert!(
                (first.value.rect.left() - second.value.rect.left()).abs() < 0.5,
                "width={width}: 两行值起点不一致"
            );
        }
    }

    #[test]
    fn field_row_stacks_below_breakpoint() {
        let ctx = egui::Context::default();
        egui_material_icons::initialize(&ctx);
        test_style(&ctx);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(240.0, 200.0),
            )),
            ..Default::default()
        };
        let mut row = None;
        let _ = ctx.run_ui(raw, |ui| {
            row = Some(FieldRow::new("插件配置").actions(2).show(
                ui,
                |ui| {
                    fitted_label(
                        ui,
                        r"C:\Users\tyd27\AppData\Roaming\HardwareWorkbench\plugin-config",
                        egui::TextStyle::Monospace.resolve(ui.style()),
                        Color32::WHITE,
                    )
                },
                |ui| {
                    (
                        super::super::icon_button(ui, ICON_CONTENT_COPY, "复制路径"),
                        super::super::icon_button(ui, ICON_FOLDER_OPEN, "打开所在目录"),
                    )
                },
            ));
        });
        let row = row.expect("row should render");
        assert!(
            row.label.rect.bottom() <= row.value.rect.top(),
            "窄面板应把标签放到值列上一行：label={:?} value={:?}",
            row.label.rect,
            row.value.rect
        );
    }
}
