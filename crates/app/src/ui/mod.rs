mod bottom_panel;
mod contributions;
mod device_panel;
mod dialogs;
mod settings_panel;
mod status_bar;
mod tiles;
pub(crate) mod toast;
mod top_bar;

use crate::app::WorkbenchApp;
use eframe::egui;

impl WorkbenchApp {
    pub(crate) fn draw_shell(&mut self, ui: &mut egui::Ui, _ctx: &egui::Context) {
        crate::shared_shell::show_shell(self, ui);
    }

    /// native 端口条目投影：顶栏与设备面板共用这一份映射。
    ///
    /// `kind` 与 `pending_reconnect` 都是 tool-panels 侧的可见判据（`kind == "网络"`
    /// 才给"移除网络端口"按钮，`kind` 非空才在 label 后加 ` · 类别`），同一个映射分写
    /// 两处就是让两个面板各自漂移。web 那两处不并进来：它们的 `kind` 展示规则不同
    /// （浏览器串口会写 `"Serial"`）。
    fn native_port_items(&self) -> Vec<tool_panels::SerialPortItem> {
        self.serial
            .ports
            .iter()
            .map(|port| {
                let status = self.workbench.transport_status(port.id.as_str());
                tool_panels::SerialPortItem {
                    id: port.id.to_string(),
                    label: port.label.clone(),
                    kind: port.detail.to_string(),
                    open: status.open,
                    connecting: status.connecting,
                    pending_reconnect: self.serial.reconnects.entry(port.id.as_str()).is_some(),
                }
            })
            .collect()
    }
}

impl crate::shared_shell::AppShellHost for WorkbenchApp {
    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        self.top_bar(ui);
    }

    fn render_status_bar(&mut self, ui: &mut egui::Ui) {
        self.status_bar(ui);
    }

    fn after_dock(&mut self, ui: &egui::Ui) {
        if self.layout_dirty && !ui.input(|input| input.pointer.primary_down()) {
            self.layout_dirty = false;
            if let Err(error) = self.save_config() {
                log::warn!("save_config failed: {error}");
            }
        }
    }
}
