// Release 模式下不显示控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use egui::Color32;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use tool_core::{Direction, Event, LogLevel, Payload, now_timestamp_ms};
use tool_databus::DataBus;
use tool_extension::PluginManager;
use tool_lua_host::{LuaReplayConfig, run_replay_analyzer};
use tool_panels::{
    Activity, DynamicPanels, LogPanel, PanelKind, PanelManager, PluginsPanel, ReplayPanel,
    TerminalPanel, theme,
};
use tool_recorder::{JsonlRecorder, RecordMode};
use tool_transport::{
    DataBits, Parity, SerialConfig, SerialPortDescriptor, StopBits, TransportManager,
};

const ACTIVITY_BAR_WIDTH: f32 = 104.0;
const BOTTOM_PANEL_HEIGHT: f32 = 350.0;
const BOTTOM_PANEL_MIN: f32 = 550.0;
const INSPECTOR_WIDTH: f32 = 240.0;
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 820.0;
const REPAINT_INTERVAL_MS: u64 = 50;
const PORT_REFRESH_INTERVAL_SECS: f64 = 0.5;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
            .with_min_inner_size([960.0, 640.0]),
        persist_window: true,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            present_mode: eframe::wgpu::PresentMode::Immediate,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "硬件调试工作台",
        options,
        Box::new(|cc| Ok(Box::new(WorkbenchApp::new(cc)))),
    )
}

// ── 字体 ──
fn setup_fonts(cc: &eframe::CreationContext<'_>) {
    for path in &[
        "assets/NotoSansSC-VF.ttf".to_owned(),
        "C:\\Windows\\Fonts\\msyh.ttc".to_owned(),
    ] {
        if let Ok(fb) = std::fs::read(path) {
            let mut f = egui::FontDefinitions::default();
            f.font_data
                .insert("zh".into(), egui::FontData::from_owned(fb).into());
            if let Ok(eb) = std::fs::read("assets/seguiemj.ttf") {
                f.font_data
                    .insert("emoji".into(), egui::FontData::from_owned(eb).into());
            }
            let p = f
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default();
            p.insert(0, "zh".into());
            if f.font_data.contains_key("emoji") {
                p.insert(0, "emoji".into());
            }
            f.families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("zh".into());
            cc.egui_ctx.set_fonts(f);
            return;
        }
    }
}

// ── 主题 ──
fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
    let mut s = (*ctx.global_style()).clone();
    s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    s.spacing.button_padding = egui::vec2(10.0, 5.0);
    s.spacing.interact_size = egui::vec2(40.0, 28.0);
    s.spacing.slider_width = 180.0;
    s.spacing.combo_width = 140.0;
    s.spacing.text_edit_width = 220.0;
    s.interaction.resize_grab_radius_side = 6.0;
    s.interaction.resize_grab_radius_corner = 10.0;
    s.animation_time = 0.0;
    #[cfg(debug_assertions)]
    {
        s.debug.show_interactive_widgets = false;
        s.debug.show_focused_widget = false;
        s.debug.show_unaligned = false;
        s.debug.warn_if_rect_changes_id = false;
        s.debug.show_resize = false;
        s.debug.show_widget_hits = false;
    }
    let mut v = egui::Visuals::dark();
    v.panel_fill = theme::BG_PRIMARY;
    v.window_fill = theme::BG_SECONDARY;
    v.extreme_bg_color = theme::BG_SECONDARY;
    v.faint_bg_color = theme::BG_TERTIARY;
    v.code_bg_color = theme::BG_INPUT;
    v.text_edit_bg_color = Some(theme::BG_INPUT);
    v.override_text_color = Some(theme::TEXT_PRIMARY);
    v.weak_text_color = Some(theme::TEXT_SECONDARY);
    v.warn_fg_color = theme::YELLOW;
    v.error_fg_color = theme::RED;
    v.selection.bg_fill = theme::BG_SELECTION;
    v.selection.stroke = egui::Stroke::new(1.0, theme::BLUE);
    v.hyperlink_color = theme::CYAN;
    v.window_stroke = egui::Stroke::new(1.0, theme::BORDER_LIGHT);
    v.resize_corner_size = 8.0;
    v.striped = true;
    v.collapsing_header_frame = false;
    v.window_highlight_topmost = false;
    v.button_frame = true;
    v.indent_has_left_vline = false;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = theme::BG_INPUT;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    w.noninteractive.weak_bg_fill = theme::BG_SECONDARY;
    w.inactive.bg_fill = theme::BG_TERTIARY;
    w.inactive.weak_bg_fill = theme::BG_INPUT;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER_LIGHT);
    w.inactive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    w.hovered.bg_fill = theme::WIDGET_HOVER;
    w.hovered.weak_bg_fill = theme::BG_TERTIARY;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, theme::BLUE);
    w.hovered.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    w.active.bg_fill = theme::WIDGET_ACTIVE_WEAK;
    w.active.weak_bg_fill = theme::WIDGET_ACTIVE_WEAK;
    w.active.bg_stroke = egui::Stroke::new(1.0, theme::BLUE);
    w.active.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_WHITE);
    w.open.bg_fill = theme::WIDGET_OPEN;
    w.open.weak_bg_fill = theme::BG_INPUT;
    w.open.bg_stroke = egui::Stroke::new(1.0, theme::BLUE);
    w.open.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    s.visuals = v;
    ctx.set_global_style(s);
}

// ── 数据结构 ──
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetachedPanelAction {
    None,
    Attach,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTab {
    Terminal,
    Logs,
}

impl BottomTab {
    const ALL: [Self; 2] = [Self::Terminal, Self::Logs];

    fn label(self) -> &'static str {
        match self {
            Self::Terminal => "接收",
            Self::Logs => "日志",
        }
    }

    fn is_available(self, terminal_popup_open: bool) -> bool {
        !matches!(self, Self::Terminal) || !terminal_popup_open
    }
}

struct WorkbenchApp {
    bus: DataBus,
    transport: TransportManager,
    plugin_manager: PluginManager,
    recorder: JsonlRecorder,
    panels: PanelManager,
    terminal_panel: TerminalPanel,
    dynamic_panels: DynamicPanels,
    plugins_panel: PluginsPanel,
    replay_panel: ReplayPanel,
    bottom_log_panel: LogPanel,
    ports: Vec<SerialPortDescriptor>,
    selected_port: Option<String>,
    baud_rate: String,
    data_bits: String,
    stop_bits: String,
    parity: String,
    timeout_ms: String,
    recorder_path: String,
    status_message: String,
    last_port_refresh: f64,
    bottom_panel_visible: bool,
    bottom_tab: BottomTab,
    send_input: String,
    send_hex_mode: bool,
    send_append_lf: bool,
    send_error: Option<String>,
    send_popup_open: bool,
    terminal_popup_open: bool,
    detached_dynamic_panels: BTreeSet<String>,
    top_bar_serial_collapsed: bool,
    activity_order: Vec<Activity>,
    activity_drag_source: Option<usize>,
    activity_rects_cache: Vec<egui::Rect>,
    last_rate_check_time: f64,
    last_event_count: u64,
    event_rate: f64,
    dynamic_drag_source: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedConfig {
    panels: PanelManager,
    selected_port: Option<String>,
    baud_rate: String,
    data_bits: String,
    stop_bits: String,
    parity: String,
    timeout_ms: String,
    recorder_path: String,
    #[serde(default = "default_activity_order")]
    activity_order: Vec<Activity>,
    #[serde(default)]
    enabled_plugins: Vec<String>,
}

fn default_activity_order() -> Vec<Activity> {
    vec![
        Activity::Devices,
        Activity::Replay,
        Activity::Plugins,
        Activity::Settings,
    ]
}

// ══════════════════════════════════════════
//  WorkbenchApp impl
// ══════════════════════════════════════════

impl WorkbenchApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(cc);
        cc.egui_ctx.set_embed_viewports(false);
        let bus = DataBus::new();
        let transport = TransportManager::new(bus.clone());
        let mut pm = PluginManager::new(bus.clone(), transport.clone());
        if let Err(e) = pm.discover_roots([PathBuf::from("plugins")]) {
            bus.publish(Event::system_log(
                LogLevel::Error,
                "ext",
                format!("plugin discover: {e}"),
            ));
        }
        let recorder = JsonlRecorder::new(bus.clone());
        let config = load_config();
        apply_theme(&cc.egui_ctx);
        let mut rp = config
            .as_ref()
            .map(|c| c.panels.clone())
            .unwrap_or_default();
        rp.discard_dynamic_tabs();

        let mut app = Self {
            terminal_panel: TerminalPanel::new(&bus),
            dynamic_panels: DynamicPanels::new(&bus),
            plugins_panel: PluginsPanel::new(),
            replay_panel: ReplayPanel::new(&bus),
            bottom_log_panel: LogPanel::new(&bus),
            ports: Vec::new(),
            selected_port: config.as_ref().and_then(|c| c.selected_port.clone()),
            baud_rate: config
                .as_ref()
                .map(|c| c.baud_rate.clone())
                .unwrap_or_else(|| "115200".into()),
            data_bits: config
                .as_ref()
                .map(|c| c.data_bits.clone())
                .unwrap_or_else(|| "8".into()),
            stop_bits: config
                .as_ref()
                .map(|c| c.stop_bits.clone())
                .unwrap_or_else(|| "1".into()),
            parity: config
                .as_ref()
                .map(|c| c.parity.clone())
                .unwrap_or_else(|| "none".into()),
            timeout_ms: config
                .as_ref()
                .map(|c| c.timeout_ms.clone())
                .unwrap_or_else(|| "50".into()),
            recorder_path: config
                .as_ref()
                .map(|c| c.recorder_path.clone())
                .unwrap_or_else(default_recorder_path),
            panels: rp.clone(),
            status_message: "就绪".into(),
            last_port_refresh: 0.0,
            bottom_panel_visible: rp.bottom_logs_visible,
            bottom_tab: BottomTab::Terminal,
            send_input: String::new(),
            send_hex_mode: false,
            send_append_lf: false,
            send_error: None,
            send_popup_open: false,
            terminal_popup_open: false,
            detached_dynamic_panels: BTreeSet::new(),
            top_bar_serial_collapsed: false,
            activity_order: config
                .as_ref()
                .map(|c| c.activity_order.clone())
                .unwrap_or_else(default_activity_order),
            activity_drag_source: None,
            activity_rects_cache: Vec::new(),
            last_rate_check_time: 0.0,
            last_event_count: 0,
            event_rate: 0.0,
            bus,
            transport,
            plugin_manager: pm,
            recorder,
            dynamic_drag_source: None,
        };
        app.refresh_ports();
        let enabled: Vec<String> = config
            .as_ref()
            .map(|c| c.enabled_plugins.clone())
            .unwrap_or_default();
        for id in &enabled {
            if let Err(e) = app.plugin_manager.enable(id) {
                app.log(LogLevel::Warn, format!("restore plugin {id}: {e}"));
            }
        }
        app.log(LogLevel::Info, "就绪");
        app
    }

    fn log(&self, lv: LogLevel, m: impl Into<String>) {
        self.bus.publish(Event::system_log(lv, "app", m.into()));
    }
    fn refresh_ports(&mut self) {
        self.refresh_ports_impl(true);
    }

    fn refresh_ports_silent(&mut self) {
        self.refresh_ports_impl(false);
    }

    fn refresh_ports_impl(&mut self, show_status: bool) {
        let old_names: BTreeSet<String> = self
            .ports
            .iter()
            .map(|port| port.port_name.clone())
            .collect();

        let old_selected = self.selected_port.clone();

        match self.transport.list_serial_ports() {
            Ok(new_ports) => {
                let new_names: BTreeSet<String> = new_ports
                    .iter()
                    .map(|port| port.port_name.clone())
                    .collect();

                let added_ports: Vec<String> = new_names.difference(&old_names).cloned().collect();

                let removed_ports: Vec<String> =
                    old_names.difference(&new_names).cloned().collect();

                self.ports = new_ports;

                let selected_still_exists = self
                    .selected_port
                    .as_ref()
                    .is_some_and(|selected| new_names.contains(selected));

                // 关键：只在当前选中端口消失时清空选择，不自动切到新端口。
                if !selected_still_exists {
                    self.selected_port = None;
                }

                if show_status {
                    self.status_message = format!("{} 个串口", self.ports.len());
                    return;
                }

                if !added_ports.is_empty() {
                    self.status_message = format!("发现串口 {}", added_ports.join(", "));
                } else if !removed_ports.is_empty() {
                    self.status_message = format!("移除串口 {}", removed_ports.join(", "));
                } else if self.selected_port != old_selected {
                    self.status_message = "请选择串口".into();
                }
            }
            Err(error) => {
                self.status_message = error.to_string();
            }
        }
    }

    fn open_selected_port(&mut self) {
        self.refresh_ports_silent();

        let Some(p) = self.selected_port.clone() else {
            self.log(LogLevel::Warn, "请选择串口");
            self.status_message = "请选择串口".into();
            return;
        };

        let selected_exists = self.ports.iter().any(|port| port.port_name == p);

        if !selected_exists {
            self.status_message = format!("串口 {p} 不存在，请重新选择");
            return;
        }

        let b = self.baud_rate.parse().unwrap_or(115200);

        let cfg = SerialConfig {
            port_name: p.clone(),
            baud_rate: b,
            data_bits: pdb(&self.data_bits),
            stop_bits: psb(&self.stop_bits),
            parity: ppar(&self.parity),
            timeout_ms: self.timeout_ms.parse().unwrap_or(50),
        };

        match self.transport.open_serial(cfg) {
            Ok(()) => {
                self.status_message = format!("{p} 已连接");
                self.open_bottom_panel();
            }
            Err(e) => {
                self.status_message = e.to_string();
            }
        }
    }
    fn start_or_stop_recording(&mut self) {
        if self.recorder.is_running() {
            self.recorder.stop();
            self.status_message = "录制已停止".into();
        } else {
            match self.recorder.start(PathBuf::from(&self.recorder_path)) {
                Ok(()) => {
                    self.status_message = "录制中".into();
                }
                Err(e) => {
                    self.status_message = e.to_string();
                }
            }
        }
    }
    fn save_config(&mut self) {
        self.panels.bottom_logs_visible = self.bottom_panel_visible;
        let mut p = self.panels.clone();
        p.discard_dynamic_tabs();
        p.bottom_logs_visible = self.bottom_panel_visible;
        let cfg = PersistedConfig {
            panels: p,
            selected_port: self.selected_port.clone(),
            baud_rate: self.baud_rate.clone(),
            data_bits: self.data_bits.clone(),
            stop_bits: self.stop_bits.clone(),
            parity: self.parity.clone(),
            timeout_ms: self.timeout_ms.clone(),
            recorder_path: self.recorder_path.clone(),
            activity_order: self.activity_order.clone(),
            enabled_plugins: self
                .plugin_manager
                .summaries()
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.state,
                        tool_extension::PluginState::Enabled | tool_extension::PluginState::Running
                    )
                })
                .map(|s| s.id)
                .collect(),
        };
        if let Ok(t) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(config_path(), t);
        }
    }
    fn available_bottom_tabs(&self) -> Vec<BottomTab> {
        BottomTab::ALL
            .into_iter()
            .filter(|tab| tab.is_available(self.terminal_popup_open))
            .collect()
    }

    fn ensure_bottom_tab_available(&mut self) {
        if self.bottom_tab.is_available(self.terminal_popup_open) {
            return;
        }
        if let Some(tab) = self.available_bottom_tabs().into_iter().next() {
            self.bottom_tab = tab;
        }
    }

    fn open_bottom_panel(&mut self) {
        self.bottom_panel_visible = true;
        if BottomTab::Terminal.is_available(self.terminal_popup_open) {
            self.bottom_tab = BottomTab::Terminal;
        } else {
            self.ensure_bottom_tab_available();
        }
    }

    fn toggle_bottom_panel(&mut self) {
        if self.bottom_panel_visible {
            self.bottom_panel_visible = false;
        } else {
            self.open_bottom_panel();
            self.status_message = "底部面板已打开".into();
        }
    }

    // ── UI 组件 ──

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("检查器");
        let st = self.transport.status();
        ui.label(egui::RichText::new("串口").strong());
        if st.open {
            ui.colored_label(
                theme::GREEN,
                format!(
                    "● {} @ {}",
                    st.port_name.as_deref().unwrap_or("?"),
                    st.baud_rate.unwrap_or(0)
                ),
            );
        } else {
            ui.colored_label(theme::TEXT_SECONDARY, "○ 已关闭");
        }
        ui.separator();
        ui.label(egui::RichText::new("录制").strong());
        ui.label(if self.recorder.is_running() {
            "⏺ 运行中"
        } else {
            "⏹ 已停止"
        });
        if let Some(p) = self.recorder.current_path() {
            ui.monospace(p.display().to_string());
        }
        ui.separator();
        ui.label(egui::RichText::new("运行时").strong());
        ui.label(format!("插件: {}", self.plugin_manager.count()));
        ui.label(format!("动态面板: {}", self.dynamic_panels.count()));
        if let Some(e) = self.dynamic_panels.last_error() {
            ui.colored_label(theme::RED, e);
        }
        ui.separator();
        ui.label(egui::RichText::new("DataBus").strong());
        ui.label(format!(
            "事件 {} | {:.0}/s",
            self.bus.history_len(),
            self.event_rate
        ));
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let st = self.transport.status();
        ui.horizontal(|ui| {
            let (d, l) = if let (Some(p), Some(b)) = (st.port_name.clone(), st.baud_rate) {
                (if st.open { "●" } else { "○" }, format!("{p} @ {b}"))
            } else {
                ("○", "串口已关闭".into())
            };
            ui.label(egui::RichText::new(d).color(if st.open {
                theme::GREEN
            } else {
                theme::TEXT_SECONDARY
            }));
            ui.label(l);
            ui.separator();
            let rec = self.recorder.is_running();
            ui.label(egui::RichText::new("●").color(if rec {
                theme::RED
            } else {
                theme::TEXT_SECONDARY
            }));
            ui.label(if rec { "录制中" } else { "未录制" });
            ui.separator();
            ui.label(format!("{:.0} 事件/秒", self.event_rate));
            ui.separator();
            ui.label(&self.status_message);
        });
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let pointer = ui.ctx().pointer_latest_pos();
        let mut activity_rects = Vec::with_capacity(self.activity_order.len());

        ui.vertical_centered(|ui| {
            for (idx, &act) in self.activity_order.iter().enumerate() {
                let selected = self.panels.active_dynamic_id().is_none()
                    && self.panels.activity == act;
                let label = format!("{} {}", aicon(act), act.label());
                let shortcut = ashortcut(act);

                let hover = if shortcut.is_empty() {
                    act.label().to_owned()
                } else {
                    format!("{} ({})", act.label(), shortcut)
                };

                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 28.0),
                    egui::Sense::click_and_drag(),
                );

                if response.drag_started() {
                    self.activity_drag_source = Some(idx);
                }

                if response.clicked() && self.activity_drag_source.is_none() {
                    self.panels.select_activity(act);
                }

                let is_source = self.activity_drag_source == Some(idx);

                let bg = if is_source {
                    theme::BG_TERTIARY
                } else if selected || response.hovered() {
                    if selected {
                        theme::BG_SELECTION
                    } else {
                        theme::WIDGET_HOVER
                    }
                } else {
                    theme::BG_SECONDARY
                };

                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, bg);

                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &label,
                    egui::FontId::proportional(12.0),
                    if is_source {
                        theme::TEXT_SECONDARY
                    } else {
                        theme::TEXT_PRIMARY
                    },
                );

                response.on_hover_text(hover);

                activity_rects.push(rect);
            }
        });

        let drag_insert_index = if self.activity_drag_source.is_some() {
            pointer.and_then(|pos| activity_insert_index_from_pointer(&activity_rects, pos))
        } else {
            None
        };

        if let Some(insert_index) = drag_insert_index {
            paint_activity_insert_line(ui, &activity_rects, insert_index);
        }

        if self.activity_drag_source.is_some() && ui.input(|i| i.pointer.any_released()) {
            if let Some(source_index) = self.activity_drag_source.take() {
                if let Some(mut insert_index) = drag_insert_index {
                    insert_index = insert_index.min(self.activity_order.len());

                    if insert_index > source_index {
                        insert_index -= 1;
                    }

                    if insert_index != source_index {
                        let item = self.activity_order.remove(source_index);
                        let insert_index = insert_index.min(self.activity_order.len());
                        self.activity_order.insert(insert_index, item);
                        self.save_config();
                    }
                }
            }
        }

        if self.activity_drag_source.is_some() && !ui.input(|i| i.pointer.primary_down()) {
            self.activity_drag_source = None;
        }

        self.activity_rects_cache = activity_rects;

        self.dynamic_panel_shortcuts(ui);

        ui.separator();

        if ui
            .selectable_label(self.bottom_panel_visible, "▽ 终端区")
            .on_hover_text("Ctrl+B")
            .clicked()
        {
            self.toggle_bottom_panel();
        }
    }
    fn dynamic_panel_shortcuts(&mut self, ui: &mut egui::Ui) {
        let items: Vec<(String, String)> = self
            .panels
            .tabs
            .iter()
            .filter_map(|kind| kind.dynamic_id().map(|id| id.to_owned()))
            .filter(|id| self.dynamic_panels.contains(id))
            .map(|id| {
                let title = self.dynamic_panels.title(&id).unwrap_or(&id).to_owned();
                (id, title)
            })
            .collect();

        if items.is_empty() {
            return;
        }

        ui.separator();

        let pointer = ui.ctx().pointer_latest_pos();
        let mut rects = Vec::with_capacity(items.len());

        for (index, (id, title)) in items.iter().enumerate() {
            let active = self.panels.active_dynamic_id() == Some(id);
            let is_source = self.dynamic_drag_source == Some(index);

            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 24.0),
                egui::Sense::click_and_drag(),
            );

            if response.drag_started() {
                self.dynamic_drag_source = Some(index);
            }

            if response.clicked() && self.dynamic_drag_source.is_none() {
                self.panels.open_tab(PanelKind::Dynamic(id.clone()));
            }

            let bg = if is_source {
                theme::BG_TERTIARY
            } else if active || response.hovered() {
                if active {
                    theme::BG_SELECTION
                } else {
                    theme::WIDGET_HOVER
                }
            } else {
                Color32::TRANSPARENT
            };

            let painter = ui.painter_at(rect);

            if bg != Color32::TRANSPARENT {
                painter.rect_filled(rect, 4.0, bg);
            }

            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("  {title}"),
                egui::FontId::proportional(12.0),
                if is_source {
                    theme::TEXT_SECONDARY
                } else {
                    theme::TEXT_PRIMARY
                },
            );

            response.on_hover_text("拖动调整插件标签顺序");

            rects.push(rect);
        }

        let insert_index = if self.dynamic_drag_source.is_some() {
            pointer.and_then(|pos| vertical_insert_index_from_pointer(&rects, pos))
        } else {
            None
        };

        if let Some(insert_index) = insert_index {
            paint_vertical_insert_line(ui, &rects, insert_index);
        }

        if self.dynamic_drag_source.is_some() && ui.input(|input| input.pointer.any_released()) {
            if let Some(source_index) = self.dynamic_drag_source.take() {
                if let Some(insert_index) = insert_index {
                    self.reorder_dynamic_tabs(source_index, insert_index);
                }
            }
        }

        if self.dynamic_drag_source.is_some() && !ui.input(|input| input.pointer.primary_down()) {
            self.dynamic_drag_source = None;
        }
    }
    fn reorder_dynamic_tabs(&mut self, source_index: usize, mut insert_index: usize) {
        let mut dynamic_tabs: Vec<PanelKind> = self
            .panels
            .tabs
            .iter()
            .filter(|kind| kind.dynamic_id().is_some())
            .cloned()
            .collect();

        if source_index >= dynamic_tabs.len() {
            return;
        }

        insert_index = insert_index.min(dynamic_tabs.len());

        if insert_index > source_index {
            insert_index -= 1;
        }

        if insert_index == source_index {
            return;
        }

        let item = dynamic_tabs.remove(source_index);
        let insert_index = insert_index.min(dynamic_tabs.len());
        dynamic_tabs.insert(insert_index, item);

        let mut dynamic_iter = dynamic_tabs.into_iter();

        for kind in &mut self.panels.tabs {
            if kind.dynamic_id().is_some() {
                if let Some(next) = dynamic_iter.next() {
                    *kind = next;
                }
            }
        }

        self.save_config();
    }
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let so = self.transport.status().open;
            let sl = if so {
                format!(
                    "串口 ▸ {}",
                    self.transport.status().port_name.as_deref().unwrap_or("?")
                )
            } else {
                "串口 ▸ 未连接".into()
            };
            if ui
                .selectable_label(
                    !self.top_bar_serial_collapsed,
                    egui::RichText::new(format!("{} {sl}", if so { "●" } else { "○" }))
                        .color(if so { theme::GREEN } else { theme::RED }),
                )
                .clicked()
            {
                self.top_bar_serial_collapsed = !self.top_bar_serial_collapsed;
            }
            if !self.top_bar_serial_collapsed {
                self.serial_connect_controls(ui, "top-port", "top-baud", 130.0, 80.0, true);
            }
            ui.separator();
            let rec = self.recorder.is_running();
            if ui
                .button(if rec {
                    egui::RichText::new("⏹ 停止").color(theme::RED)
                } else {
                    egui::RichText::new("⏺ 录制").color(theme::TEXT_SECONDARY)
                })
                .clicked()
            {
                self.start_or_stop_recording();
            }
            if ui.small_button("保存布局").clicked() {
                self.save_config();
                self.status_message = "布局已保存".into();
            }
        });
    }

    fn send_bar(&mut self, ui: &mut egui::Ui) {
        let so = self.transport.status().open;
        ui.horizontal(|ui| {
            ui.label("发送");
            ui.radio_value(&mut self.send_hex_mode, false, "文本");
            ui.radio_value(&mut self.send_hex_mode, true, "HEX");
            ui.checkbox(&mut self.send_append_lf, "LF");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("⛶").on_hover_text("放大编辑").clicked() {
                    self.send_popup_open = true;
                }
            });
        });
        ui.add(
            egui::TextEdit::multiline(&mut self.send_input)
                .desired_width(f32::INFINITY)
                .desired_rows(5)
                .hint_text(if so {
                    "Ctrl+Enter 发送 | ⛶ 放大编辑"
                } else {
                    "可先编辑内容，打开串口后发送"
                }),
        );
        let ctrl_enter = ui
            .ctx()
            .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(so && !self.send_input.is_empty(), egui::Button::new("发送"))
                .clicked()
                || (ctrl_enter && so && !self.send_input.is_empty())
            {
                self.do_send();
            }
            if ui.button("清空").clicked() {
                self.send_input.clear();
                self.send_error = None;
            }
            if !so {
                ui.colored_label(theme::YELLOW, "⚠ 请先打开串口");
            }
            if let Some(ref e) = self.send_error {
                ui.colored_label(theme::RED, translate_error(e));
            }
        });
    }

    fn do_send(&mut self) {
        self.send_error = send_impl(
            &self.send_input,
            self.send_hex_mode,
            self.send_append_lf,
            &self.transport,
        )
        .err()
        .map(|e| e.to_string());
    }

    fn show_bottom_panel_contents(&mut self, ui: &mut egui::Ui) {
        self.ensure_bottom_tab_available();
        let visible_tabs = self.available_bottom_tabs();

        // 顶部标签栏：固定在底部面板顶部
        ui.horizontal_wrapped(|ui| {
            for tab in &visible_tabs {
                if ui
                    .selectable_label(self.bottom_tab == *tab, tab.label())
                    .clicked()
                {
                    self.bottom_tab = *tab;
                }
            }
        });
        ui.separator();

        let body_height = ui.available_height();

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), body_height),
            egui::Layout::bottom_up(egui::Align::Min),
            |ui| {
                // 1. 状态栏固定在最底部
                self.status_bar(ui);

                // 2. 发送区固定在状态栏上方
                if !self.send_popup_open {
                    ui.separator();
                    self.send_bar(ui);
                }

                ui.separator();

                // 3. 剩余空间全部给接收区 / 日志区
                let receive_area_total_height = ui.available_height().max(80.0);

                match self.bottom_tab {
                    BottomTab::Terminal => {
                        // TerminalPanel 内部自己还有 RX/TX/HEX 工具栏 + separator
                        let terminal_header_height = 42.0;

                        self.terminal_panel.height =
                            (receive_area_total_height - terminal_header_height).max(40.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), receive_area_total_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.terminal_panel.ui(ui);
                            },
                        );
                    }

                    BottomTab::Logs => {
                        ui.allocate_ui_with_layout(
                            egui::vec2(ui.available_width(), receive_area_total_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                self.bottom_log_panel.ui(ui);
                            },
                        );
                    }
                }
            },
        );
    }
    fn device_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备");

        ui.horizontal(|ui| {
            self.serial_connect_controls(ui, "dev-port", "dev-baud", 180.0, 90.0, false);
        });

        ui.horizontal(|ui| {
            ui.label("数据位");
            egui::ComboBox::from_id_salt("dev-db")
                .width(60.0)
                .selected_text(&self.data_bits)
                .show_ui(ui, |ui| {
                    for &v in &["5", "6", "7", "8"] {
                        ui.selectable_value(&mut self.data_bits, v.to_owned(), v);
                    }
                });

            ui.label("停止位");
            egui::ComboBox::from_id_salt("dev-sb")
                .width(60.0)
                .selected_text(&self.stop_bits)
                .show_ui(ui, |ui| {
                    for &v in &["1", "2"] {
                        ui.selectable_value(&mut self.stop_bits, v.to_owned(), v);
                    }
                });

            ui.label("校验");
            egui::ComboBox::from_id_salt("dev-par")
                .width(70.0)
                .selected_text(&self.parity)
                .show_ui(ui, |ui| {
                    for &(v, l) in &[("none", "无"), ("odd", "奇"), ("even", "偶")] {
                        ui.selectable_value(&mut self.parity, v.to_owned(), l);
                    }
                });

            ui.label("超时(ms)");
            ui.add(egui::TextEdit::singleline(&mut self.timeout_ms).desired_width(50.0));
        });

        ui.separator();

        ui.heading("录制");

        ui.horizontal(|ui| {
            ui.label("路径");

            let recording = self.recorder.is_running();

            ui.add_enabled(
                !recording,
                egui::TextEdit::singleline(&mut self.recorder_path).desired_width(360.0),
            );

            if ui
                .add_enabled(!recording, egui::Button::new("浏览"))
                .on_hover_text(if recording {
                    "录制中不能修改保存路径"
                } else {
                    "选择录制保存路径"
                })
                .clicked()
            {
                if let Some(path) = pick_recorder_path(&self.recorder_path) {
                    self.recorder_path = path.display().to_string();
                }
            }

            if ui.button(if recording { "停止" } else { "录制" }).clicked() {
                self.start_or_stop_recording();
            }
        });

        ui.horizontal(|ui| {
            ui.label("模式");
            let mut mode = self.recorder.mode();
            egui::ComboBox::from_id_salt("record-mode")
                .width(160.0)
                .selected_text(record_mode_label(mode))
                .show_ui(ui, |ui| {
                    for &m in &[
                        RecordMode::StandardReplay,
                        RecordMode::RawSerial,
                        RecordMode::FullDebug,
                    ] {
                        ui.selectable_value(&mut mode, m, record_mode_label(m));
                    }
                });
            self.recorder.set_mode(mode);
        });

        ui.separator();

        ui.heading("可用端口");
        egui::ScrollArea::vertical().show(ui, |ui| {
            for port in &self.ports {
                ui.monospace(format!("{} {}", port.port_name, port.port_type));
            }
        });
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
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

    // ── 动态面板辅助 ──

    fn dynamic_tab_cleanup(&mut self) {
        let stale: Vec<String> = self
            .panels
            .tabs
            .iter()
            .filter_map(|k| k.dynamic_id().map(str::to_owned))
            .filter(|id| !self.dynamic_panels.contains(id))
            .collect();
        for id in stale {
            self.detached_dynamic_panels.remove(&id);
            self.panels.close_tab(PanelKind::Dynamic(id));
        }
    }

    fn dynamic_panel_ui(&mut self, ui: &mut egui::Ui, id: &str) {
        let title = self.dynamic_panels.title(id).unwrap_or(id).to_owned();
        ui.horizontal(|ui| {
            ui.heading(&title);
            if self.detached_dynamic_panels.contains(id) {
                if ui.button("↙").clicked() {
                    self.detached_dynamic_panels.remove(id);
                }
            } else if ui.button("↗").clicked() {
                self.detached_dynamic_panels.insert(id.to_owned());
            }
        });
        ui.separator();
        if self.detached_dynamic_panels.contains(id) {
            ui.label("已弹出到独立窗口");
            return;
        }
        self.dynamic_panels.ui_body(ui, id);
    }

    fn detached_dynamic_panel_viewports(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self.detached_dynamic_panels.iter().cloned().collect();

        for id in ids {
            if !self.dynamic_panels.contains(&id) {
                self.detached_dynamic_panels.remove(&id);
                continue;
            }

            let title = self.dynamic_panels.title(&id).unwrap_or(&id).to_owned();
            let viewport_id = egui::ViewportId::from_hash_of(("dynamic-panel", id.as_str()));

            let builder = egui::ViewportBuilder::default()
                .with_title(format!("{title} - 硬件调试工作台"))
                .with_inner_size([900.0, 640.0])
                .with_min_inner_size([520.0, 360.0]);

            let action = ctx.show_viewport_immediate(viewport_id, builder, |ctx, _class| {
                let mut action = DetachedPanelAction::None;

                if ctx.input(|input| input.viewport().close_requested()) {
                    action = DetachedPanelAction::Attach;
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(theme::BG_PRIMARY))
                    .show(ctx, |ui| {
                        // 再手动铺一层，避免某些平台 / resize 时出现未清屏黑边。
                        let rect = ui.max_rect();
                        ui.painter().rect_filled(rect, 0.0, theme::BG_PRIMARY);

                        ui.horizontal(|ui| {
                            ui.heading(&title);

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("关闭面板").clicked() {
                                        action = DetachedPanelAction::Close;
                                    }

                                    if ui.button("↙ 回到标签栏").clicked() {
                                        action = DetachedPanelAction::Attach;
                                    }
                                },
                            );
                        });

                        ui.separator();

                        egui::Frame::default()
                            .fill(theme::BG_PRIMARY)
                            .show(ui, |ui| {
                                self.dynamic_panels.ui_body(ui, &id);
                            });
                    });

                action
            });

            match action {
                DetachedPanelAction::Attach => {
                    self.detached_dynamic_panels.remove(&id);
                    self.panels.open_tab(PanelKind::Dynamic(id));
                }
                DetachedPanelAction::Close => {
                    self.detached_dynamic_panels.remove(&id);
                    self.dynamic_panels.remove(&id);
                    self.panels.close_tab(PanelKind::Dynamic(id));
                }
                DetachedPanelAction::None => {}
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::R) && !i.modifiers.shift {
                self.refresh_ports();
            }
            if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::O) {
                self.open_selected_port();
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::B) {
                self.toggle_bottom_panel();
            }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::I) {
                self.panels.inspector_visible = !self.panels.inspector_visible;
            }
            if i.modifiers.ctrl {
                for (k, a) in [
                    (egui::Key::Num1, Activity::Devices),
                    (egui::Key::Num2, Activity::Plugins),
                    (egui::Key::Num3, Activity::Settings),
                ] {
                    if i.key_pressed(k) {
                        self.panels.select_activity(a);
                    }
                }
            }
        });
    }

    fn serial_connect_controls(
        &mut self,
        ui: &mut egui::Ui,
        port_combo_id: &'static str,
        baud_combo_id: &'static str,
        port_width: f32,
        baud_width: f32,
        compact: bool,
    ) {
        if !compact {
            ui.label("端口");
        }

        serial_combo(
            ui,
            port_combo_id,
            port_width,
            &self.ports,
            &mut self.selected_port,
        );

        if !compact {
            ui.label("波特率");
        }

        baud_combo(ui, baud_combo_id, baud_width, &mut self.baud_rate);

        let selected_open = self
            .selected_port
            .as_deref()
            .is_some_and(|port| self.transport.status_port(port).open);

        if serial_action_button(ui, "打开").clicked() {
            self.open_selected_port();
        }

        if serial_action_button_enabled(ui, selected_open, "关闭").clicked() {
            if let Some(ref port) = self.selected_port {
                self.transport.close_port(port);
                self.status_message = format!("{port} 已关闭");
            }
        }

        if !compact {
            match self.selected_port.as_deref() {
                Some(port) => {
                    let st = self.transport.status_port(port);

                    if st.open {
                        ui.label(
                            egui::RichText::new(format!(
                                "● {} @ {} {}N{}",
                                port,
                                st.baud_rate.unwrap_or(0),
                                &self.data_bits,
                                &self.stop_bits
                            ))
                            .color(theme::GREEN),
                        );
                    } else {
                        ui.label(egui::RichText::new("○ 未连接").color(theme::TEXT_SECONDARY));
                    }
                }
                None => {
                    ui.label(egui::RichText::new("○ 未选择串口").color(theme::TEXT_SECONDARY));
                }
            }
        }
    }
}

// ══════════════════════════════════════════
//  eframe::App
// ══════════════════════════════════════════

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        theme::BG_PRIMARY.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 终端放大按钮
        if self.terminal_panel.maximize_clicked {
            self.terminal_panel.maximize_clicked = false;
            self.terminal_popup_open = true;
        }
        // 回放清理
        if self.replay_panel.want_clear_on_play {
            self.replay_panel.want_clear_on_play = false;
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();
        }

        if let Some(steps) = self.replay_panel.want_step_backward.take() {
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();

            self.bus.publish(Event::new(
                "ui.replay.reset",
                "ui.replay",
                Direction::Internal,
                Payload::Empty,
            ));

            self.replay_panel.do_step_backward(steps);

            let terminal_count = self.terminal_panel.ingest_all_pending();
            let log_count = self.bottom_log_panel.ingest_all_pending();
            let chart_count = self.dynamic_panels.ingest_all_pending();

            self.status_message = format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            );
            ctx.request_repaint();
        }

        if let Some(p) = self.replay_panel.want_seek_replay.take() {
            self.terminal_panel.clear();
            self.bottom_log_panel.clear();
            self.dynamic_panels.clear_charts();

            self.bus.publish(Event::new(
                "ui.replay.reset",
                "ui.replay",
                Direction::Internal,
                Payload::Empty,
            ));

            self.replay_panel.do_seek_replay(p);

            let terminal_count = self.terminal_panel.ingest_all_pending();
            let log_count = self.bottom_log_panel.ingest_all_pending();
            let chart_count = self.dynamic_panels.ingest_all_pending();

            self.status_message = format!(
                "回放重建完成：接收 {terminal_count} 条，日志 {log_count} 条，图表 {chart_count} 条"
            );
            ctx.request_repaint();
        }
        if self.replay_panel.want_pick_file {
            self.replay_panel.want_pick_file = false;
            if let Some(p) = windows_open_dialog() {
                self.replay_panel.path = p.display().to_string();
                self.replay_panel.auto_load = true;
            }
        }
        // 运行 replay analyzer（如果需要）
        if self.replay_panel.want_run_analyzers {
            self.run_replay_analyzers();
        }

        self.dynamic_panels.ingest(&mut self.panels);
        let n = self.plugin_manager.process_pending();
        if n > 0 {
            self.status_message = format!("{n} 个插件事件");
        }
        self.handle_keys(&ctx);

        // 速率统计
        let now = ctx.input(|i| i.time);
        if self.last_rate_check_time > 0.0 {
            let el = now - self.last_rate_check_time;
            if el >= 1.0 {
                let c = self.bus.published_count();
                self.event_rate = c.saturating_sub(self.last_event_count) as f64 / el;
                self.last_event_count = c;
                self.last_rate_check_time = now;
            }
        } else {
            self.last_rate_check_time = now;
            self.last_event_count = self.bus.published_count();
        }
        if now - self.last_port_refresh > PORT_REFRESH_INTERVAL_SECS {
            self.last_port_refresh = now;
            self.refresh_ports_silent();
        }

        // 面板
        egui::Panel::top("top-bar").show_inside(ui, |ui| self.top_bar(ui));
        egui::Panel::left("activity-bar")
            .resizable(false)
            .default_size(ACTIVITY_BAR_WIDTH)
            .show_inside(ui, |ui| self.activity_bar(ui));

        egui::Panel::right("inspector")
            .resizable(false)
            .exact_size(if self.panels.inspector_visible {
                INSPECTOR_WIDTH
            } else {
                0.0
            })
            .show_separator_line(self.panels.inspector_visible)
            .show_inside(ui, |ui| {
                if self.panels.inspector_visible {
                    self.inspector(ui);
                }
            });

        if self.bottom_panel_visible {
            egui::Panel::bottom("bottom-bar")
                .resizable(true)
                .min_size(BOTTOM_PANEL_MIN)
                .default_size(BOTTOM_PANEL_HEIGHT)
                .show_separator_line(true)
                .show_inside(ui, |ui| self.show_bottom_panel_contents(ui));
        } else {
            egui::Panel::bottom("status-only")
                .resizable(false)
                .show_separator_line(false)
                .default_size(24.0)
                .show_inside(ui, |ui| self.status_bar(ui));
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.dynamic_tab_cleanup();
            if let Some(id) = self.panels.active_dynamic_id().map(str::to_owned) {
                self.dynamic_panel_ui(ui, &id);
            } else {
                match self.panels.activity {
                    Activity::Devices => self.device_panel(ui),
                    Activity::Replay => self.replay_panel.ui(ui),
                    Activity::Plugins => self.plugins_panel.ui(ui, &mut self.plugin_manager),
                    Activity::Settings => self.settings_panel(ui),
                    _ => self.device_panel(ui),
                }
            }
        });

        // 浮动拖拽副本
        if let Some(s) = self.activity_drag_source
            && s < self.activity_order.len()
            && let Some(p) = ctx.pointer_latest_pos()
        {
            let act = self.activity_order[s];
            let label = format!("{} {}", aicon(act), act.label());
            let gal = ctx.fonts_mut(|f| {
                f.layout(
                    label.clone(),
                    egui::FontId::proportional(12.0),
                    theme::TEXT_PRIMARY,
                    f32::INFINITY,
                )
            });
            let rect = egui::Rect::from_min_size(
                p + egui::vec2(8.0, -12.0),
                egui::vec2(gal.size().x + 16.0, 26.0),
            );
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("dghost"),
            ));
            painter.rect_filled(
                rect,
                5.0,
                egui::Color32::from_rgba_premultiplied(46, 80, 120, 210),
            );
            painter.galley(
                rect.center() - gal.size() * 0.5,
                gal,
                egui::Color32::from_rgba_premultiplied(255, 255, 255, 240),
            );
        }
        self.bottom_log_panel.ingest_pending();
        self.detached_dynamic_panel_viewports(&ctx);
        self.send_popup(&ctx);
        self.terminal_popup(&ctx);

        ctx.request_repaint_after(std::time::Duration::from_millis(REPAINT_INTERVAL_MS));
    }
}

// ── 发送放大窗口 ──
impl WorkbenchApp {
    fn terminal_popup(&mut self, ctx: &egui::Context) {
        if !self.terminal_popup_open {
            return;
        }

        let vid = egui::ViewportId::from_hash_of("term-popup");
        let builder = egui::ViewportBuilder::default()
            .with_title("接收区 - 硬件调试工作台")
            .with_inner_size([800.0, 600.0]);

        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }

            egui::CentralPanel::default()
                .show_inside(ui, |ui| {
                    let mut close = false;

                    ui.horizontal(|ui| {
                        ui.heading("接收区");
                        if ui.button("关闭").clicked() {
                            close = true;
                        }
                    });
                    ui.separator();

                    self.terminal_panel.height = (ui.available_height() - 42.0).max(120.0);
                    self.terminal_panel.ui(ui);

                    close
                })
                .inner
        });

        if should_close {
            self.terminal_popup_open = false;
        }
    }
    fn send_popup(&mut self, ctx: &egui::Context) {
        if !self.send_popup_open {
            return;
        }
        let vid = egui::ViewportId::from_hash_of("send-popup");
        let builder = egui::ViewportBuilder::default()
            .with_title("发送 - 硬件调试工作台")
            .with_inner_size([640.0, 480.0])
            .with_min_inner_size([360.0, 260.0]);
        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) {
                return true;
            }
            egui::CentralPanel::default()
                .show_inside(ui, |ui| {
                    let so = self.transport.status().open;
                    let ctrl_enter = ui
                        .ctx()
                        .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.send_hex_mode, false, "文本");
                        ui.radio_value(&mut self.send_hex_mode, true, "HEX");
                        ui.checkbox(&mut self.send_append_lf, "LF");
                        if ui
                            .add_enabled(
                                so && !self.send_input.is_empty(),
                                egui::Button::new("发送 (Ctrl+Enter)"),
                            )
                            .clicked()
                            || (ctrl_enter && so && !self.send_input.is_empty())
                        {
                            self.do_send();
                        }
                        if ui.button("清空").clicked() {
                            self.send_input.clear();
                            self.send_error = None;
                        }
                    });
                    ui.separator();
                    ui.add(
                        egui::TextEdit::multiline(&mut self.send_input)
                            .desired_width(f32::INFINITY)
                            .desired_rows(24)
                            .hint_text("Ctrl+Enter 发送"),
                    );
                    if let Some(ref e) = self.send_error {
                        ui.colored_label(theme::RED, translate_error(e));
                    }
                    false
                })
                .inner
        });
        if should_close {
            self.send_popup_open = false;
        }
    }

    /// 运行所有已发现插件的 replay analyzer，结果注入 ReplayManager。
    fn run_replay_analyzers(&mut self) {
        let entries = self.plugin_manager.replay_analyzer_entries();
        if entries.is_empty() {
            self.replay_panel
                .set_analyzer_error("没有可用的 replay analyzer".to_owned());
            self.status_message = "回放：没有可用的 replay analyzer".to_owned();
            return;
        }

        let raw_events = self.replay_panel.manager().raw_serial_events();
        if raw_events.is_empty() {
            self.replay_panel
                .set_analyzer_error("录制文件中没有原始串口事件".to_owned());
            self.status_message = "回放：录制文件中没有原始串口事件".to_owned();
            return;
        }

        let mut all_derived = Vec::new();
        let mut error_count = 0;

        for entry in &entries {
            let replay_config = match &entry.manifest.replay {
                Some(cfg) => cfg,
                None => continue,
            };

            let script_path = entry.root.join(&replay_config.main);
            let script = match std::fs::read_to_string(&script_path) {
                Ok(s) => s,
                Err(e) => {
                    error_count += 1;
                    self.replay_panel
                        .set_analyzer_error(format!("读取 {} 失败: {e}", script_path.display()));
                    continue;
                }
            };

            let config = LuaReplayConfig {
                script_name: format!("replay:{}:{}", entry.plugin_id, replay_config.main),
                plugin_id: entry.plugin_id.clone(),
                plugin_version: entry.manifest.version.clone(),
                subscriptions: replay_config.subscriptions.clone(),
                context: serde_json::json!({
                    "id": entry.manifest.id,
                    "name": entry.manifest.name,
                    "version": entry.manifest.version,
                }),
            };

            match run_replay_analyzer(script, config, &raw_events) {
                Ok(output) => {
                    for log_msg in &output.logs {
                        self.log(LogLevel::Info, log_msg);
                    }
                    all_derived.extend(output.events);
                }
                Err(e) => {
                    error_count += 1;
                    self.log(
                        LogLevel::Warn,
                        &format!("analyzer {} 失败: {e}", entry.plugin_id),
                    );
                }
            }
        }

        if all_derived.is_empty() {
            self.replay_panel
                .set_analyzer_error("所有 analyzer 运行失败（终端可显示原始事件）".to_owned());
            self.status_message = "回放：analyzer 运行失败（终端可显示原始事件）".to_owned();
            return;
        }

        all_derived.sort_by_key(|e| (e.timestamp_ms, e.id));
        let count = all_derived.len();
        self.replay_panel.set_analyzer_cache(all_derived);

        if error_count > 0 {
            self.status_message = format!(
                "回放 analyzer 部分完成：{count} 个派生事件，{error_count} 个 analyzer 失败"
            );
        } else {
            self.status_message = format!("回放 analyzer 完成：{count} 个派生事件");
        }
        let msg = format!(
            "replay analyzers produced {count} derived events from {} raw events ({} plugins)",
            raw_events.len(),
            entries.len()
        );
        self.log(LogLevel::Info, &msg);
    }
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        self.save_config();
        self.recorder.stop();
        self.transport.close_serial();
    }
}

// ══════════════════════════════════════════
//  辅助函数
// ══════════════════════════════════════════

fn pdb(v: &str) -> DataBits {
    match v {
        "5" => DataBits::Five,
        "6" => DataBits::Six,
        "7" => DataBits::Seven,
        _ => DataBits::Eight,
    }
}
fn psb(v: &str) -> StopBits {
    match v {
        "2" => StopBits::Two,
        _ => StopBits::One,
    }
}
fn ppar(v: &str) -> Parity {
    match v {
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        _ => Parity::None,
    }
}
fn serial_combo(
    ui: &mut egui::Ui,
    id: &'static str,
    w: f32,
    ports: &[SerialPortDescriptor],
    sel: &mut Option<String>,
) {
    let selected_text = match sel.as_deref() {
        Some(port) => port.to_owned(),
        None if ports.is_empty() => "无端口".to_owned(),
        None => "请选择串口".to_owned(),
    };

    egui::ComboBox::from_id_salt(id)
        .width(w)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            if ports.is_empty() {
                ui.add_enabled(false, egui::Label::new("无可用串口"));
            } else {
                for port in ports {
                    ui.selectable_value(sel, Some(port.port_name.clone()), &port.port_name);
                }
            }
        });
}

fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
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
fn send_impl(
    input: &str,
    hex: bool,
    lf: bool,
    t: &TransportManager,
) -> Result<(), tool_transport::TransportError> {
    if input.trim().is_empty() {
        return Ok(());
    }
    if hex {
        for line in input.lines() {
            let x = line.trim();
            if x.is_empty() {
                continue;
            }
            t.send_hex(x)?;
        }
        Ok(())
    } else {
        let mut text = input.to_owned();
        if lf {
            text.push('\n');
        }
        t.send_text(&text)
    }
}
fn translate_error(m: &str) -> String {
    if m.contains("no serial") {
        "串口未打开".into()
    } else if m.contains("invalid hex") {
        format!("无效HEX: {}", m.trim_start_matches("invalid hex input: "))
    } else {
        m.to_owned()
    }
}
fn load_config() -> Option<PersistedConfig> {
    let t = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str(&t).ok()
}
fn config_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("workspace.json")
}
fn windows_open_dialog() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter("JSONL", &["jsonl"])
        .set_directory("logs")
        .pick_file()
}
fn pick_recorder_path(current: &str) -> Option<PathBuf> {
    let current_path = PathBuf::from(current);

    let mut dialog = rfd::FileDialog::new().add_filter("JSONL", &["jsonl"]);

    if let Some(parent) = current_path.parent()
        && !parent.as_os_str().is_empty()
    {
        dialog = dialog.set_directory(parent);
    } else {
        dialog = dialog.set_directory("logs");
    }

    if let Some(file_name) = current_path.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(file_name);
    } else {
        dialog = dialog.set_file_name(format!("session-{}.jsonl", now_timestamp_ms()));
    }

    dialog.save_file().map(ensure_jsonl_extension)
}

fn ensure_jsonl_extension(mut path: PathBuf) -> PathBuf {
    let is_jsonl = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"));

    if !is_jsonl {
        path.set_extension("jsonl");
    }

    path
}
fn record_mode_label(mode: RecordMode) -> &'static str {
    match mode {
        RecordMode::StandardReplay => "标准回放",
        RecordMode::RawSerial => "原始串口",
        RecordMode::FullDebug => "完整调试",
    }
}

fn default_recorder_path() -> String {
    format!("logs/session-{}.jsonl", now_timestamp_ms())
}
fn aicon(a: Activity) -> &'static str {
    match a {
        Activity::Devices => "📟",
        Activity::Replay => "⏪",
        Activity::Plugins => "🧩",
        Activity::Settings => "⚙",
        _ => "",
    }
}
fn ashortcut(a: Activity) -> &'static str {
    match a {
        Activity::Devices => "Ctrl+1",
        Activity::Plugins => "Ctrl+2",
        Activity::Settings => "Ctrl+3",
        _ => "",
    }
}

const SERIAL_ACTION_BUTTON_SIZE: egui::Vec2 = egui::vec2(52.0, 26.0);

fn serial_action_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized(SERIAL_ACTION_BUTTON_SIZE, egui::Button::new(text))
}

fn serial_action_button_enabled(ui: &mut egui::Ui, enabled: bool, text: &str) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(text).min_size(SERIAL_ACTION_BUTTON_SIZE),
    )
}
fn activity_insert_index_from_pointer(rects: &[egui::Rect], pointer: egui::Pos2) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let top = rects.first()?.top() - 14.0;
    let bottom = rects.last()?.bottom() + 14.0;

    if pointer.x < left - 16.0 || pointer.x > right + 16.0 || pointer.y < top || pointer.y > bottom
    {
        return None;
    }

    for (index, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return Some(index);
        }
    }

    Some(rects.len())
}

fn paint_activity_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    if rects.is_empty() {
        return;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let y = if insert_index == 0 {
        rects[0].top() - 3.0
    } else if insert_index >= rects.len() {
        rects[rects.len() - 1].bottom() + 3.0
    } else {
        let above = rects[insert_index - 1];
        let below = rects[insert_index];
        (above.bottom() + below.top()) * 0.5
    };

    let painter = ui.painter();

    painter.line_segment(
        [egui::pos2(left + 6.0, y), egui::pos2(right - 6.0, y)],
        egui::Stroke::new(2.0, theme::BLUE),
    );

    painter.circle_filled(egui::pos2(left + 6.0, y), 3.0, theme::BLUE);
    painter.circle_filled(egui::pos2(right - 6.0, y), 3.0, theme::BLUE);
}
fn vertical_insert_index_from_pointer(rects: &[egui::Rect], pointer: egui::Pos2) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let top = rects.first()?.top() - 10.0;
    let bottom = rects.last()?.bottom() + 10.0;

    if pointer.x < left - 16.0 || pointer.x > right + 16.0 || pointer.y < top || pointer.y > bottom
    {
        return None;
    }

    for (index, rect) in rects.iter().enumerate() {
        if pointer.y < rect.center().y {
            return Some(index);
        }
    }

    Some(rects.len())
}

fn paint_vertical_insert_line(ui: &egui::Ui, rects: &[egui::Rect], insert_index: usize) {
    if rects.is_empty() {
        return;
    }

    let left = rects
        .iter()
        .map(|rect| rect.left())
        .fold(f32::INFINITY, f32::min);

    let right = rects
        .iter()
        .map(|rect| rect.right())
        .fold(f32::NEG_INFINITY, f32::max);

    let y = if insert_index == 0 {
        rects[0].top() - 3.0
    } else if insert_index >= rects.len() {
        rects[rects.len() - 1].bottom() + 3.0
    } else {
        let above = rects[insert_index - 1];
        let below = rects[insert_index];
        (above.bottom() + below.top()) * 0.5
    };

    let painter = ui.painter();

    painter.line_segment(
        [egui::pos2(left + 6.0, y), egui::pos2(right - 6.0, y)],
        egui::Stroke::new(2.0, theme::BLUE),
    );

    painter.circle_filled(egui::pos2(left + 6.0, y), 3.0, theme::BLUE);
    painter.circle_filled(egui::pos2(right - 6.0, y), 3.0, theme::BLUE);
}
