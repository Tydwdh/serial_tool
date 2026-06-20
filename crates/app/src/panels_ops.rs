//! `WorkbenchApp` 面板编排操作：底部面板开关、工作区加载后处理。
//!
//! 从 `commands.rs` 抽出的跨面板协调逻辑。

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;

impl WorkbenchApp {
    pub(crate) fn add_recent_workspace(&mut self, path: &std::path::Path) {
        let s = path.display().to_string();
        self.recent_workspaces.retain(|p| p != &s);
        self.recent_workspaces.insert(0, s);
        self.recent_workspaces.truncate(10);
    }

    pub(crate) fn apply_loaded_workspace_postprocess(&mut self) {
        self.panels.discard_dynamic_tabs();
        self.panels.dock.normalize_tool_layout();
        self.bottom_panel_visible = self.panels.dock.bottom_visible;
        self.refresh_ports_silent();
        self.dynamic_panels.set_ports(&self.serial.ports);
        self.send.target_port = None;
        self.ensure_send_target_port();
    }

    pub(crate) fn open_bottom_panel(&mut self) {
        self.set_bottom_visible(true);
        self.panels.dock.move_panel(
            tool_panels::PanelKind::Terminal,
            tool_panels::DockArea::Bottom,
        );
    }

    pub(crate) fn set_bottom_visible(&mut self, visible: bool) {
        self.bottom_panel_visible = visible;
        self.panels.dock.bottom_visible = visible;
    }

    pub(crate) fn toggle_bottom_panel(&mut self) {
        self.set_bottom_visible(!self.panels.dock.bottom_visible);

        if self.panels.dock.bottom_visible {
            self.set_status(StatusLevel::Info, "底部面板已打开");
        }
    }
}
