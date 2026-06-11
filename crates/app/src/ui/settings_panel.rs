use crate::app::WorkbenchApp;
use eframe::egui;

impl WorkbenchApp {
    pub(crate) fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置");
        ui.separator();
        ui.heading("外观");
        ui.checkbox(&mut self.bottom_panel_visible, "底部面板");
        ui.checkbox(&mut self.panels.inspector_visible, "检查器");
        ui.separator();
        ui.heading("快捷键");
        ui.label("Ctrl+R 刷新  Ctrl+Shift+O 打开  Ctrl+B 底部  Ctrl+I 检查器  Ctrl+1~3 切换");
        ui.separator();
        ui.label("硬件调试工作台 v0.1.0");
    }
}
