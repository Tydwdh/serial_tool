use egui::Ui;
use tool_platform::NetworkSerialConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkSerialAction {
    Submit(NetworkSerialConfig),
    Error(String),
}

pub struct NetworkSerialFormView<'a> {
    pub host: &'a mut String,
    pub port: &'a mut String,
    pub api_key: &'a mut String,
}

/// Shared network-serial connection form. The application roots own the
/// resulting configuration and decide whether it is persisted or connected.
pub fn network_serial_form_ui(
    ui: &mut Ui,
    view: &mut NetworkSerialFormView<'_>,
) -> Vec<NetworkSerialAction> {
    let mut actions = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.label("网络");
        ui.add(
            egui::TextEdit::singleline(view.host)
                .desired_width((ui.available_width() - 300.0).clamp(140.0, 220.0))
                .hint_text("IP 或主机名"),
        );
        ui.add(
            egui::TextEdit::singleline(view.port)
                .desired_width(64.0)
                .hint_text("7125"),
        );
        ui.add(
            egui::TextEdit::singleline(view.api_key)
                .desired_width(150.0)
                .password(true)
                .hint_text("API Key（可选）"),
        );
        if ui.button("添加并连接").clicked() {
            let host = view.host.trim().to_owned();
            if host.is_empty() {
                actions.push(NetworkSerialAction::Error(
                    "请输入服务器 IP 或主机名".to_owned(),
                ));
                return;
            }
            let Ok(port) = view.port.trim().parse::<u16>() else {
                actions.push(NetworkSerialAction::Error(
                    "网络端口格式错误（1-65535）".to_owned(),
                ));
                return;
            };
            if port == 0 {
                actions.push(NetworkSerialAction::Error(
                    "网络端口格式错误（1-65535）".to_owned(),
                ));
                return;
            }
            actions.push(NetworkSerialAction::Submit(NetworkSerialConfig {
                host,
                port,
                api_key: (!view.api_key.trim().is_empty()).then(|| view.api_key.trim().to_owned()),
            }));
        }
    });
    actions
}
