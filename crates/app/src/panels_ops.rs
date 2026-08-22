//! `WorkbenchApp` 面板编排操作：底部面板开关、工作区加载后处理。
//!
//! 从 `commands.rs` 抽出的跨面板协调逻辑。

use crate::app::WorkbenchApp;
use crate::state::StatusLevel;

impl WorkbenchApp {
    pub(crate) fn apply_loaded_workspace_postprocess(&mut self) {
        self.panels.discard_dynamic_tabs();
        self.panels.dock.normalize_tool_layout();
        self.panels.ensure_tiles_layout();
        self.refresh_ports_silent();
        self.dynamic_panels.set_ports(
            &self
                .serial
                .ports
                .iter()
                .map(|d| tool_panels::PortItem {
                    port_name: d.port_name.clone(),
                })
                .collect::<Vec<_>>(),
        );
        self.send.target_port = None;
        self.ensure_send_target_port();
    }

    pub(crate) fn set_bottom_visible(&mut self, visible: bool) {
        self.panels.set_bottom_visible(visible);
    }

    pub(crate) fn toggle_bottom_panel(&mut self) {
        let visible = self.panels.bottom_visible();
        self.set_bottom_visible(!visible);

        if self.panels.bottom_visible() {
            self.set_status(StatusLevel::Info, "底部面板已打开");
        }
    }
}
