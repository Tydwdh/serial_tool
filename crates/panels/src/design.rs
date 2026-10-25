//! Hardware Workbench 的通用视觉组件。
//!
//! 业务面板只表达状态与操作，颜色、圆角、间距和按钮层级集中在这里，
//! 避免不同页面各自拼出一套视觉语言。

use egui::{Color32, Frame, Response, RichText, Stroke, WidgetText};
use egui_material_icons::MaterialIcon;

use crate::theme;

mod layout;

pub use layout::{
    FIELD_LABEL_WIDTH, FIELD_MIN_VALUE_WIDTH, FieldRow, FieldRowResponse, fitted_label, toolbar,
};

pub const CONTROL_HEIGHT: f32 = 32.0;
pub const ICON_BUTTON_SIZE: f32 = 32.0;
pub const CARD_RADIUS: f32 = 10.0;
pub const SECTION_GAP: f32 = 14.0;

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

/// 在换行行（`horizontal_wrapped`）里按已知宽度放置一个下拉框。
///
/// `egui::ComboBox` 自身不参与换行：剩余宽度不足时它直接向右溢出。更糟的是
/// `Region::expand_to_include_rect` 会同时放大 `min_rect` 与 `max_rect`，于是
/// **同一块 Ui 里后面所有行**都改用被撑坏的边界排版——表现就是按钮被裁到面板外、
/// 而且不再换行。先用 `allocate_ui_with_layout` 按已知宽度占位，换行判断就落在
/// 占位块上，ComboBox 在块内正常绘制。
pub fn combo_slot<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let height = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        add,
    )
    .inner
}

pub fn button(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    label: impl AsRef<str>,
    kind: ButtonKind,
) -> Response {
    // 不直接 `Button::fill/stroke`：那两个方法会覆盖所有状态，悬停与按下就失去
    // 任何反馈。这里按状态改局部 style，按钮仍走 egui 自己的 state 解析。
    let (idle, hovered, active, idle_stroke, hover_stroke, foreground) = match kind {
        ButtonKind::Primary => (
            theme::blue(),
            theme::blue().gamma_multiply(1.08),
            theme::blue().gamma_multiply(0.92),
            Stroke::new(1.0, theme::blue()),
            Stroke::new(1.0, theme::blue()),
            Color32::WHITE,
        ),
        ButtonKind::Secondary => (
            theme::bg_tertiary(),
            theme::bg_hover(),
            theme::widget_active_weak(),
            Stroke::new(1.0, theme::border()),
            Stroke::new(1.0, theme::border_light()),
            theme::text_primary(),
        ),
        ButtonKind::Ghost => (
            Color32::TRANSPARENT,
            theme::bg_hover(),
            theme::bg_tertiary(),
            Stroke::NONE,
            Stroke::NONE,
            theme::text_secondary(),
        ),
        ButtonKind::Danger => (
            theme::red().gamma_multiply(0.16),
            theme::red().gamma_multiply(0.26),
            theme::red().gamma_multiply(0.34),
            Stroke::new(1.0, theme::red().gamma_multiply(0.55)),
            Stroke::new(1.0, theme::red().gamma_multiply(0.8)),
            theme::red(),
        ),
    };
    ui.scope(|ui| {
        let widgets = &mut ui.style_mut().visuals.widgets;
        widgets.inactive.weak_bg_fill = idle;
        widgets.inactive.bg_stroke = idle_stroke;
        widgets.hovered.weak_bg_fill = hovered;
        widgets.hovered.bg_stroke = hover_stroke;
        widgets.active.weak_bg_fill = active;
        widgets.active.bg_stroke = hover_stroke;
        ui.add(
            egui::Button::new(icon_text(icon, label).color(foreground))
                .corner_radius(CARD_RADIUS)
                .min_size(egui::vec2(0.0, CONTROL_HEIGHT)),
        )
    })
    .inner
}

pub fn icon_button(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    tooltip: impl Into<WidgetText>,
) -> Response {
    // 图标按钮常态无底色，但悬停要有一层浅底；`frame(false)` 会把悬停态一起
    // 抹掉，因此这里用透明底色 + 状态色。
    ui.scope(|ui| {
        // 图标按钮必须真的占 ICON_BUTTON_SIZE，不能吃全局 button_padding /
        // interact_size：否则工具栏按 32px 算宽度，实际按钮有 40+px，
        // 最右侧的按钮会被挤出面板（配置路径行的「打开目录」就是这样消失的）。
        ui.spacing_mut().button_padding = egui::vec2(6.0, 5.0);
        ui.spacing_mut().interact_size = egui::vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE);
        let widgets = &mut ui.style_mut().visuals.widgets;
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.inactive.bg_stroke = Stroke::NONE;
        widgets.hovered.weak_bg_fill = theme::bg_hover();
        widgets.hovered.bg_stroke = Stroke::NONE;
        widgets.active.weak_bg_fill = theme::bg_tertiary();
        widgets.active.bg_stroke = Stroke::NONE;
        ui.add_sized(
            egui::vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE),
            egui::Button::new(icon_only(icon, theme::text_secondary(), 18.0))
                .min_size(egui::vec2(ICON_BUTTON_SIZE, ICON_BUTTON_SIZE))
                .corner_radius(CARD_RADIUS),
        )
    })
    .inner
    .on_hover_text(tooltip)
}

pub fn card() -> Frame {
    Frame::new()
        .fill(theme::bg_card())
        // 卡片边框用更亮的层级：边框与卡片底色只差 8 个灰阶时，深色屏幕上
        // 卡片会完全融进面板背景。
        .stroke(Stroke::new(1.0, theme::border_light().gamma_multiply(0.55)))
        .corner_radius(CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(16, 14))
}

/// 两步确认的机械状态：首次点击武装，再次点击才算确认。
///
/// 与 egui 无关，便于单独测试。默认 3 秒内未确认即自动解除。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TwoStepConfirm {
    armed_at_ms: Option<u64>,
}

impl TwoStepConfirm {
    /// 确认态自动解除的时长（毫秒）。用户看得到、也来得及点第二下。
    pub const TIMEOUT_MS: u64 = 3_000;

    pub fn is_armed(&self, now_ms: u64) -> bool {
        self.armed_at_ms
            .is_some_and(|armed| now_ms.saturating_sub(armed) < Self::TIMEOUT_MS)
    }

    pub fn arm(&mut self, now_ms: u64) {
        self.armed_at_ms = Some(now_ms);
    }

    pub fn disarm(&mut self) {
        self.armed_at_ms = None;
    }

    /// 处理一次点击。返回 `true` 表示这次点击应当真正执行动作。
    pub fn click(&mut self, now_ms: u64) -> bool {
        if self.is_armed(now_ms) {
            self.disarm();
            true
        } else {
            self.arm(now_ms);
            false
        }
    }
}

/// 读取两步确认按钮当前是否处于确认态，供调用方选择标签与配色。
///
/// 标签必须与 [`confirm_click`] 中渲染的按钮使用同一个 `id_salt`。
pub fn confirm_armed(ui: &egui::Ui, id_salt: &str) -> bool {
    read_confirm_state(ui, id_salt).is_armed(now_ms(ui))
}

/// 提交两步确认按钮本帧的交互，返回是否应当执行动作。
///
/// 两个步骤都没有额外占位：确认态只换标签，不要另外插入「取消」按钮——那样会
/// 加宽工具栏，窄面板下 `horizontal_wrapped` 会把整组确认控件折到下一行。
/// 取消由超时、Esc 和「点击别处」负责。
///
/// 调用方必须保证确认态标签与常态标签等宽（通常同为两个或四个全角字符）。
pub fn confirm_click(ui: &egui::Ui, id_salt: &str, clicked: bool) -> bool {
    let now = now_ms(ui);
    let mut state = read_confirm_state(ui, id_salt);
    if clicked {
        let confirmed = state.click(now);
        write_confirm_state(ui, id_salt, state);
        return confirmed;
    }

    // 只有已经武装时才需要处理「放弃」：Esc 或任意其它点击都算取消。
    //
    // 这里只看 `any_click()`（按下并抬起）而不是 `any_pressed()`：确认态的第二次
    // 点击在按下帧还没有产生 `clicked()`，若把按下也算作「点击别处」，就会在
    // 用户确认的瞬间先解除、再被抬起帧重新武装。
    let dismissed = state.is_armed(now)
        && ui.input(|input| input.pointer.any_click() || input.key_pressed(egui::Key::Escape));
    if dismissed {
        state.disarm();
        write_confirm_state(ui, id_salt, state);
    }
    false
}

fn now_ms(ui: &egui::Ui) -> u64 {
    (ui.input(|input| input.time) * 1_000.0).max(0.0) as u64
}

fn confirm_id(ui: &egui::Ui, id_salt: &str) -> egui::Id {
    ui.id().with(id_salt)
}

fn read_confirm_state(ui: &egui::Ui, id_salt: &str) -> TwoStepConfirm {
    ui.ctx()
        .memory(|memory| memory.data.get_temp(confirm_id(ui, id_salt)))
        .unwrap_or_default()
}

fn write_confirm_state(ui: &egui::Ui, id_salt: &str, state: TwoStepConfirm) {
    let id = confirm_id(ui, id_salt);
    // 时间必须在取内存锁之前算好：在 memory_mut 闭包里读 input 会形成锁嵌套。
    let now = now_ms(ui);
    ui.ctx().memory_mut(|memory| {
        if state.is_armed(now) {
            memory.data.insert_temp(id, state);
        } else {
            memory.data.remove_temp::<TwoStepConfirm>(id);
        }
    });
}

pub fn elevated_card() -> Frame {
    card().fill(theme::bg_secondary()).shadow(egui::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::BLACK.gamma_multiply(0.28),
    })
}

pub fn section_header(ui: &mut egui::Ui, icon: MaterialIcon, title: impl AsRef<str>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(icon_only(icon, theme::blue(), 19.0));
        ui.label(
            RichText::new(title.as_ref())
                .size(15.5)
                .strong()
                .color(theme::text_white()),
        );
    });
}

pub fn badge(ui: &mut egui::Ui, text: impl AsRef<str>, color: Color32) -> Response {
    ui.add(
        egui::Button::new(RichText::new(text.as_ref()).small().color(color))
            .sense(egui::Sense::hover())
            .wrap_mode(egui::TextWrapMode::Extend)
            .fill(color.gamma_multiply(0.14))
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.42)))
            .corner_radius(10.0)
            .min_size(egui::vec2(0.0, 24.0)),
    )
}

pub fn status_pill(ui: &mut egui::Ui, color: Color32, text: impl AsRef<str>) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.11))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
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

pub fn status_pill_sized(
    ui: &mut egui::Ui,
    color: Color32,
    text: impl AsRef<str>,
    width: f32,
) -> Response {
    Frame::new()
        .fill(color.gamma_multiply(0.11))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.35)))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.set_min_width((width - 20.0).max(0.0));
            ui.with_layout(
                egui::Layout::left_to_right(egui::Align::Center)
                    .with_main_align(egui::Align::Center),
                |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 3.5, color);
                    ui.label(
                        RichText::new(text.as_ref())
                            .small()
                            .color(theme::text_primary()),
                    );
                },
            );
        })
        .response
}

pub fn empty_state(ui: &mut egui::Ui, icon: MaterialIcon, title: impl AsRef<str>) {
    empty_state_with_hint(ui, icon, title, "");
}

/// 空状态：图标 + 主标题 + 一句说明。说明允许换行，窄面板里不会溢出。
pub fn empty_state_with_hint(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    title: impl AsRef<str>,
    hint: impl AsRef<str>,
) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.add_space(32.0);
        ui.label(icon_only(icon, theme::text_dimmed(), 38.0));
        ui.add_space(10.0);
        ui.label(
            RichText::new(title.as_ref())
                .size(15.5)
                .strong()
                .color(theme::text_primary()),
        );
        let hint = hint.as_ref();
        if !hint.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(hint)
                    .size(13.0)
                    .color(theme::text_secondary()),
            );
        }
        ui.add_space(32.0);
    });
}

/// 一枚可切换的胶囊按钮。`Button::selectable(false)` 默认在未选中时不画底框，
/// 组里的其余选项会退化成纯文字；这里始终保留底框，才像一组分段控件。
pub fn choice_toggle(ui: &mut egui::Ui, selected: bool, label: impl AsRef<str>) -> Response {
    let mut text = RichText::new(label.as_ref()).color(if selected {
        theme::toggle_selected_text()
    } else {
        theme::text_secondary()
    });
    if selected {
        text = text.strong();
    }
    ui.scope(|ui| {
        let visuals = &mut ui.style_mut().visuals;
        if selected {
            // 选中态走 egui 的 selectable 通道：它会把 selection.bg_fill 作为
            // 按钮底色、selection.stroke 的颜色作为文字色。
            visuals.selection.bg_fill = theme::toggle_selected_bg();
            visuals.selection.stroke = Stroke::new(1.0, theme::toggle_selected_text());
        } else {
            visuals.widgets.inactive.weak_bg_fill = theme::bg_tertiary();
            visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, theme::border());
            visuals.widgets.hovered.weak_bg_fill = theme::bg_hover();
            visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, theme::border_light());
            visuals.widgets.active.weak_bg_fill = theme::widget_active_weak();
        }
        let button = egui::Button::selectable(selected, text)
            .frame_when_inactive(true)
            .corner_radius(CARD_RADIUS)
            .min_size(egui::vec2(56.0, CONTROL_HEIGHT));
        let button = if selected {
            button.stroke(Stroke::new(1.0, theme::toggle_selected_border()))
        } else {
            button
        };
        ui.add(button)
    })
    .inner
}

pub fn segmented_toggle(
    ui: &mut egui::Ui,
    selected: &mut bool,
    off_label: &str,
    on_label: &str,
) -> Response {
    let label = if *selected { on_label } else { off_label };
    let response = choice_toggle(ui, *selected, label);
    if response.clicked() {
        *selected = !*selected;
    }
    response
}

/// 列表类面板（终端 / 日志）的空状态：内容高度必须小于视口，否则
/// 自动跟随会在「内容比视口高」时每帧请求滚动修复，形成持续重绘。
/// 这里用更小的上下留白与图标，并配合调用方的 `set_min_height` 铺满视口。
pub fn empty_state_compact_with_hint(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    title: impl AsRef<str>,
    hint: impl AsRef<str>,
) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.add_space(8.0);
        ui.label(icon_only(icon, theme::text_dimmed(), 30.0));
        ui.add_space(6.0);
        ui.label(
            RichText::new(title.as_ref())
                .size(14.5)
                .strong()
                .color(theme::text_primary()),
        );
        let hint = hint.as_ref();
        if !hint.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(hint)
                    .size(12.5)
                    .color(theme::text_secondary()),
            );
        }
        ui.add_space(8.0);
    });
}

/// 自动跟随生效时，把滚动位置钉到内容末尾。
///
/// 终端与日志两个面板原本各有一份**逐字节相同**的实现（只差函数名）。留两份的意义
/// 只是让改一处忘一处：这条路径依赖"在当前位置分配一个零尺寸锚点再滚到它"，
/// 任何一份被单独改动都会让两个面板的自动跟随行为静默分叉。
pub(crate) fn scroll_to_bottom_if_requested(ui: &mut egui::Ui, force_scroll_to_bottom: bool) {
    if force_scroll_to_bottom {
        let (rect, _sense) = ui.allocate_exact_size(egui::vec2(0.0, 0.0), egui::Sense::hover());
        ui.scroll_to_rect(rect, Some(egui::Align::BOTTOM));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};
    use egui_material_icons::icons::{ICON_CHECK, ICON_DELETE_SWEEP};
    use std::{cell::Cell, rc::Rc};

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

    #[test]
    fn two_step_confirm_needs_a_second_click_inside_the_timeout() {
        let mut state = TwoStepConfirm::default();
        assert!(!state.click(1_000), "首次点击只进入确认态");
        assert!(state.is_armed(1_000));

        assert!(state.click(2_000), "超时内的第二次点击才算确认");
        assert!(!state.is_armed(2_000), "确认后必须解除确认态");
    }

    #[test]
    fn two_step_confirm_expires_without_a_second_click() {
        let mut state = TwoStepConfirm::default();
        state.click(0);

        assert!(
            !state.is_armed(TwoStepConfirm::TIMEOUT_MS),
            "超过超时窗口后确认态必须失效"
        );
        assert!(
            !state.click(TwoStepConfirm::TIMEOUT_MS),
            "失效后的点击重新进入确认态，而不是直接执行"
        );
    }

    #[test]
    fn confirm_button_keeps_a_stable_width_and_offers_no_cancel_widget() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 120.0))
            .build_ui(|ui| {
                egui_material_icons::initialize(ui.ctx());
                ui.horizontal_wrapped(|ui| {
                    let armed = confirm_armed(ui, "demo-confirm");
                    let response = button(
                        ui,
                        ICON_DELETE_SWEEP,
                        if armed { "确认" } else { "清空" },
                        if armed {
                            ButtonKind::Danger
                        } else {
                            ButtonKind::Ghost
                        },
                    );
                    let clicked = response.clicked();
                    confirm_click(ui, "demo-confirm", clicked);
                });
            });
        harness.run();

        let idle = harness.get_by_label_contains("清空").rect();
        harness.get_by_label_contains("清空").click();
        harness.run();

        let armed = harness
            .query_by_label_contains("确认")
            .expect("首次点击后进入确认态");
        // 关键是「不变宽」：变宽才会把窄工具栏挤到下一行。测试环境没有中日韩
        // 字体，两个全角字走回退字形时会有几像素差异，因此允许极小抖动。
        assert!(
            armed.rect().width() <= idle.width() + 4.0,
            "确认态不得让按钮变宽：idle={} armed={}",
            idle.width(),
            armed.rect().width()
        );
        assert!(
            harness.query_by_label_contains("取消").is_none(),
            "确认态不能再插入「取消」按钮，否则窄面板下工具栏会重新换行"
        );
    }

    #[test]
    fn confirm_button_is_dismissed_by_escape_and_by_other_clicks() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 120.0))
            .build_ui(|ui| {
                egui_material_icons::initialize(ui.ctx());
                ui.horizontal_wrapped(|ui| {
                    let armed = confirm_armed(ui, "demo-confirm");
                    let response = button(
                        ui,
                        ICON_DELETE_SWEEP,
                        if armed { "确认" } else { "清空" },
                        ButtonKind::Ghost,
                    );
                    let clicked = response.clicked();
                    confirm_click(ui, "demo-confirm", clicked);
                    button(ui, ICON_CHECK, "别处", ButtonKind::Ghost);
                });
            });
        harness.run();

        harness.get_by_label_contains("清空").click();
        harness.run();
        assert!(
            harness.query_by_label_contains("确认").is_some(),
            "首次点击进入确认态"
        );

        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(
            harness.query_by_label_contains("清空").is_some(),
            "Esc 应解除确认态"
        );

        harness.get_by_label_contains("清空").click();
        harness.run();
        assert!(
            harness.query_by_label_contains("确认").is_some(),
            "Esc 解除后再次点击应重新进入确认态"
        );

        harness.get_by_label_contains("别处").click();
        harness.run();
        assert!(
            harness.query_by_label_contains("清空").is_some(),
            "点击别处应解除确认态"
        );
    }

    #[test]
    fn badge_wraps_as_one_atom_instead_of_stacking_characters() {
        let measured = Rc::new(Cell::new(egui::Vec2::ZERO));
        let measured_in_ui = Rc::clone(&measured);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(240.0, 100.0))
            .build_ui(move |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("运行时 lua API 0.1 权限");
                    measured_in_ui.set(
                        badge(ui, "fs.read.user_selected", Color32::LIGHT_BLUE)
                            .rect
                            .size(),
                    );
                });
            });
        harness.run();

        let size = measured.get();
        assert!(size.x > 80.0, "badge was compressed to {size:?}");
        assert!(size.y <= 30.0, "badge text wrapped vertically: {size:?}");
    }

    #[test]
    fn sized_status_pills_have_identical_dimensions() {
        let measured = Rc::new(std::cell::RefCell::new(Vec::new()));
        let measured_in_ui = Rc::clone(&measured);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(420.0, 80.0))
            .build_ui(move |ui| {
                measured_in_ui.borrow_mut().clear();
                ui.horizontal(|ui| {
                    for label in ["已发现 5", "运行 1", "未运行 4"] {
                        measured_in_ui.borrow_mut().push(
                            status_pill_sized(ui, Color32::LIGHT_BLUE, label, 104.0)
                                .rect
                                .size(),
                        );
                    }
                });
            });
        harness.run();

        let measured = measured.borrow();
        assert_eq!(measured.len(), 3);
        for size in measured.iter().skip(1) {
            assert!((size.x - measured[0].x).abs() < 0.1);
            assert!((size.y - measured[0].y).abs() < 0.1);
        }
    }
}
