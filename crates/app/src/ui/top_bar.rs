use eframe::egui;
use tool_transport::SerialPortDescriptor;

const SERIAL_ACTION_BUTTON_SIZE: egui::Vec2 = egui::vec2(52.0, 26.0);

pub(crate) fn serial_combo(
    ui: &mut egui::Ui,
    id: &'static str,
    w: f32,
    ports: &[SerialPortDescriptor],
    sel: &mut Option<String>,
) {
    let selected_text = sel
        .as_deref()
        .and_then(|name| {
            ports
                .iter()
                .find(|p| p.port_name == name)
                .map(|p| format!("{}  {}", p.port_name, p.port_type))
        })
        .unwrap_or_else(|| {
            if ports.is_empty() {
                "无端口".to_owned()
            } else {
                "请选择串口".to_owned()
            }
        });

    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            if ports.is_empty() {
                ui.add_enabled(false, egui::Label::new("无可用串口"));
            } else {
                for port in ports {
                    ui.selectable_value(
                        sel,
                        Some(port.port_name.clone()),
                        format!("{}  {}", port.port_name, port.port_type),
                    );
                }
            }
        });
}

pub(crate) fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    let r = [
        "9600", "19200", "38400", "57600", "115200", "230400", "460800", "921600",
    ];
    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(baud.clone())
        .show_ui(ui, |ui| {
            for x in r {
                ui.selectable_value(baud, x.to_owned(), x);
            }
        });
}

pub(crate) fn serial_action_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(SERIAL_ACTION_BUTTON_SIZE, egui::Button::new(text))
}

pub(crate) fn serial_action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &str,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(SERIAL_ACTION_BUTTON_SIZE),
    )
}

use tool_transport::{DataBits, Parity, StopBits};

pub(crate) fn pdb(v: &str) -> DataBits {
    match v {
        "5" => DataBits::Five,
        "6" => DataBits::Six,
        "7" => DataBits::Seven,
        _ => DataBits::Eight,
    }
}
pub(crate) fn psb(v: &str) -> StopBits {
    match v {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}
pub(crate) fn ppar(v: &str) -> Parity {
    match v {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}
