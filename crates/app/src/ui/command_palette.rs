//! 命令面板（Ctrl+K）：搜索内置 Action 与插件命令。
//!
//! 支持鼠标悬浮高亮、点击执行，以及键盘 ↑↓ 选择、Enter 确认。

use crate::app::WorkbenchApp;
use crate::keymap::Action;
use eframe::egui;
use tool_panels::theme;

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
    label: String,
    shortcut: String,
    kind: CommandKind,
}

enum CommandKind {
    /// 内置 Action（执行走 execute_action）。
    Action(Action),
    /// 插件命令。
    PluginCommand(String, String),
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

        // 构建候选列表
        let mut entries: Vec<CommandEntry> = Vec::new();

        for action in Action::ALL {
            if matches!(action, Action::CommandPalette) {
                continue;
            }
            let shortcut = self
                .keymap
                .get_bindings(action)
                .first()
                .map(|b| b.display())
                .unwrap_or_default();
            entries.push(CommandEntry {
                label: action.label(),
                shortcut,
                kind: CommandKind::Action(action.clone()),
            });
        }

        for summary in self.plugin_summaries() {
            for cmd in &summary.contributes.commands {
                entries.push(CommandEntry {
                    label: format!("{}: {}", summary.name, cmd.title),
                    shortcut: String::new(),
                    kind: CommandKind::PluginCommand(summary.id.clone(), cmd.id.clone()),
                });
            }
        }

        // 过滤
        let query = self.command_palette.query.to_lowercase();
        if !query.is_empty() {
            entries.retain(|e| e.label.to_lowercase().contains(&query));
        }

        // 按最近使用排序：usage_order 中 position 越小的排越前面。
        // 未使用的条目排最后。
        entries.sort_by_key(|e| {
            self.command_palette
                .usage_order
                .iter()
                .position(|u| u == &e.label)
                .map(|p| p as i32)
                .unwrap_or(i32::MAX)
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

        let mut action_to_run: Option<(CommandKind, String)> = None;
        let mut close_after = false;
        let mut hovered_idx: Option<usize> = None;

        let window_resp = egui::Window::new("命令面板")
            .open(&mut self.command_palette.open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -100.0])
            .min_width(340.0)
            .show(ctx, |ui| {
                // 搜索框
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.command_palette.query)
                        .hint_text("搜索命令…")
                        .desired_width(f32::INFINITY),
                );
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
                let row_height = font_size + 6.0;
                // 鼠标是否在移动（用于判断是否跟随 hover）
                let mouse_moving = ctx.input(|i| i.pointer.delta().length_sq() > 0.01);

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let col_width = ui.available_width();

                        if entries.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new("无匹配命令")
                                        .color(theme::text_secondary()),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("按 Esc 关闭")
                                        .small()
                                        .color(theme::text_dimmed()),
                                );
                            });
                            ui.add_space(20.0);
                        }

                        for (i, entry) in entries.iter().enumerate() {
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
                                        action_to_run =
                                            Some((clone_kind(&entry.kind), entry.label.clone()));
                                        close_after = true;
                                    }

                                    ui.add_space(4.0);

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
                                                ui.label(
                                                    egui::RichText::new(&entry.shortcut)
                                                        .small()
                                                        .color(theme::text_secondary()),
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

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("↑↓ 选择 · Enter 执行 · Esc 关闭")
                            .small()
                            .color(theme::text_secondary()),
                    );
                });
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
                    action_to_run =
                        Some((clone_kind(&entries[idx].kind), entries[idx].label.clone()));
                    close_after = true;
                }
            } else if !query_empty && !entries.is_empty() {
                action_to_run = Some((clone_kind(&entries[0].kind), entries[0].label.clone()));
                close_after = true;
            }
        }

        if let Some((kind, key)) = action_to_run {
            // 记录使用：key 移到最前
            self.command_palette.usage_order.retain(|u| u != &key);
            self.command_palette.usage_order.insert(0, key);
            close_after = true;

            self.run_command_kind(kind);
        }

        if close_after {
            self.command_palette.open = false;
            self.command_palette.selected = None;
        }
    }

    fn run_command_kind(&mut self, kind: CommandKind) {
        match kind {
            CommandKind::Action(action) => {
                self.pending_action = Some(action);
            }
            CommandKind::PluginCommand(plugin_id, command_id) => {
                self.publish_plugin_command_action(&plugin_id, &command_id);
            }
        }
    }
}

/// 克隆 CommandKind（避免在点击响应中持有 entries 的引用）。
fn clone_kind(kind: &CommandKind) -> CommandKind {
    match kind {
        CommandKind::Action(a) => CommandKind::Action(a.clone()),
        CommandKind::PluginCommand(p, c) => CommandKind::PluginCommand(p.clone(), c.clone()),
    }
}
