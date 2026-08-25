//! 统一命令模型（CommandRegistry）。
//!
//! 所有可触发行为——内置命令、插件命令——收敛为同一种 [`Command`]，由
//! [`CommandRegistry`] 统一管理。快捷键、命令面板、设置页快捷键编辑器
//! 都只面对「命令 ID → 元数据 + 分发表」，不再区分 built-in / plugin。
//!
//! # 命令 ID（= Keymap 持久化键）
//!
//! - 内置命令：`$` 前缀，如 `$RefreshPorts`；
//! - 插件命令：`plugin_id:command_id`。
//!
//! 该格式与既有 `workspace.json` 中的快捷键映射完全一致，无需配置迁移。

use crate::app::WorkbenchApp;
use egui_material_icons::{
    MaterialIcon,
    icons::{ICON_BOLT, ICON_EXTENSION},
};
use tool_application::api::extension::PluginSummary;

// ── 内置命令 ID 常量（Keymap 持久化键的单一来源）──

pub(crate) const CMD_REFRESH_PORTS: &str = "$RefreshPorts";
pub(crate) const CMD_OPEN_PORT: &str = "$OpenPort";
pub(crate) const CMD_TOGGLE_BOTTOM_PANEL: &str = "$ToggleBottomPanel";
pub(crate) const CMD_TOGGLE_RIGHT_DOCK: &str = "$ToggleRightDock";
pub(crate) const CMD_SEND: &str = "$Send";
pub(crate) const CMD_START_RECORDING: &str = "$StartRecording";
pub(crate) const CMD_RECONNECT_PORT: &str = "$ReconnectPort";
pub(crate) const CMD_ADD_BOOKMARK: &str = "$AddBookmark";
pub(crate) const CMD_COMMAND_PALETTE: &str = "$CommandPalette";
pub(crate) const CMD_CLEAR_TERMINAL: &str = "$ClearTerminal";

/// 命令分类（命令面板按此分组显示）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CommandCategory {
    Serial,
    Send,
    Recorder,
    View,
    System,
    Plugin,
}

impl CommandCategory {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Serial => "串口",
            Self::Send => "发送",
            Self::Recorder => "录制",
            Self::View => "视图",
            Self::System => "系统",
            Self::Plugin => "插件",
        }
    }
}

/// 命令执行分派：分两类
/// - AppCommand：业务行为，经 Workbench::dispatch
/// - UiCommand：纯 UI 操作，直接改 WorkbenchApp
#[derive(Clone)]
pub(crate) enum CommandHandler {
    App(tool_application::AppCommand),
    Ui(fn(&mut WorkbenchApp)),
    Plugin {
        plugin_id: String,
        command_id: String,
    },
}

impl CommandHandler {
    fn run(&self, app: &mut WorkbenchApp) {
        match self {
            Self::App(cmd) => {
                if let Err(e) = app.workbench.dispatch(cmd.clone()) {
                    app.notifications.push(
                        "command",
                        crate::state::StatusLevel::Error,
                        e.to_string(),
                    );
                }
            }
            Self::Ui(handler) => handler(app),
            Self::Plugin {
                plugin_id,
                command_id,
            } => app.publish_plugin_command_action(plugin_id, command_id),
        }
    }
}

/// 一条可执行命令。`id` 即 Keymap 持久化键。
#[derive(Clone)]
pub(crate) struct Command {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) icon: MaterialIcon,
    pub(crate) category: CommandCategory,
    handler: CommandHandler,
}

impl Command {
    fn builtin(
        id: &'static str,
        title: &'static str,
        category: CommandCategory,
        handler: fn(&mut WorkbenchApp),
    ) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            icon: ICON_BOLT,
            category,
            handler: CommandHandler::Ui(handler),
        }
    }

    fn app_builtin(
        id: &'static str,
        title: &'static str,
        category: CommandCategory,
        cmd: tool_application::AppCommand,
    ) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            icon: ICON_BOLT,
            category,
            handler: CommandHandler::App(cmd),
        }
    }
}

/// 命令注册表：内置命令构造时注册；插件命令随插件启停动态增删。
#[derive(Default)]
pub(crate) struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// 注册全部内置命令。
    pub(crate) fn builtin() -> Self {
        let mut registry = Self::default();
        registry.add(Command::app_builtin(
            CMD_REFRESH_PORTS,
            "刷新串口",
            CommandCategory::Serial,
            tool_application::AppCommand::RefreshPorts,
        ));
        registry.add(Command::builtin(
            CMD_OPEN_PORT,
            "打开/关闭串口",
            CommandCategory::Serial,
            WorkbenchApp::toggle_selected_port,
        ));
        registry.add(Command::builtin(
            CMD_RECONNECT_PORT,
            "重连串口",
            CommandCategory::Serial,
            WorkbenchApp::reconnect_selected_port,
        ));
        registry.add(Command::builtin(
            CMD_SEND,
            "发送",
            CommandCategory::Send,
            WorkbenchApp::cmd_send_if_ready,
        ));
        registry.add(Command::app_builtin(
            CMD_CLEAR_TERMINAL,
            "清空终端",
            CommandCategory::Send,
            tool_application::AppCommand::ClearTerminal,
        ));
        registry.add(Command::app_builtin(
            CMD_START_RECORDING,
            "开始/停止录制",
            CommandCategory::Recorder,
            tool_application::AppCommand::StopRecording,
        ));
        registry.add(Command::app_builtin(
            CMD_ADD_BOOKMARK,
            "添加录制标记",
            CommandCategory::Recorder,
            tool_application::AppCommand::AddBookmark { name: None },
        ));
        registry.add(Command::builtin(
            CMD_TOGGLE_BOTTOM_PANEL,
            "切换底部面板",
            CommandCategory::View,
            WorkbenchApp::toggle_bottom_panel,
        ));
        registry.add(Command::builtin(
            CMD_TOGGLE_RIGHT_DOCK,
            "切换右侧边栏",
            CommandCategory::View,
            WorkbenchApp::cmd_toggle_right_dock,
        ));
        registry.add(Command::builtin(
            CMD_COMMAND_PALETTE,
            "命令面板",
            CommandCategory::System,
            WorkbenchApp::cmd_toggle_command_palette,
        ));
        registry
    }

    fn add(&mut self, command: Command) {
        self.commands.push(command);
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }

    pub(crate) fn all(&self) -> &[Command] {
        &self.commands
    }

    /// 插件命令随插件启停重建：先移除全部插件命令，再按当前 summaries 注册。
    /// 内置命令不受影响。
    pub(crate) fn rebuild_plugin_commands(&mut self, summaries: &[PluginSummary]) {
        self.commands
            .retain(|c| !matches!(&c.handler, CommandHandler::Plugin { .. }));
        for summary in summaries {
            for cmd in &summary.contributes.commands {
                self.commands.push(Command {
                    id: format!("{}:{}", summary.id, cmd.id),
                    title: format!("{}: {}", summary.name, cmd.title),
                    icon: ICON_EXTENSION,
                    category: CommandCategory::Plugin,
                    handler: CommandHandler::Plugin {
                        plugin_id: summary.id.clone(),
                        command_id: cmd.id.clone(),
                    },
                });
            }
        }
    }
}

impl WorkbenchApp {
    /// 执行命令：统一入口（快捷键、命令面板共用）。
    pub(crate) fn execute_command(&mut self, id: &str) {
        let Some(command) = self.commands.get(id).cloned() else {
            log::warn!("unknown command: {id}");
            return;
        };
        command.handler.run(self);
    }

    /// 命令标题（设置页/状态提示用）；未知 id 回退为 id 本身。
    pub(crate) fn command_label(&self, id: &str) -> String {
        self.commands
            .get(id)
            .map(|c| c.title.clone())
            .unwrap_or_else(|| id.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin_ids() -> Vec<String> {
        CommandRegistry::builtin()
            .all()
            .iter()
            .map(|c| c.id.clone())
            .collect()
    }

    #[test]
    fn builtin_ids_are_unique_and_prefixed() {
        let ids = builtin_ids();
        assert!(!ids.is_empty());
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "builtin command ids must be unique"
        );
        for id in &ids {
            assert!(id.starts_with('$'), "builtin id must use $ prefix: {id}");
        }
    }

    #[test]
    fn builtin_titles_are_unique_and_nonempty() {
        let registry = CommandRegistry::builtin();
        let titles: Vec<String> = registry.all().iter().map(|c| c.title.clone()).collect();
        assert!(titles.iter().all(|t| !t.is_empty()));
        let mut sorted = titles.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            titles.len(),
            "builtin command titles must be unique"
        );
    }

    #[test]
    fn get_hits_and_misses() {
        let registry = CommandRegistry::builtin();
        assert!(registry.get(CMD_REFRESH_PORTS).is_some());
        assert!(registry.get("$NoSuchCommand").is_none());
    }

    #[test]
    fn plugin_commands_rebuild_and_clear() {
        let mut registry = CommandRegistry::builtin();
        let builtin_count = registry.all().len();

        let summary = PluginSummary {
            id: "demo.test".to_owned(),
            name: "Demo Test".to_owned(),
            version: String::new(),
            api_version: String::new(),
            runtime: String::new(),
            state: tool_application::api::extension::PluginState::Running,
            permissions: Vec::new(),
            contributes: tool_application::api::extension::manifest::PluginContributes {
                commands: vec![tool_application::api::extension::manifest::PluginCommand {
                    id: "demo.test.run".to_owned(),
                    title: "Run".to_owned(),
                }],
                ..Default::default()
            },
            path: std::path::PathBuf::new(),
            last_error: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            category: None,
            icon: None,
            has_replay_analyzer: false,
            replay_subscriptions: Vec::new(),
            replay_outputs: Vec::new(),
            registered_commands: Vec::new(),
            missing_commands: Vec::new(),
            undeclared_commands: Vec::new(),
        };

        registry.rebuild_plugin_commands(&[summary]);
        assert_eq!(registry.all().len(), builtin_count + 1);
        let plugin_cmd = registry
            .get("demo.test:demo.test.run")
            .expect("plugin command registered");
        assert_eq!(plugin_cmd.title, "Demo Test: Run");

        // 空 summaries → 插件命令全部移除，内置保留
        registry.rebuild_plugin_commands(&[]);
        assert_eq!(registry.all().len(), builtin_count);
        assert!(registry.get("demo.test:demo.test.run").is_none());
        assert!(registry.get(CMD_REFRESH_PORTS).is_some());
    }
}
