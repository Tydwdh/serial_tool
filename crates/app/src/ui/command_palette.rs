//! 命令面板（Ctrl+K）：搜索并执行统一命令（内置 + 插件）。
//!
//! 候选列表来自 [`crate::command_registry::CommandRegistry`]，快捷键、命令
//! 面板、插件命令共用同一份命令元数据与执行入口。支持鼠标悬浮高亮、点击
//! 执行，以及键盘 ↑↓ 选择、Enter 确认。

use crate::app::WorkbenchApp;
use crate::command_registry::{CMD_COMMAND_PALETTE, CommandCategory};
use eframe::egui;
use egui_material_icons::{
    MaterialIcon,
    icons::{ICON_SEARCH, ICON_SEARCH_OFF},
};
use tool_panels::{SearchQuery, design, theme};

/// 命令面板运行时状态。
#[derive(Default)]
pub(crate) struct CommandPaletteState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) selected: Option<usize>,
    pub(crate) usage_order: Vec<String>,
}

/// 命令面板的一条候选。
struct CommandEntry {
    id: String,
    icon: MaterialIcon,
    label: String,
    shortcut: String,
    category: CommandCategory,
}

impl WorkbenchApp {
    /// 命令面板的一帧：候选列表 → 选中同步 → 窗口渲染 → 派发执行。
    pub(crate) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette.open {
            return;
        }

        // Esc 关闭
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.command_palette.open = false;
            return;
        }

        // 搜索词只编译一次：候选过滤与搜索框的 hover 提示共用同一份判定
        // （普通词字面量 / re: 正则）。
        let query = SearchQuery::new(&self.command_palette.query, false);

        // 候选列表：统一命令注册表（内置 + 插件命令同表）
        let mut entries = self.palette_candidates();
        if !query.is_empty() {
            entries.retain(|entry| query.matches(&entry.label));
        }
        self.sort_palette_entries(&mut entries);

        // 键盘导航：搜索文字变化重置选中 → 夹到有效范围 → 处理 ↑↓
        let entry_count = entries.len();
        self.reset_selection_on_query_change(ctx, entry_count);
        self.clamp_selection(entry_count);
        self.move_selection_by_arrow_keys(ctx, entry_count);

        // 渲染窗口：点击候选行执行、点击窗口外部关闭
        let (mut command_to_run, mut close_after) =
            self.render_palette_window(ctx, &entries, &query);

        // Enter 执行选中项。
        // 注意：查询为空且用户未用方向键选中任何条目时，Enter 不应执行——
        // 否则误开面板后随手回车会触发最近使用的命令。
        if let Some(picked) = self.enter_confirmed_command(ctx, &entries) {
            command_to_run = Some(picked);
            close_after = true;
        }

        if let Some((command_id, key)) = command_to_run {
            // 记录使用：key 移到最前
            self.command_palette.usage_order.retain(|used| used != &key);
            self.command_palette.usage_order.insert(0, key);
            close_after = true;

            // 与快捷键一致：统一经 pending_command 在下一帧 tick_pre_ui 执行，
            // 避免在 UI 渲染中途变更状态。内置/插件命令都走同一入口。
            self.pending_command = Some(command_id);
        }

        if close_after {
            self.command_palette.open = false;
            self.command_palette.selected = None;
        }
    }

    /// 构建候选：注册表里的每条命令（面板自身除外）带上展示标题与首条快捷键。
    fn palette_candidates(&self) -> Vec<CommandEntry> {
        self.commands
            .all()
            .iter()
            .filter(|command| command.id != CMD_COMMAND_PALETTE)
            .map(|command| CommandEntry {
                id: command.id.clone(),
                icon: command.icon,
                label: command.title.clone(),
                shortcut: self
                    .keymap
                    .get_bindings(&command.id)
                    .first()
                    .map(|binding| binding.display())
                    .unwrap_or_default(),
                category: command.category,
            })
            .collect()
    }

    /// 排序：先按分类分组，组内按最近使用（usage_order 中 position 越小越靠前）。
    /// 未使用的条目排组内最后（保持注册顺序，稳定排序）。
    fn sort_palette_entries(&self, entries: &mut [CommandEntry]) {
        entries.sort_by_key(|entry| {
            let usage = self
                .command_palette
                .usage_order
                .iter()
                .position(|used| used == &entry.label)
                .map(|position| position as i32)
                .unwrap_or(i32::MAX);
            (entry.category, usage)
        });
    }

    /// 搜索文字变化时把选中重置到第一项。
    ///
    /// 用 UI memory 追踪上次 query，检测变化——面板状态本身不带这份记忆。
    fn reset_selection_on_query_change(&mut self, ctx: &egui::Context, entry_count: usize) {
        let current_query = self.command_palette.query.clone();
        let last_query = ctx.memory_mut(|memory| {
            let key = egui::Id::new("cp_last_query");
            let prev: String = memory.data.get_temp(key).unwrap_or_default();
            memory.data.insert_temp(key, current_query);
            prev
        });
        if last_query != self.command_palette.query && entry_count > 0 {
            self.command_palette.selected = Some(0);
        }
    }

    /// 确保 selected 在有效范围。
    fn clamp_selection(&mut self, entry_count: usize) {
        if let Some(index) = self.command_palette.selected {
            if entry_count == 0 {
                self.command_palette.selected = None;
            } else if index >= entry_count {
                self.command_palette.selected = Some(entry_count - 1);
            }
        } else if entry_count > 0 {
            // 刚打开时默认选中第一项
            self.command_palette.selected = Some(0);
        }
    }

    /// 键盘 ↑↓ 循环选择（列表为空时不响应）。
    fn move_selection_by_arrow_keys(&mut self, ctx: &egui::Context, entry_count: usize) {
        if entry_count == 0 {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
            let current = self.command_palette.selected.unwrap_or(0);
            self.command_palette.selected = Some(if current + 1 >= entry_count {
                0
            } else {
                current + 1
            });
        }
        if ctx.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
            let current = self.command_palette.selected.unwrap_or(0);
            self.command_palette.selected = Some(if current == 0 {
                entry_count - 1
            } else {
                current - 1
            });
        }
    }

    /// 渲染面板窗口。
    ///
    /// 返回 `(点击选中的命令 (id, 标题), 是否在本帧后关闭)`；点击窗口外部也算关闭。
    fn render_palette_window(
        &mut self,
        ctx: &egui::Context,
        entries: &[CommandEntry],
        query: &SearchQuery,
    ) -> (Option<(String, String)>, bool) {
        let mut command_to_run: Option<(String, String)> = None;
        let mut close_after = false;

        let window_response = egui::Window::new("命令面板")
            .open(&mut self.command_palette.open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -100.0])
            .min_width(440.0)
            .frame(design::elevated_card())
            .show(ctx, |ui| {
                render_palette_search_box(ui, &mut self.command_palette.query, query);

                ui.separator();

                // 鼠标是否在移动（用于判断是否跟随 hover）
                let mouse_moving = ctx.input(|input| input.pointer.delta().length_sq() > 0.01);
                let (hovered_index, clicked) =
                    render_palette_rows(ui, entries, self.command_palette.selected, mouse_moving);

                if let Some(command) = clicked {
                    command_to_run = Some(command);
                    close_after = true;
                }

                // 鼠标移动时 hover 跟随更新键盘选中
                if let Some(hovered) = hovered_index
                    && mouse_moving
                {
                    self.command_palette.selected = Some(hovered);
                }
            });

        // 点击窗口外部 → 关闭（VSCode 风格）
        if let Some(inner) = window_response {
            let win_rect = inner.response.rect;
            if ctx.input(|input| {
                input.pointer.any_click()
                    && input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pos| !win_rect.contains(pos))
            }) {
                close_after = true;
            }
        }

        (command_to_run, close_after)
    }

    /// Enter 要执行的命令：已选中项优先，无选中时只在有查询词的情况下取第一项。
    fn enter_confirmed_command(
        &self,
        ctx: &egui::Context,
        entries: &[CommandEntry],
    ) -> Option<(String, String)> {
        let query_empty = self.command_palette.query.trim().is_empty();
        if !ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            return None;
        }
        let index = match self.command_palette.selected {
            Some(index) if index < entries.len() => index,
            Some(_) => return None,
            None if !query_empty && !entries.is_empty() => 0,
            None => return None,
        };
        Some((entries[index].id.clone(), entries[index].label.clone()))
    }
}

/// 搜索框：打开面板即把焦点交给它，键盘才接得上。
///
/// hover 文案来自 [`SearchQuery::hover_hint`]，非法正则回退时换成提醒语。
fn render_palette_search_box(ui: &mut egui::Ui, text: &mut String, search: &SearchQuery) {
    let response = ui
        .horizontal(|ui| {
            ui.label(design::icon_only(
                ICON_SEARCH,
                theme::text_secondary(),
                20.0,
            ));
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text("搜索命令…")
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE),
            )
            .on_hover_text(SearchQuery::hover_hint(
                search.used_invalid_regex_fallback(),
            ))
        })
        .inner;
    if !response.has_focus() {
        response.request_focus();
    }
}

/// 渲染候选列表（分类 header + 行）。
///
/// 返回 `(鼠标停留的行, 被点击执行的命令 (id, 标题))`。
fn render_palette_rows(
    ui: &mut egui::Ui,
    entries: &[CommandEntry],
    selected: Option<usize>,
    mouse_moving: bool,
) -> (Option<usize>, Option<(String, String)>) {
    let font_size = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .map(|style| style.size)
        .unwrap_or(14.0);
    let row_height = (font_size + 14.0).max(32.0);

    let mut hovered_index: Option<usize> = None;
    let mut command_to_run: Option<(String, String)> = None;

    egui::ScrollArea::vertical()
        .max_height(300.0)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let col_width = ui.available_width();

            if entries.is_empty() {
                design::empty_state(ui, ICON_SEARCH_OFF, "无匹配命令");
            }

            // 分类 header：分组变化时渲染一次
            let mut last_category: Option<CommandCategory> = None;
            for (index, entry) in entries.iter().enumerate() {
                if last_category != Some(entry.category) {
                    last_category = Some(entry.category);
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(entry.category.title())
                            .small()
                            .color(theme::text_secondary()),
                    );
                    ui.add_space(2.0);
                }

                let is_selected = selected == Some(index);
                let (hovered, clicked) =
                    render_command_row(ui, entry, index, is_selected, row_height, col_width);
                // 仅鼠标移动时才让 hover 跟随选中，
                // 否则保持键盘选中优先。
                if hovered && mouse_moving {
                    hovered_index = Some(index);
                }
                if let Some(command) = clicked {
                    command_to_run = Some(command);
                }
            }
        });

    (hovered_index, command_to_run)
}

/// 渲染一条候选行：命中底色 + 图标 + 标题 + 右对齐的快捷键徽标。
///
/// 返回 `(是否被鼠标悬浮, 被点击时要执行的命令 (id, 标题))`。
fn render_command_row(
    ui: &mut egui::Ui,
    entry: &CommandEntry,
    index: usize,
    is_selected: bool,
    row_height: f32,
    col_width: f32,
) -> (bool, Option<(String, String)>) {
    let mut hovered = false;
    let mut command_to_run: Option<(String, String)> = None;

    let row_id = ui.id().with(("cp_row", index));
    ui.allocate_ui_with_layout(
        egui::vec2(col_width, row_height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let rect = ui.max_rect();
            let response = ui.interact(rect, row_id, egui::Sense::click());
            hovered = response.hovered();
            if response.clicked() {
                command_to_run = Some((entry.id.clone(), entry.label.clone()));
            }
            if hovered || is_selected {
                let color = if is_selected {
                    theme::bg_selection()
                } else {
                    theme::bg_hover()
                };
                ui.painter().rect_filled(rect, 3.0, color);
            }

            ui.add_space(4.0);

            let icon_color = if is_selected {
                theme::blue()
            } else {
                theme::text_dimmed()
            };
            ui.label(design::icon_only(entry.icon, icon_color, 17.0));

            ui.add(
                egui::Label::new(egui::RichText::new(&entry.label).color(theme::text_primary()))
                    .selectable(false),
            );

            if !entry.shortcut.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(4.0);
                    design::badge(ui, &entry.shortcut, theme::text_secondary());
                });
            }
        },
    );

    (hovered, command_to_run)
}
