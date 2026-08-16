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
use tool_panels::{design, theme};

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
    pub(crate) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette.open {
            return;
        }

        // Esc 关闭
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.command_palette.open = false;
            return;
        }

        // 构建候选列表：统一命令注册表（内置 + 插件命令同表）
        let mut entries: Vec<CommandEntry> = Vec::new();

        for command in self.commands.all() {
            if command.id == CMD_COMMAND_PALETTE {
                continue;
            }
            let shortcut = self
                .keymap
                .get_bindings(&command.id)
                .first()
                .map(|b| b.display())
                .unwrap_or_default();
            entries.push(CommandEntry {
                id: command.id.clone(),
                icon: command.icon,
                label: command.title.clone(),
                shortcut,
                category: command.category,
            });
        }

        // 过滤（普通词字面量 / re: 正则）
        let query = tool_panels::SearchQuery::new(&self.command_palette.query, false);
        if !query.is_empty() {
            entries.retain(|e| query.matches(&e.label));
        }

        // 排序：先按分类分组，组内按最近使用（usage_order 中 position 越小越靠前）。
        // 未使用的条目排组内最后（保持注册顺序，稳定排序）。
        entries.sort_by_key(|e| {
            let usage = self
                .command_palette
                .usage_order
                .iter()
                .position(|u| u == &e.label)
                .map(|p| p as i32)
                .unwrap_or(i32::MAX);
            (e.category, usage)
        });

        // 键盘导航：搜索文字变化时重置选中到第一项
        // 用 UI memory 追踪上次 query，检测变化
        {
            let last_query = ctx.memory_mut(|m| {
                let key = egui::Id::new("cp_last_query");
                let prev: String = m.data.get_temp(key).unwrap_or_default();
                m.data.insert_temp(key, self.command_palette.query.clone());
                prev
            });
            if last_query != self.command_palette.query && !entries.is_empty() {
                self.command_palette.selected = Some(0);
            }
        }

        // 确保 selected 在有效范围
        if let Some(idx) = self.command_palette.selected {
            if entries.is_empty() {
                self.command_palette.selected = None;
            } else if idx >= entries.len() {
                self.command_palette.selected = Some(entries.len() - 1);
            }
        } else if !entries.is_empty() {
            // 刚打开时默认选中第一项
            self.command_palette.selected = Some(0);
        }

        // 处理键盘 ↑↓
        if !entries.is_empty() {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                let cur = self.command_palette.selected.unwrap_or(0);
                self.command_palette.selected =
                    Some(if cur + 1 >= entries.len() { 0 } else { cur + 1 });
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                let cur = self.command_palette.selected.unwrap_or(0);
                self.command_palette.selected =
                    Some(if cur == 0 { entries.len() - 1 } else { cur - 1 });
            }
        }

        let mut command_to_run: Option<(String, String)> = None;
        let mut close_after = false;
        let mut hovered_idx: Option<usize> = None;

        let window_resp = egui::Window::new("命令面板")
            .open(&mut self.command_palette.open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -100.0])
            .min_width(440.0)
            .frame(design::elevated_card())
            .show(ctx, |ui| {
                // 搜索框
                let resp = ui
                    .horizontal(|ui| {
                        ui.label(design::icon_only(
                            ICON_SEARCH,
                            theme::text_secondary(),
                            20.0,
                        ));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.command_palette.query)
                                .hint_text("搜索命令…")
                                .desired_width(f32::INFINITY)
                                .frame(egui::Frame::NONE),
                        )
                    })
                    .inner;
                if !resp.has_focus() {
                    resp.request_focus();
                }

                ui.separator();

                let font_size = ui
                    .style()
                    .text_styles
                    .get(&egui::TextStyle::Body)
                    .map(|f| f.size)
                    .unwrap_or(14.0);
                let row_height = (font_size + 14.0).max(32.0);
                // 鼠标是否在移动（用于判断是否跟随 hover）
                let mouse_moving = ctx.input(|i| i.pointer.delta().length_sq() > 0.01);

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
                        for (i, entry) in entries.iter().enumerate() {
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

                            let is_selected = self.command_palette.selected == Some(i);

                            let row_id = ui.id().with(("cp_row", i));
                            let row_resp = ui.allocate_ui_with_layout(
                                egui::vec2(col_width, row_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let rect = ui.max_rect();
                                    let resp = ui.interact(rect, row_id, egui::Sense::click());
                                    // 仅鼠标移动时才让 hover 跟随选中，
                                    // 否则保持键盘选中优先。
                                    if resp.hovered() && mouse_moving {
                                        hovered_idx = Some(i);
                                    }
                                    if resp.hovered() || is_selected {
                                        let color = if is_selected {
                                            theme::bg_selection()
                                        } else {
                                            theme::bg_hover()
                                        };
                                        ui.painter().rect_filled(rect, 3.0, color);
                                    }
                                    if resp.clicked() {
                                        command_to_run =
                                            Some((entry.id.clone(), entry.label.clone()));
                                        close_after = true;
                                    }

                                    ui.add_space(4.0);

                                    ui.label(design::icon_only(
                                        entry.icon,
                                        if is_selected {
                                            theme::blue()
                                        } else {
                                            theme::text_dimmed()
                                        },
                                        17.0,
                                    ));

                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&entry.label)
                                                .color(theme::text_primary()),
                                        )
                                        .selectable(false),
                                    );

                                    if !entry.shortcut.is_empty() {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.add_space(4.0);
                                                design::badge(
                                                    ui,
                                                    &entry.shortcut,
                                                    theme::text_secondary(),
                                                );
                                            },
                                        );
                                    }
                                },
                            );
                            let _ = row_resp;
                        }
                    });

                // 鼠标移动时 hover 跟随更新键盘选中
                if let Some(hi) = hovered_idx
                    && mouse_moving
                {
                    self.command_palette.selected = Some(hi);
                }
            });

        // 点击窗口外部 → 关闭（VSCode 风格）
        if let Some(inner) = window_resp {
            let win_rect = inner.response.rect;
            if ctx.input(|i| {
                i.pointer.any_click()
                    && i.pointer
                        .hover_pos()
                        .is_some_and(|pos| !win_rect.contains(pos))
            }) {
                close_after = true;
            }
        }

        // Enter 执行选中项。
        // 注意：查询为空且用户未用方向键选中任何条目时，Enter 不应执行——
        // 否则误开面板后随手回车会触发最近使用的命令。
        let query_empty = self.command_palette.query.trim().is_empty();
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(idx) = self.command_palette.selected {
                if idx < entries.len() {
                    command_to_run = Some((entries[idx].id.clone(), entries[idx].label.clone()));
                    close_after = true;
                }
            } else if !query_empty && !entries.is_empty() {
                command_to_run = Some((entries[0].id.clone(), entries[0].label.clone()));
                close_after = true;
            }
        }

        if let Some((command_id, key)) = command_to_run {
            // 记录使用：key 移到最前
            self.command_palette.usage_order.retain(|u| u != &key);
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
}
