//! Web V1 composition root.
//!
//! The first browser build intentionally contains only presentation and the
//! data path that is already platform independent. Native-only services are
//! not constructed here: updater, marketplace, Lua plugins, recorder and
//! replay will be added behind capabilities in later Web milestones.

use eframe::egui;
use egui::FontFamily;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use tool_application::web::{WebAppEvent, WebApplication};
use tool_application::{AppCommand, CommandOutcome};
use tool_databus::DataBus;
use tool_panels::{
    ChartPanel, SerialAction, SerialPanel, SerialPortItem, SerialView, TerminalPanel, theme,
};
use tool_platform::storage::{SettingsStore, web::WebSettingsStore};
use tool_platform::{PortDescriptor, PortId, SerialParity, SerialSettings};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use wasm_bindgen_futures::spawn_local;

const NOTO_SANS_SC: &[u8] = include_bytes!("../../../assets/NotoSansSC-VF.ttf");
const JETBRAINS_MONO: &[u8] =
    include_bytes!("../../../assets/JetBrainsMonoNerdFontMono-Regular.ttf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebPanel {
    Terminal,
    Chart,
    Serial,
    Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSettings {
    #[serde(default)]
    theme: theme::AppTheme,
    #[serde(default)]
    serial: SerialSettings,
    #[serde(default)]
    tx_hex: bool,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            theme: theme::AppTheme::default(),
            serial: SerialSettings::default(),
            tx_hex: false,
        }
    }
}

struct WebSerialState {
    ports: Vec<PortDescriptor>,
    connected: Option<PortId>,
    send_input: String,
    tx_hex: bool,
    dtr: bool,
    rts: bool,
    settings: SerialSettings,
    status: String,
}

impl Default for WebSerialState {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            connected: None,
            send_input: String::new(),
            tx_hex: false,
            dtr: true,
            rts: true,
            settings: SerialSettings::default(),
            status: "Web Serial：点击“刷新”读取已授权设备".to_owned(),
        }
    }
}

pub(crate) struct WebApp {
    terminal: TerminalPanel,
    chart: ChartPanel,
    active_panel: WebPanel,
    theme: theme::AppTheme,
    application: Option<WebApplication>,
    serial: Rc<RefCell<WebSerialState>>,
    settings_store: Option<WebSettingsStore>,
    settings_load: Rc<RefCell<Option<WebSettings>>>,
}

impl WebApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let selected_theme = theme::AppTheme::default();
        apply_web_theme(&cc.egui_ctx, selected_theme);
        setup_web_fonts(cc);

        let bus = DataBus::new();
        let application = WebApplication::new(bus.clone()).ok().map(|application| {
            let ctx = cc.egui_ctx.clone();
            application.set_repaint_waker(Rc::new(move || ctx.request_repaint()));
            application
        });
        let serial = Rc::new(RefCell::new(WebSerialState::default()));
        let settings_store = WebSettingsStore::from_window("hardware-workbench").ok();
        let settings_load = Rc::new(RefCell::new(None));
        if let Some(store) = settings_store.clone() {
            let loaded = settings_load.clone();
            let ctx = cc.egui_ctx.clone();
            spawn_local(async move {
                let settings = match store.load("settings.json".to_owned()).await {
                    Ok(Some(bytes)) => serde_json::from_slice::<WebSettings>(&bytes).ok(),
                    _ => match store.load("theme.json".to_owned()).await {
                        Ok(Some(bytes)) => serde_json::from_slice::<theme::AppTheme>(&bytes)
                            .ok()
                            .map(|theme| WebSettings {
                                theme,
                                ..WebSettings::default()
                            }),
                        _ => None,
                    },
                };
                if let Some(settings) = settings {
                    *loaded.borrow_mut() = Some(settings);
                    ctx.request_repaint();
                }
            });
        }
        let serial_status = if application.is_some() {
            "Web Serial：点击“刷新”读取已授权设备"
        } else {
            "当前浏览器不支持 Web Serial"
        };
        serial.borrow_mut().status = serial_status.to_owned();
        Self {
            terminal: TerminalPanel::new(&bus),
            chart: ChartPanel::new(&bus),
            active_panel: WebPanel::Terminal,
            theme: selected_theme,
            application,
            serial,
            settings_store,
            settings_load,
        }
    }

    fn poll_loaded_settings(&mut self, ctx: &egui::Context) {
        let Some(settings) = self.settings_load.borrow_mut().take() else {
            return;
        };
        self.theme = settings.theme;
        self.serial.borrow_mut().settings = settings.serial;
        self.serial.borrow_mut().tx_hex = settings.tx_hex;
        apply_web_theme(ctx, self.theme);
    }

    fn persist_settings(&self) {
        let Some(store) = self.settings_store.clone() else {
            return;
        };
        let settings = WebSettings {
            theme: self.theme,
            serial: self.serial.borrow().settings,
            tx_hex: self.serial.borrow().tx_hex,
        };
        let Ok(value) = serde_json::to_vec(&settings) else {
            return;
        };
        spawn_local(async move {
            let _ = store.save("settings.json".to_owned(), value).await;
        });
    }

    fn poll_web_events(&mut self) {
        let Some(application) = self.application.clone() else {
            return;
        };
        for event in application.drain_events() {
            let mut serial = self.serial.borrow_mut();
            match event {
                WebAppEvent::TaskStateChanged(snapshot) => {
                    serial.status = snapshot.message;
                }
                WebAppEvent::PortsRefreshed(ports) => {
                    serial.status = format!("已授权设备 {} 个", ports.len());
                    serial.ports = ports;
                }
                WebAppEvent::PortRequested(port) => {
                    serial.ports.retain(|item| item.id != port.id);
                    serial.ports.push(port);
                    serial.status = "设备已授权，可连接".to_owned();
                }
                WebAppEvent::Connected { port } => {
                    serial.connected = Some(port.clone());
                    serial.status = format!("已连接 {port} @ {}", settings_label(serial.settings));
                }
                WebAppEvent::Disconnected { port } => {
                    if serial.connected.as_ref() == Some(&port) {
                        serial.connected = None;
                    }
                    serial.status = "设备已断开".to_owned();
                }
                WebAppEvent::Sent { bytes, .. } => {
                    serial.status = format!("发送成功（{bytes} 字节）");
                }
                WebAppEvent::SignalsChanged { signal, value, .. } => {
                    match signal {
                        tool_application::web::SignalKind::Dtr => serial.dtr = value,
                        tool_application::web::SignalKind::Rts => serial.rts = value,
                    }
                    serial.status = format!("{signal:?} 已更新");
                }
                WebAppEvent::TaskFailed { error, .. } => {
                    serial.status = format!("操作失败：{error}");
                }
                WebAppEvent::TaskCancelled { .. } => {
                    serial.status = "操作已取消".to_owned();
                }
            }
        }
    }

    fn dispatch_serial(&self, command: AppCommand, ctx: &egui::Context) {
        let status = match self.application.as_ref() {
            Some(application) => match application.dispatch(command) {
                Ok(CommandOutcome::Pending { message, .. }) => message,
                Ok(CommandOutcome::Done) => "操作完成".to_owned(),
                Err(error) => format!("操作失败：{error}"),
            },
            None => "当前浏览器不支持 Web Serial".to_owned(),
        };
        self.serial.borrow_mut().status = status;
        ctx.request_repaint();
    }

    fn panel_button(&mut self, ui: &mut egui::Ui, panel: WebPanel, label: &str) {
        if ui
            .selectable_label(self.active_panel == panel, label)
            .clicked()
        {
            self.active_panel = panel;
        }
    }
}

impl eframe::App for WebApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::bg_primary().to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loaded_settings(ui.ctx());
        self.poll_web_events();
        egui::Panel::top("web-v1-header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("硬件调试工作台");
                ui.separator();
                ui.label("Web V1");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let status = self.serial.borrow().status.clone();
                    ui.colored_label(theme::yellow(), status);
                });
            });
        });

        egui::Panel::left("web-v1-navigation")
            .resizable(false)
            .default_size(150.0)
            .show(ui, |ui| {
                ui.heading("工作区");
                ui.separator();
                self.panel_button(ui, WebPanel::Terminal, "接收 / Terminal");
                self.panel_button(ui, WebPanel::Chart, "图表 / Chart");
                self.panel_button(ui, WebPanel::Serial, "串口 / Serial");
                self.panel_button(ui, WebPanel::Settings, "设置 / Settings");
                ui.add_space(16.0);
                ui.label("第一阶段已关闭");
                ui.label("Updater");
                ui.label("Marketplace");
                ui.label("Lua Plugin");
                ui.label("Recorder / Replay");
            });

        egui::CentralPanel::default().show(ui, |ui| match self.active_panel {
            WebPanel::Terminal => self.terminal.ui(ui),
            WebPanel::Chart => self.chart.ui(ui),
            WebPanel::Serial => self.serial_ui(ui),
            WebPanel::Settings => self.settings_ui(ui),
        });
    }
}

impl WebApp {
    fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("基础设置");
        ui.separator();
        ui.label("串口能力通过浏览器 Web Serial 提供，仅支持已授权设备。");
        ui.label("当前浏览器构建使用内嵌字体，不依赖本地文件系统。");
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("主题");
            for candidate in [
                theme::AppTheme::OneDarkPro,
                theme::AppTheme::Latte,
                theme::AppTheme::Mocha,
            ] {
                if ui
                    .selectable_label(self.theme == candidate, candidate.label())
                    .clicked()
                {
                    self.theme = candidate;
                    apply_web_theme(ui.ctx(), candidate);
                    self.persist_settings();
                }
            }
        });
    }

    fn serial_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut serial = self.serial.borrow_mut();
        let previous_settings = serial.settings;
        let previous_tx_hex = serial.tx_hex;
        let ports: Vec<SerialPortItem> = serial
            .ports
            .iter()
            .map(|port| SerialPortItem {
                id: port.id.to_string(),
                label: port.label.clone(),
                kind: String::new(),
            })
            .collect();
        let connected = serial.connected.as_ref().map(|port| port.to_string());
        let status = serial.status.clone();
        let (actions, settings_changed, tx_mode_changed) = {
            let state = &mut *serial;
            let WebSerialState {
                settings,
                send_input,
                tx_hex,
                dtr,
                rts,
                ..
            } = state;
            let mut view = SerialView {
                ports: &ports,
                connected: connected.as_deref(),
                status: &status,
                settings,
                send_input,
                tx_hex,
                dtr,
                rts,
                capabilities: tool_platform::TransportCapabilities::WEB_SERIAL,
                show_ports: true,
                show_sender: true,
            };
            let actions = SerialPanel::ui(ui, &mut view);
            let settings_changed = *settings != previous_settings;
            let tx_mode_changed = *tx_hex != previous_tx_hex;
            (actions, settings_changed, tx_mode_changed)
        };
        drop(serial);

        if settings_changed || tx_mode_changed {
            self.persist_settings();
        }
        for action in actions {
            let command = match action {
                SerialAction::Refresh => AppCommand::RefreshPorts,
                SerialAction::RequestPort => AppCommand::RequestPort,
                SerialAction::Connect { port, settings } => AppCommand::Connect {
                    port_name: port,
                    settings,
                },
                SerialAction::Disconnect { port } => AppCommand::Disconnect { port_name: port },
                SerialAction::SendText { port, text } => AppCommand::SendText {
                    port_name: port,
                    text,
                },
                SerialAction::SendHex { port, hex } => AppCommand::SendHex {
                    port_name: port,
                    hex,
                },
                SerialAction::SetDtr { port, value } => AppCommand::SetDtr {
                    port_name: port,
                    value,
                },
                SerialAction::SetRts { port, value } => AppCommand::SetRts {
                    port_name: port,
                    value,
                },
            };
            self.dispatch_serial(command, &ctx);
        }
    }
}

fn setup_web_fonts(cc: &eframe::CreationContext<'_>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "zh".to_owned(),
        egui::FontData::from_static(NOTO_SANS_SC).into(),
    );
    fonts.font_data.insert(
        "jetbrains".to_owned(),
        egui::FontData::from_static(JETBRAINS_MONO).into(),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "zh".to_owned());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "jetbrains".to_owned());
    cc.egui_ctx.set_fonts(fonts);
    egui_material_icons::initialize(&cc.egui_ctx);
}

fn apply_web_theme(ctx: &egui::Context, selected_theme: theme::AppTheme) {
    theme::set_active_theme(selected_theme);
    let is_dark = selected_theme.is_dark();
    ctx.set_theme(if is_dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });

    let mut style = (*ctx.global_style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.interact_size = egui::vec2(40.0, 28.0);
    style.visuals = if is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.panel_fill = theme::bg_primary();
    style.visuals.window_fill = theme::bg_deep();
    style.visuals.extreme_bg_color = theme::bg_deep();
    style.visuals.faint_bg_color = theme::bg_card();
    style.visuals.code_bg_color = theme::bg_input();
    style.visuals.text_edit_bg_color = Some(theme::bg_input());
    style.visuals.override_text_color = Some(theme::text_primary());
    style.visuals.weak_text_color = Some(theme::text_secondary());
    ctx.set_global_style(style);
}

fn settings_label(settings: SerialSettings) -> String {
    format!(
        "{} {}{}{}",
        settings.baud_rate,
        settings.data_bits,
        match settings.parity {
            SerialParity::None => 'N',
            SerialParity::Odd => 'O',
            SerialParity::Even => 'E',
        },
        settings.stop_bits
    )
}

/// Start the Web V1 application in an existing canvas.
///
/// The HTML/JS host owns the canvas and calls this function after the page is
/// ready, which keeps the Rust composition root independent of the hosting
/// framework.
#[wasm_bindgen]
pub async fn start(canvas_id: String) -> Result<(), JsValue> {
    let window =
        eframe::web_sys::window().ok_or_else(|| JsValue::from_str("window unavailable"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("document unavailable"))?;
    let element = document
        .get_element_by_id(&canvas_id)
        .ok_or_else(|| JsValue::from_str("canvas element not found"))?;
    let canvas = element
        .dyn_into::<eframe::web_sys::HtmlCanvasElement>()
        .map_err(|_| JsValue::from_str("element is not a canvas"))?;

    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| Ok(Box::new(WebApp::new(cc)))),
        )
        .await
}

/// Trunk loads the generated wasm module after the canvas has been parsed.
/// Starting from this hook keeps the host page free of generated wasm-bindgen
/// module names while retaining `start(canvas_id)` for embedding scenarios.
#[wasm_bindgen(start)]
pub fn bootstrap() {
    spawn_local(async {
        if let Err(error) = start("hardware-workbench".to_owned()).await {
            web_sys::console::error_1(&error);
        }
    });
}
