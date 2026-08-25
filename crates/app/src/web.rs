//! Web V1 composition root.
//!
//! The first browser build intentionally contains only presentation and the
//! data path that is already platform independent. Native-only services are
//! not constructed here: updater, marketplace, Lua plugins, recorder and
//! replay will be added behind capabilities in later Web milestones.

use eframe::egui;
use egui::FontFamily;
use std::cell::RefCell;
use std::rc::Rc;
use tool_application::web::{WebAppEvent, WebApplication};
use tool_application::{AppCommand, CommandOutcome};
use tool_databus::DataBus;
use tool_panels::{ChartPanel, TerminalPanel, theme};
use tool_platform::storage::{SettingsStore, web::WebSettingsStore};
use tool_platform::{PortDescriptor, PortId};
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

struct WebSerialState {
    ports: Vec<PortDescriptor>,
    connected: Option<PortId>,
    send_text: String,
    dtr: bool,
    rts: bool,
    status: String,
}

impl Default for WebSerialState {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            connected: None,
            send_text: String::new(),
            dtr: true,
            rts: true,
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
}

impl WebApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let selected_theme = theme::AppTheme::default();
        apply_web_theme(&cc.egui_ctx, selected_theme);
        setup_web_fonts(cc);

        let bus = DataBus::new();
        let application = WebApplication::new(bus.clone()).ok();
        let serial_status = if application.is_some() {
            "Web Serial：点击“刷新”读取已授权设备"
        } else {
            "当前浏览器不支持 Web Serial"
        };
        Self {
            terminal: TerminalPanel::new(&bus),
            chart: ChartPanel::new(&bus),
            active_panel: WebPanel::Terminal,
            theme: selected_theme,
            application,
            serial: Rc::new(RefCell::new(WebSerialState {
                status: serial_status.to_owned(),
                ..WebSerialState::default()
            })),
            settings_store: WebSettingsStore::from_window("hardware-workbench").ok(),
        }
    }

    fn poll_web_events(&mut self, ctx: &egui::Context) {
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
                    serial.status = format!("已连接 {port} @ 115200 8N1");
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
        ctx.request_repaint();
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
        self.poll_web_events(ui.ctx());
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
                    if let Some(store) = self.settings_store.clone() {
                        let value = serde_json::to_vec(&candidate).unwrap_or_default();
                        spawn_local(async move {
                            let _ = store.save("theme.json".to_owned(), value).await;
                        });
                    }
                }
            }
        });
    }

    fn serial_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.heading("Web Serial");
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("刷新已授权设备").clicked() {
                self.dispatch_serial(AppCommand::RefreshPorts, &ctx);
            }
            if ui.button("添加设备").clicked() {
                self.dispatch_serial(AppCommand::RequestPort, &ctx);
            }
        });

        let snapshot = self.serial.borrow();
        ui.label(&snapshot.status);
        let connected = snapshot.connected.clone();
        let ports = snapshot.ports.clone();
        drop(snapshot);

        for port in ports {
            ui.horizontal(|ui| {
                ui.label(&port.label);
                ui.label(format!("({})", port.id));
                if connected.as_ref() == Some(&port.id) {
                    if ui.button("断开").clicked() {
                        self.dispatch_serial(
                            AppCommand::Disconnect {
                                port_name: port.id.to_string(),
                            },
                            &ctx,
                        );
                    }
                } else if ui.button("连接 115200 8N1").clicked() {
                    self.dispatch_serial(
                        AppCommand::Connect {
                            port_name: port.id.to_string(),
                        },
                        &ctx,
                    );
                }
            });
        }

        ui.add_space(12.0);
        let mut send_text = self.serial.borrow().send_text.clone();
        ui.horizontal(|ui| {
            ui.label("TEXT");
            ui.text_edit_singleline(&mut send_text);
            let enabled = connected.is_some();
            if ui.add_enabled(enabled, egui::Button::new("发送")).clicked()
                && let Some(port) = connected.clone()
            {
                self.dispatch_serial(
                    AppCommand::SendText {
                        port_name: port.to_string(),
                        text: send_text.clone(),
                    },
                    &ctx,
                );
            }
        });
        self.serial.borrow_mut().send_text = send_text;

        let mut dtr = self.serial.borrow().dtr;
        let mut rts = self.serial.borrow().rts;
        ui.horizontal(|ui| {
            ui.checkbox(&mut dtr, "DTR");
            ui.checkbox(&mut rts, "RTS");
        });
        if dtr != self.serial.borrow().dtr
            && let Some(port) = connected.clone()
        {
            self.dispatch_serial(
                AppCommand::SetDtr {
                    port_name: port.to_string(),
                    value: dtr,
                },
                &ctx,
            );
        }
        if rts != self.serial.borrow().rts
            && let Some(port) = connected
        {
            self.dispatch_serial(
                AppCommand::SetRts {
                    port_name: port.to_string(),
                    value: rts,
                },
                &ctx,
            );
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
