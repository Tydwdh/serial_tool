//! 命令面板（Ctrl+K）：fuzzy 搜索内置 Action + 插件命令 + 最近发送历史。

use crate::app::WorkbenchApp;
use crate::keymap::Action;
use eframe::egui;
use tool_panels::theme;

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
    /// 发送历史条目（执行走 do_send）。
    History(String),
}

impl WorkbenchApp {
    pub(crate) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.command_palette_open {
            return;
        }

        // Esc 关闭（独立于 keymap 录制，且不依赖 Action::Send 路径）。
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.command_palette_open = false;
            return;
        }

        // 构建候选列表：内置 Action + 插件命令 + 最近发送历史。
        let mut entries: Vec<CommandEntry> = Vec::new();

        for action in Action::ALL {
            // CommandPalette 自身不列入（避免递归打开）。
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

        for summary in self.plugin_manager.summaries() {
            for cmd in &summary.contributes.commands {
                entries.push(CommandEntry {
                    label: format!("{}: {}", summary.name, cmd.title),
                    shortcut: String::new(),
                    kind: CommandKind::PluginCommand(summary.id.clone(), cmd.id.clone()),
                });
            }
        }

        // 最近发送历史（最多 10 条）。
        for item in self.send.send_history.iter().take(10) {
            entries.push(CommandEntry {
                label: format!("发送: {}", truncate_for_display(item, 40)),
                shortcut: String::new(),
                kind: CommandKind::History(item.clone()),
            });
        }

        // 过滤：query 为空时全部显示；否则子串匹配（大小写不敏感）。
        let query = self.command_palette_query.to_lowercase();
        if !query.is_empty() {
            entries.retain(|e| e.label.to_lowercase().contains(&query));
        }

        let mut action_to_run: Option<CommandKind> = None;
        let mut close_after = false;

        egui::Window::new("命令面板")
            .open(&mut self.command_palette_open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -100.0])
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.set_max_width(520.0);

                // 搜索框：自动获取焦点。
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.command_palette_query)
                        .hint_text("搜索命令或历史…")
                        .desired_width(f32::INFINITY),
                );
                if !resp.has_focus() {
                    resp.request_focus();
                }

                ui.separator();

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        if entries.is_empty() {
                            ui.label(
                                egui::RichText::new("无匹配命令").color(theme::TEXT_SECONDARY),
                            );
                        }
                        for entry in &entries {
                            let resp = ui.horizontal(|ui| {
                                ui.label(&entry.label);
                                if !entry.shortcut.is_empty() {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                egui::RichText::new(&entry.shortcut)
                                                    .small()
                                                    .color(theme::TEXT_SECONDARY),
                                            );
                                        },
                                    );
                                }
                            });
                            if resp.response.clicked() {
                                action_to_run = Some(clone_kind(&entry.kind));
                            }
                        }
                    });

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("↑↓ 选择 · Enter 执行 · Esc 关闭")
                            .small()
                            .color(theme::TEXT_SECONDARY),
                    );
                });
            });

        // Enter 执行第一个候选。
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !entries.is_empty() {
            action_to_run = Some(clone_kind(&entries[0].kind));
        }

        if let Some(kind) = action_to_run {
            self.run_command_kind(kind);
            close_after = true;
        }

        if close_after {
            self.command_palette_open = false;
        }
    }

    fn run_command_kind(&mut self, kind: CommandKind) {
        match kind {
            CommandKind::Action(action) => {
                // 通过 pending_action 走已有 execute_action 路径，避免跨模块调用私有方法。
                self.pending_action = Some(action);
            }
            CommandKind::PluginCommand(plugin_id, command_id) => {
                self.publish_plugin_command_action(&plugin_id, &command_id);
            }
            CommandKind::History(text) => {
                self.send.input = text;
                self.do_send();
            }
        }
    }
}

/// 克隆 CommandKind（避免在点击响应中持有 entries 的引用）。
fn clone_kind(kind: &CommandKind) -> CommandKind {
    match kind {
        CommandKind::Action(a) => CommandKind::Action(a.clone()),
        CommandKind::PluginCommand(p, c) => {
            CommandKind::PluginCommand(p.clone(), c.clone())
        }
        CommandKind::History(s) => CommandKind::History(s.clone()),
    }
}

fn truncate_for_display(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push('…');
    }
    out
}
