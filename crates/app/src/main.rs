// Release 模式下不显示控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use tool_core::{Event, LogLevel, now_timestamp_ms};
use tool_databus::DataBus;
use tool_extension::PluginManager;
use tool_panels::{
    Activity, DynamicPanels, LogPanel, PanelKind, PanelManager,
    PluginsPanel, ReplayPanel, TerminalPanel, theme,
};
use tool_recorder::JsonlRecorder;
use tool_transport::{DataBits, Parity, SerialConfig, SerialPortDescriptor, StopBits, TransportManager};

const ACTIVITY_BAR_WIDTH: f32 = 104.0;
const BOTTOM_PANEL_HEIGHT: f32 = 300.0;
const BOTTOM_PANEL_MIN: f32 = 250.0;
const INSPECTOR_WIDTH: f32 = 240.0;
const DEFAULT_WINDOW_WIDTH: f32 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f32 = 820.0;
const REPAINT_INTERVAL_MS: u64 = 50;
const PORT_REFRESH_INTERVAL_SECS: f64 = 2.0;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([DEFAULT_WINDOW_WIDTH, DEFAULT_WINDOW_HEIGHT])
            .with_min_inner_size([960.0, 640.0]),
        persist_window: true,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration { present_mode: eframe::wgpu::PresentMode::Immediate, ..Default::default() },
        ..Default::default()
    };
    eframe::run_native("硬件调试工作台", options, Box::new(|cc| Ok(Box::new(WorkbenchApp::new(cc)))))
}

// ── 字体 ──
fn setup_fonts(cc: &eframe::CreationContext<'_>) {
    for path in &["assets/NotoSansSC-VF.ttf".to_owned(), "C:\\Windows\\Fonts\\msyh.ttc".to_owned()] {
        if let Ok(fb) = std::fs::read(path) {
            let mut f = egui::FontDefinitions::default();
            f.font_data.insert("zh".into(), egui::FontData::from_owned(fb).into());
            if let Ok(eb) = std::fs::read("assets/seguiemj.ttf") { f.font_data.insert("emoji".into(), egui::FontData::from_owned(eb).into()); }
            let p = f.families.entry(egui::FontFamily::Proportional).or_default();
            p.insert(0, "zh".into()); if f.font_data.contains_key("emoji") { p.insert(0, "emoji".into()); }
            f.families.entry(egui::FontFamily::Monospace).or_default().push("zh".into());
            cc.egui_ctx.set_fonts(f); return;
        }
    }
}

// ── 主题 ──
fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
    let mut s = (*ctx.global_style()).clone();
    s.spacing.item_spacing = egui::vec2(8.0, 6.0); s.spacing.button_padding = egui::vec2(10.0, 5.0);
    s.spacing.interact_size = egui::vec2(40.0, 28.0); s.spacing.slider_width = 180.0; s.spacing.combo_width = 140.0; s.spacing.text_edit_width = 220.0;
    s.interaction.resize_grab_radius_side = 6.0; s.interaction.resize_grab_radius_corner = 10.0;
    s.animation_time = 0.0;
    #[cfg(debug_assertions)] { s.debug.show_interactive_widgets = false; s.debug.show_focused_widget = false; s.debug.show_unaligned = false; s.debug.warn_if_rect_changes_id = false; s.debug.show_resize = false; s.debug.show_widget_hits = false; }
    let mut v = egui::Visuals::dark();
    v.panel_fill = theme::BG_PRIMARY; v.window_fill = theme::BG_SECONDARY; v.extreme_bg_color = theme::BG_SECONDARY;
    v.faint_bg_color = theme::BG_TERTIARY; v.code_bg_color = theme::BG_INPUT; v.text_edit_bg_color = Some(theme::BG_INPUT);
    v.override_text_color = Some(theme::TEXT_PRIMARY); v.weak_text_color = Some(theme::TEXT_SECONDARY);
    v.warn_fg_color = theme::YELLOW; v.error_fg_color = theme::RED;
    v.selection.bg_fill = theme::BG_SELECTION; v.selection.stroke = egui::Stroke::new(1.0, theme::BLUE);
    v.hyperlink_color = theme::CYAN; v.window_stroke = egui::Stroke::new(1.0, theme::BORDER_LIGHT);
    v.resize_corner_size = 8.0; v.striped = true; v.collapsing_header_frame = false; v.window_highlight_topmost = false; v.button_frame = true; v.indent_has_left_vline = false;
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = theme::BG_INPUT; w.noninteractive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER); w.noninteractive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY); w.noninteractive.weak_bg_fill = theme::BG_SECONDARY;
    w.inactive.bg_fill = theme::BG_TERTIARY; w.inactive.weak_bg_fill = theme::BG_INPUT; w.inactive.bg_stroke = egui::Stroke::new(1.0, theme::BORDER_LIGHT); w.inactive.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    w.hovered.bg_fill = theme::WIDGET_HOVER; w.hovered.weak_bg_fill = theme::BG_TERTIARY; w.hovered.bg_stroke = egui::Stroke::new(1.0, theme::BLUE); w.hovered.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    w.active.bg_fill = theme::WIDGET_ACTIVE_WEAK; w.active.weak_bg_fill = theme::WIDGET_ACTIVE_WEAK; w.active.bg_stroke = egui::Stroke::new(1.0, theme::BLUE); w.active.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_WHITE);
    w.open.bg_fill = theme::WIDGET_OPEN; w.open.weak_bg_fill = theme::BG_INPUT; w.open.bg_stroke = egui::Stroke::new(1.0, theme::BLUE); w.open.fg_stroke = egui::Stroke::new(1.0, theme::TEXT_PRIMARY);
    s.visuals = v; ctx.set_global_style(s);
}

// ── 数据结构 ──
#[derive(Clone, Copy, PartialEq, Eq)]
enum DetachedPanelAction { None, Attach, Close }

#[derive(Clone, Copy, PartialEq, Eq)]
enum BottomTab { Terminal, Logs }

struct WorkbenchApp {
    bus: DataBus, transport: TransportManager, plugin_manager: PluginManager, recorder: JsonlRecorder,
    panels: PanelManager, terminal_panel: TerminalPanel,
    dynamic_panels: DynamicPanels, plugins_panel: PluginsPanel, replay_panel: ReplayPanel, bottom_log_panel: LogPanel,
    ports: Vec<SerialPortDescriptor>, selected_port: Option<String>,
    baud_rate: String, data_bits: String, stop_bits: String, parity: String, timeout_ms: String,
    recorder_path: String, status_message: String, last_port_refresh: f64,
    bottom_panel_visible: bool, bottom_tab: BottomTab, send_input: String, send_hex_mode: bool, send_append_lf: bool, send_error: Option<String>,
    send_popup_open: bool, terminal_popup_open: bool,
    detached_dynamic_panels: BTreeSet<String>, top_bar_serial_collapsed: bool,
    activity_order: Vec<Activity>, activity_drag_source: Option<usize>, activity_rects_cache: Vec<egui::Rect>,
    last_rate_check_time: f64, last_event_count: usize, event_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedConfig {
    panels: PanelManager, selected_port: Option<String>, baud_rate: String,
    data_bits: String, stop_bits: String, parity: String, timeout_ms: String, recorder_path: String,
    #[serde(default = "default_activity_order")] activity_order: Vec<Activity>,
    #[serde(default)] enabled_plugins: Vec<String>,
}

fn default_activity_order() -> Vec<Activity> { vec![Activity::Devices, Activity::Replay, Activity::Plugins, Activity::Settings] }

// ══════════════════════════════════════════
//  WorkbenchApp impl
// ══════════════════════════════════════════

impl WorkbenchApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(cc);
        cc.egui_ctx.set_embed_viewports(false);
        let bus = DataBus::new(); let transport = TransportManager::new(bus.clone());
        let mut pm = PluginManager::new(bus.clone(), transport.clone());
        if let Err(e) = pm.discover_roots([PathBuf::from("plugins")]) { bus.publish(Event::system_log(LogLevel::Error, "ext", format!("plugin discover: {e}"))); }
        let recorder = JsonlRecorder::new(bus.clone());
        let config = load_config();
        apply_theme(&cc.egui_ctx);
        let mut rp = config.as_ref().map(|c| c.panels.clone()).unwrap_or_default(); rp.discard_dynamic_tabs();

        let mut app = Self {
            terminal_panel: TerminalPanel::new(&bus), dynamic_panels: DynamicPanels::new(&bus),
            plugins_panel: PluginsPanel::new(), replay_panel: ReplayPanel::new(&bus), bottom_log_panel: LogPanel::new(&bus),
            ports: Vec::new(),
            selected_port: config.as_ref().and_then(|c| c.selected_port.clone()),
            baud_rate: config.as_ref().map(|c| c.baud_rate.clone()).unwrap_or_else(|| "115200".into()),
            data_bits: config.as_ref().map(|c| c.data_bits.clone()).unwrap_or_else(|| "8".into()),
            stop_bits: config.as_ref().map(|c| c.stop_bits.clone()).unwrap_or_else(|| "1".into()),
            parity: config.as_ref().map(|c| c.parity.clone()).unwrap_or_else(|| "none".into()),
            timeout_ms: config.as_ref().map(|c| c.timeout_ms.clone()).unwrap_or_else(|| "50".into()),
            recorder_path: config.as_ref().map(|c| c.recorder_path.clone()).unwrap_or_else(default_recorder_path),
            panels: rp.clone(), status_message: "就绪".into(), last_port_refresh: 0.0,
            bottom_panel_visible: rp.bottom_logs_visible, bottom_tab: BottomTab::Terminal,
            send_input: String::new(), send_hex_mode: false, send_append_lf: false, send_error: None,
            send_popup_open: false, terminal_popup_open: false,
            detached_dynamic_panels: BTreeSet::new(), top_bar_serial_collapsed: false,
            activity_order: config.as_ref().map(|c| c.activity_order.clone()).unwrap_or_else(default_activity_order),
            activity_drag_source: None, activity_rects_cache: Vec::new(),
            last_rate_check_time: 0.0, last_event_count: 0, event_rate: 0.0,
            bus, transport, plugin_manager: pm, recorder,
        };
        app.refresh_ports();
        let enabled: Vec<String> = config.as_ref().map(|c| c.enabled_plugins.clone()).unwrap_or_default();
        for id in &enabled { if let Err(e) = app.plugin_manager.enable(id) { app.log(LogLevel::Warn, format!("restore plugin {id}: {e}")); } }
        app.log(LogLevel::Info, "就绪"); app
    }

    fn log(&self, lv: LogLevel, m: impl Into<String>) { self.bus.publish(Event::system_log(lv, "app", m.into())); }
    fn refresh_ports(&mut self) {
        match self.transport.list_serial_ports() {
            Ok(p) => { self.ports = p; if self.selected_port.as_ref().is_none_or(|s| !self.ports.iter().any(|x| &x.port_name == s)) { self.selected_port = self.ports.first().map(|x| x.port_name.clone()); } self.status_message = format!("{} 个串口", self.ports.len()); }
            Err(e) => { self.status_message = e.to_string(); }
        }
    }
    fn open_selected_port(&mut self) {
        let Some(ref p) = self.selected_port.clone() else { self.log(LogLevel::Warn, "请选择串口"); return; };
        let b = self.baud_rate.parse().unwrap_or(115200);
        let cfg = SerialConfig { port_name: p.clone(), baud_rate: b, data_bits: pdb(&self.data_bits), stop_bits: psb(&self.stop_bits), parity: ppar(&self.parity), timeout_ms: self.timeout_ms.parse().unwrap_or(50) };
        match self.transport.open_serial(cfg) {
            Ok(()) => { self.status_message = format!("{p} 已连接"); self.bottom_panel_visible = true; }
            Err(e) => { self.status_message = e.to_string(); }
        }
    }
    fn start_or_stop_recording(&mut self) {
        if self.recorder.is_running() { self.recorder.stop(); self.status_message = "录制已停止".into(); }
        else { match self.recorder.start(PathBuf::from(&self.recorder_path)) { Ok(()) => { self.status_message = "录制中".into(); } Err(e) => { self.status_message = e.to_string(); } } }
    }
    fn save_config(&mut self) {
        self.panels.bottom_logs_visible = self.bottom_panel_visible;
        let mut p = self.panels.clone(); p.discard_dynamic_tabs(); p.bottom_logs_visible = self.bottom_panel_visible;
        let cfg = PersistedConfig { panels: p, selected_port: self.selected_port.clone(), baud_rate: self.baud_rate.clone(), data_bits: self.data_bits.clone(), stop_bits: self.stop_bits.clone(), parity: self.parity.clone(), timeout_ms: self.timeout_ms.clone(), recorder_path: self.recorder_path.clone(), activity_order: self.activity_order.clone(), enabled_plugins: self.plugin_manager.summaries().into_iter().filter(|s| matches!(s.state, tool_extension::PluginState::Enabled | tool_extension::PluginState::Running)).map(|s| s.id).collect() };
        if let Ok(t) = serde_json::to_string_pretty(&cfg) { let _ = std::fs::write(config_path(), t); }
    }
    fn toggle_bottom_panel(&mut self) { self.bottom_panel_visible = !self.bottom_panel_visible; if self.bottom_panel_visible { self.status_message = "底部面板已打开".into(); } }

    // ── UI 组件 ──

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.heading("检查器");
        let st = self.transport.status();
        ui.label(egui::RichText::new("串口").strong());
        if st.open { ui.colored_label(theme::GREEN, format!("● {} @ {}", st.port_name.as_deref().unwrap_or("?"), st.baud_rate.unwrap_or(0))); }
        else { ui.colored_label(theme::TEXT_SECONDARY, "○ 已关闭"); }
        ui.separator();
        ui.label(egui::RichText::new("录制").strong());
        ui.label(if self.recorder.is_running() { "⏺ 运行中" } else { "⏹ 已停止" });
        if let Some(p) = self.recorder.current_path() { ui.monospace(p.display().to_string()); }
        ui.separator();
        ui.label(egui::RichText::new("运行时").strong());
        ui.label(format!("插件: {}", self.plugin_manager.count()));
        ui.label(format!("动态面板: {}", self.dynamic_panels.count()));
        if let Some(e) = self.dynamic_panels.last_error() { ui.colored_label(theme::RED, e); }
        ui.separator();
        ui.label(egui::RichText::new("DataBus").strong());
        ui.label(format!("事件 {} | {:.0}/s", self.bus.history().len(), self.event_rate));
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let st = self.transport.status();
        ui.horizontal(|ui| {
            let (d, l) = if let (Some(p), Some(b)) = (st.port_name.clone(), st.baud_rate) {
                (if st.open { "●" } else { "○" }, format!("{p} @ {b}"))
            } else { ("○", "串口已关闭".into()) };
            ui.label(egui::RichText::new(d).color(if st.open { theme::GREEN } else { theme::TEXT_SECONDARY })); ui.label(l); ui.separator();
            let rec = self.recorder.is_running();
            ui.label(egui::RichText::new("●").color(if rec { theme::RED } else { theme::TEXT_SECONDARY }));
            ui.label(if rec { "录制中" } else { "未录制" }); ui.separator();
            ui.label(format!("{:.0} 事件/秒", self.event_rate)); ui.separator();
            ui.label(&self.status_message);
        });
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let mut new_rects = Vec::new();
        let dragging = self.activity_drag_source;
        let pointer = ui.ctx().pointer_latest_pos();
        let drag_target: Option<usize> = if let Some(s) = dragging && let Some(p) = pointer {
            self.activity_rects_cache.iter().enumerate().find(|(i,r)| *i != s && r.contains(p)).map(|(i,_)| i)
        } else { None };

        ui.vertical_centered(|ui| {
            for (idx, &act) in self.activity_order.iter().enumerate() {
                let selected = self.panels.activity == act;
                let label = format!("{} {}", aicon(act), act.label());
                let sh = ashortcut(act);
                let hover = if sh.is_empty() { act.label().to_owned() } else { format!("{} ({})", act.label(), sh) };
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click_and_drag());
                let is_src = dragging == Some(idx); let is_tgt = drag_target == Some(idx);
                let bg = if is_src { theme::BG_TERTIARY } else if is_tgt { theme::BG_SELECTION } else if selected || resp.hovered() { if selected { theme::BG_SELECTION } else { theme::WIDGET_HOVER } } else { theme::BG_SECONDARY };
                let p = ui.painter_at(rect); p.rect_filled(rect, 4.0, bg);
                if is_tgt { p.rect_stroke(rect, 4.0, egui::Stroke::new(2.0, theme::BLUE), egui::StrokeKind::Inside); }
                p.text(rect.center(), egui::Align2::CENTER_CENTER, &label, egui::FontId::proportional(12.0), if is_src { theme::TEXT_SECONDARY } else { theme::TEXT_PRIMARY });
                if resp.clicked() { self.panels.select_activity(act); }
                if resp.dragged() && self.activity_drag_source.is_none() { self.activity_drag_source = Some(idx); }
                resp.on_hover_text(hover); new_rects.push(rect);
            }
        });
        self.activity_rects_cache = new_rects;
        // 拖拽释放
        if self.activity_drag_source.is_some() && ui.input(|i| i.pointer.any_released()) {
            if let Some(s) = self.activity_drag_source.take() && pointer.is_some() {
                if let Some(t) = drag_target && t != s { let item = self.activity_order.remove(s); self.activity_order.insert(t, item); self.save_config(); }
            } else { self.activity_drag_source = None; }
        }
        if self.activity_drag_source.is_some() && !ui.input(|i| i.pointer.primary_down()) { self.activity_drag_source = None; }

        // 动态面板（插件子条目）
        let ids: Vec<(String, String)> = self.panels.tabs.iter()
            .filter_map(|k| k.dynamic_id().map(|id| id.to_owned()))
            .filter(|id| self.dynamic_panels.contains(id))
            .map(|id| { let t = self.dynamic_panels.title(&id).unwrap_or(&id).to_owned(); (id, t) }).collect();
        if !ids.is_empty() {
            ui.separator();
            for (id, title) in &ids {
                let active = self.panels.active_dynamic_id() == Some(id);
                if ui.selectable_label(active, format!("  {title}")).clicked() { self.panels.open_tab(PanelKind::Dynamic(id.clone())); }
            }
        }

        ui.separator();
        if ui.selectable_label(self.bottom_panel_visible, "▽ 终端区").on_hover_text("Ctrl+B").clicked() { self.toggle_bottom_panel(); }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let so = self.transport.status().open;
            let sl = if so { format!("串口 ▸ {}", self.transport.status().port_name.as_deref().unwrap_or("?")) } else { "串口 ▸ 未连接".into() };
            if ui.selectable_label(!self.top_bar_serial_collapsed, egui::RichText::new(format!("{} {sl}", if so { "●" } else { "○" })).color(if so { theme::GREEN } else { theme::RED })).clicked() { self.top_bar_serial_collapsed = !self.top_bar_serial_collapsed; }
            if !self.top_bar_serial_collapsed {
                serial_combo(ui, "top-port", 130.0, &self.ports, &mut self.selected_port);
                baud_combo(ui, "top-baud", 80.0, &mut self.baud_rate);
                if ui.small_button("打开").clicked() { self.open_selected_port(); }
                if ui.small_button("关闭").clicked() { self.transport.close_serial(); self.status_message = "已关闭".into(); }
            }
            ui.separator();
            let rec = self.recorder.is_running();
            if ui.button(if rec { egui::RichText::new("⏹ 停止").color(theme::RED) } else { egui::RichText::new("⏺ 录制").color(theme::TEXT_SECONDARY) }).clicked() { self.start_or_stop_recording(); }
            if ui.small_button("保存布局").clicked() { self.save_config(); self.status_message = "布局已保存".into(); }
        });
    }

    fn send_bar(&mut self, ui: &mut egui::Ui) {
        let so = self.transport.status().open;
        ui.horizontal(|ui| { ui.label("发送"); ui.radio_value(&mut self.send_hex_mode, false, "文本"); ui.radio_value(&mut self.send_hex_mode, true, "HEX"); ui.checkbox(&mut self.send_append_lf, "LF"); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ui.small_button("⛶").on_hover_text("放大编辑").clicked() { self.send_popup_open = true; } }); });
        if so { ui.add(egui::TextEdit::multiline(&mut self.send_input).desired_width(f32::INFINITY).desired_rows(5).hint_text("Ctrl+Enter 发送 | ⛶ 放大编辑")); }
        else { ui.add(egui::TextEdit::multiline(&mut self.send_input).desired_width(f32::INFINITY).desired_rows(5).interactive(false).hint_text("请先打开串口")); }
        let ctrl_enter = ui.ctx().input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if ui.add_enabled(so && !self.send_input.is_empty(), egui::Button::new("发送")).clicked() || (ctrl_enter && so && !self.send_input.is_empty()) { self.do_send(); }
            if ui.button("清空").clicked() { self.send_input.clear(); self.send_error = None; }
            if !so { ui.colored_label(theme::YELLOW, "⚠ 请先打开串口"); }
            if let Some(ref e) = self.send_error { ui.colored_label(theme::RED, translate_error(e)); }
        });
    }

    fn do_send(&mut self) { self.send_error = send_impl(&self.send_input, self.send_hex_mode, self.send_append_lf, &self.transport).err().map(|e| e.to_string()); }

    fn show_bottom_panel_contents(&mut self, ui: &mut egui::Ui) {
        let pm = ui.max_rect();
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            self.status_bar(ui); ui.separator(); self.send_bar(ui); ui.separator();
            ui.horizontal(|ui| {
                if ui.selectable_label(self.bottom_tab == BottomTab::Terminal, "接收").clicked() { self.bottom_tab = BottomTab::Terminal; }
                if ui.selectable_label(self.bottom_tab == BottomTab::Logs, "日志").clicked() { self.bottom_tab = BottomTab::Logs; }
            });
            ui.separator();
            match self.bottom_tab {
                BottomTab::Terminal => self.terminal_panel.ui(ui),
                BottomTab::Logs => self.bottom_log_panel.ui(ui),
            }
        });
        ui.expand_to_include_rect(pm);
    }

    fn device_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设备");
        ui.horizontal(|ui| { ui.label("端口"); serial_combo(ui, "dev-port", 180.0, &self.ports, &mut self.selected_port); ui.label("波特率"); baud_combo(ui, "dev-baud", 90.0, &mut self.baud_rate); });
        ui.horizontal(|ui| {
            ui.label("数据位"); egui::ComboBox::from_id_salt("dev-db").width(60.0).selected_text(&self.data_bits).show_ui(ui, |ui| { for &v in &["5","6","7","8"] { ui.selectable_value(&mut self.data_bits, v.to_owned(), v); } });
            ui.label("停止位"); egui::ComboBox::from_id_salt("dev-sb").width(60.0).selected_text(&self.stop_bits).show_ui(ui, |ui| { for &v in &["1","2"] { ui.selectable_value(&mut self.stop_bits, v.to_owned(), v); } });
            ui.label("校验"); egui::ComboBox::from_id_salt("dev-par").width(70.0).selected_text(&self.parity).show_ui(ui, |ui| { for &(v,l) in &[("none","无"),("odd","奇"),("even","偶")] { ui.selectable_value(&mut self.parity, v.to_owned(), l); } });
            ui.label("超时(ms)"); ui.add(egui::TextEdit::singleline(&mut self.timeout_ms).desired_width(50.0));
        });
        let st = self.transport.status();
        ui.horizontal(|ui| {
            if st.open { ui.label(egui::RichText::new(format!("● {} @ {} {}N{}", st.port_name.as_deref().unwrap_or("?"), st.baud_rate.unwrap_or(0), &self.data_bits, &self.stop_bits)).color(theme::GREEN)); }
            else { ui.label(egui::RichText::new("○ 未连接").color(theme::TEXT_SECONDARY)); }
            if ui.button("打开").clicked() { self.open_selected_port(); }
            if ui.add_enabled(st.open, egui::Button::new("关闭")).clicked() { self.transport.close_serial(); self.status_message = "已关闭".into(); }
        });
        ui.separator(); ui.heading("录制"); ui.horizontal(|ui| { ui.label("路径"); ui.text_edit_singleline(&mut self.recorder_path); if ui.button(if self.recorder.is_running() { "停止" } else { "录制" }).clicked() { self.start_or_stop_recording(); } });
        ui.separator(); ui.heading("可用端口"); egui::ScrollArea::vertical().show(ui, |ui| { for port in &self.ports { ui.monospace(&port.port_name); } });
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置"); ui.separator();
        ui.heading("外观"); ui.checkbox(&mut self.bottom_panel_visible, "底部面板"); ui.checkbox(&mut self.panels.inspector_visible, "检查器");
        ui.separator(); ui.heading("快捷键");
        ui.label("Ctrl+R 刷新  Ctrl+Shift+O 打开  Ctrl+B 底部  Ctrl+I 检查器  Ctrl+1~3 切换");
        ui.separator(); ui.label("硬件调试工作台 v0.1.0");
    }

    // ── 动态面板辅助 ──

    fn dynamic_tab_cleanup(&mut self) {
        let stale: Vec<String> = self.panels.tabs.iter().filter_map(|k| k.dynamic_id().map(str::to_owned)).filter(|id| !self.dynamic_panels.contains(id)).collect();
        for id in stale { self.detached_dynamic_panels.remove(&id); self.panels.close_tab(PanelKind::Dynamic(id)); }
    }

    fn dynamic_panel_ui(&mut self, ui: &mut egui::Ui, id: &str) {
        let title = self.dynamic_panels.title(id).unwrap_or(id).to_owned();
        ui.horizontal(|ui| { ui.heading(&title); if self.detached_dynamic_panels.contains(id) { if ui.button("↙").clicked() { self.detached_dynamic_panels.remove(id); } } else if ui.button("↗").clicked() { self.detached_dynamic_panels.insert(id.to_owned()); } });
        ui.separator();
        if self.detached_dynamic_panels.contains(id) { ui.label("已弹出到独立窗口"); return; }
        self.dynamic_panels.ui_body(ui, id);
    }

    fn detached_dynamic_panel_viewports(&mut self, ctx: &egui::Context) {
        let ids: Vec<String> = self.detached_dynamic_panels.iter().cloned().collect();
        for id in ids {
            if !self.dynamic_panels.contains(&id) { self.detached_dynamic_panels.remove(&id); continue; }
            let title = self.dynamic_panels.title(&id).unwrap_or(&id).to_owned();
            let vid = egui::ViewportId::from_hash_of(("dp", id.as_str()));
            let builder = egui::ViewportBuilder::default().with_title(format!("{title} - 硬件调试工作台")).with_inner_size([900.0, 640.0]);
            let action = ctx.show_viewport_immediate(vid, builder, |ui, _| {
                if ui.ctx().input(|i| i.viewport().close_requested()) { DetachedPanelAction::Close }
                else {
                    let mut act = DetachedPanelAction::None;
                    ui.horizontal(|ui| { ui.heading(&title); if ui.button("↙").clicked() { act = DetachedPanelAction::Attach; } });
                    ui.separator(); self.dynamic_panels.ui_body(ui, &id); act
                }
            });
            match action { DetachedPanelAction::Attach => { self.detached_dynamic_panels.remove(&id); self.panels.open_tab(PanelKind::Dynamic(id)); } DetachedPanelAction::Close => { self.detached_dynamic_panels.remove(&id); self.dynamic_panels.remove(&id); self.panels.close_tab(PanelKind::Dynamic(id)); } DetachedPanelAction::None => {} }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            if i.modifiers.ctrl && i.key_pressed(egui::Key::R) && !i.modifiers.shift { self.refresh_ports(); }
            if i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::O) { self.open_selected_port(); }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::B) { self.toggle_bottom_panel(); }
            if i.modifiers.ctrl && i.key_pressed(egui::Key::I) { self.panels.inspector_visible = !self.panels.inspector_visible; }
            if i.modifiers.ctrl {
                for (k, a) in [(egui::Key::Num1, Activity::Devices),(egui::Key::Num2, Activity::Plugins),(egui::Key::Num3, Activity::Settings)] { if i.key_pressed(k) { self.panels.select_activity(a); } }
            }
        });
    }
}

// ══════════════════════════════════════════
//  eframe::App
// ══════════════════════════════════════════

impl eframe::App for WorkbenchApp {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] { theme::BG_PRIMARY.to_normalized_gamma_f32() }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 终端放大按钮
        if self.terminal_panel.maximize_clicked { self.terminal_panel.maximize_clicked = false; self.terminal_popup_open = true; }
        // 回放清理
        if self.replay_panel.want_clear_on_play { self.replay_panel.want_clear_on_play = false; self.terminal_panel.clear(); }
        if self.replay_panel.want_step_backward { self.replay_panel.want_step_backward = false; self.terminal_panel.clear(); self.replay_panel.do_step_backward(); }
        if let Some(p) = self.replay_panel.want_seek_replay.take() { self.terminal_panel.clear(); self.replay_panel.do_seek_replay(p); }
        if self.replay_panel.want_pick_file { self.replay_panel.want_pick_file = false; if let Some(p) = windows_open_dialog() { self.replay_panel.path = p.display().to_string(); self.replay_panel.auto_load = true; } }
        self.dynamic_panels.ingest(&mut self.panels);
        let n = self.plugin_manager.process_pending(); if n > 0 { self.status_message = format!("{n} 个插件事件"); }
        self.handle_keys(&ctx);

        // 速率统计
        let now = ctx.input(|i| i.time);
        if self.last_rate_check_time > 0.0 { let el = now - self.last_rate_check_time; if el >= 1.0 { let c = self.bus.history().len(); self.event_rate = (c.saturating_sub(self.last_event_count)) as f64 / el; self.last_event_count = c; self.last_rate_check_time = now; } } else { self.last_rate_check_time = now; self.last_event_count = self.bus.history().len(); }
        if now - self.last_port_refresh > PORT_REFRESH_INTERVAL_SECS { self.last_port_refresh = now; if let Ok(p) = self.transport.list_serial_ports() { self.ports = p; } }

        // 面板
        egui::Panel::top("top-bar").show_inside(ui, |ui| self.top_bar(ui));
        egui::Panel::left("activity-bar").resizable(false).default_size(ACTIVITY_BAR_WIDTH).show_inside(ui, |ui| self.activity_bar(ui));

        egui::Panel::right("inspector").resizable(false).exact_size(if self.panels.inspector_visible { INSPECTOR_WIDTH } else { 0.0 }).show_separator_line(self.panels.inspector_visible).show_inside(ui, |ui| { if self.panels.inspector_visible { self.inspector(ui); } });

        if self.bottom_panel_visible {
            egui::Panel::bottom("bottom-bar").resizable(true).min_size(BOTTOM_PANEL_MIN).default_size(BOTTOM_PANEL_HEIGHT).show_separator_line(true).show_inside(ui, |ui| self.show_bottom_panel_contents(ui));
        } else {
            egui::Panel::bottom("status-only").resizable(false).show_separator_line(false).min_size(0.0).default_size(0.0).show_inside(ui, |ui| self.status_bar(ui));
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
        if let Some(s) = self.activity_drag_source && s < self.activity_order.len() && let Some(p) = ctx.pointer_latest_pos() {
            let act = self.activity_order[s]; let label = format!("{} {}", aicon(act), act.label());
            let gal = ctx.fonts_mut(|f| f.layout(label.clone(), egui::FontId::proportional(12.0), theme::TEXT_PRIMARY, f32::INFINITY));
            let rect = egui::Rect::from_min_size(p + egui::vec2(8.0, -12.0), egui::vec2(gal.size().x + 16.0, 26.0));
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dghost")));
            painter.rect_filled(rect, 5.0, egui::Color32::from_rgba_premultiplied(46, 80, 120, 210));
            painter.galley(rect.center() - gal.size() * 0.5, gal, egui::Color32::from_rgba_premultiplied(255, 255, 255, 240));
        }

        self.detached_dynamic_panel_viewports(&ctx);
        self.send_popup(&ctx);
        self.terminal_popup(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(REPAINT_INTERVAL_MS));
    }
}

// ── 发送放大窗口 ──
impl WorkbenchApp {
    fn terminal_popup(&mut self, ctx: &egui::Context) {
        if !self.terminal_popup_open { return; }
        let vid = egui::ViewportId::from_hash_of("term-popup");
        let builder = egui::ViewportBuilder::default().with_title("接收区 - 硬件调试工作台").with_inner_size([800.0, 600.0]);
        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) { return true; }
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let mut close = false;
                ui.horizontal(|ui| { ui.heading("接收区"); if ui.button("关闭").clicked() { close = true; } });
                ui.separator(); self.terminal_panel.ui(ui); close
            }).inner
        });
        if should_close { self.terminal_popup_open = false; }
    }

    fn send_popup(&mut self, ctx: &egui::Context) {
        if !self.send_popup_open { return; }
        let vid = egui::ViewportId::from_hash_of("send-popup");
        let builder = egui::ViewportBuilder::default().with_title("发送 - 硬件调试工作台").with_inner_size([640.0, 480.0]).with_min_inner_size([360.0, 260.0]);
        let should_close = ctx.show_viewport_immediate(vid, builder, |ui, _| {
            if ui.ctx().input(|i| i.viewport().close_requested()) { return true; }
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let so = self.transport.status().open;
                let ctrl_enter = ui.ctx().input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| { ui.radio_value(&mut self.send_hex_mode, false, "文本"); ui.radio_value(&mut self.send_hex_mode, true, "HEX"); ui.checkbox(&mut self.send_append_lf, "LF"); if ui.add_enabled(so, egui::Button::new("发送 (Ctrl+Enter)")).clicked() || (ctrl_enter && so && !self.send_input.is_empty()) { self.do_send(); } if ui.button("清空").clicked() { self.send_input.clear(); self.send_error = None; } });
                ui.separator(); ui.add(egui::TextEdit::multiline(&mut self.send_input).desired_width(f32::INFINITY).desired_rows(24).hint_text("Ctrl+Enter 发送"));
                if let Some(ref e) = self.send_error { ui.colored_label(theme::RED, translate_error(e)); }
                false
            }).inner
        });
        if should_close { self.send_popup_open = false; }
    }
}

impl Drop for WorkbenchApp { fn drop(&mut self) { self.save_config(); self.recorder.stop(); self.transport.close_serial(); } }

// ══════════════════════════════════════════
//  辅助函数
// ══════════════════════════════════════════

fn pdb(v: &str) -> DataBits { match v { "5" => DataBits::Five, "6" => DataBits::Six, "7" => DataBits::Seven, _ => DataBits::Eight } }
fn psb(v: &str) -> StopBits { match v { "2" => StopBits::Two, _ => StopBits::One } }
fn ppar(v: &str) -> Parity { match v { "odd" => Parity::Odd, "even" => Parity::Even, _ => Parity::None } }
fn serial_combo(ui: &mut egui::Ui, id: &'static str, w: f32, ports: &[SerialPortDescriptor], sel: &mut Option<String>) {
    egui::ComboBox::from_id_salt(id).width(w).selected_text(sel.clone().unwrap_or_else(|| "无端口".into())).show_ui(ui, |ui| { for p in ports { ui.selectable_value(sel, Some(p.port_name.clone()), &p.port_name); } });
}
fn baud_combo(ui: &mut egui::Ui, id: &'static str, w: f32, baud: &mut String) {
    let r = ["9600","19200","38400","57600","115200","230400","460800","921600"];
    egui::ComboBox::from_id_salt(id).width(w).selected_text(baud.clone()).show_ui(ui, |ui| { for x in r { ui.selectable_value(baud, x.to_owned(), x); } });
}
fn send_impl(input: &str, hex: bool, lf: bool, t: &TransportManager) -> Result<(), tool_transport::TransportError> {
    if input.trim().is_empty() { return Ok(()); }
    if hex { for line in input.lines() { let x = line.trim(); if x.is_empty() { continue; } t.send_hex(x)?; } Ok(()) }
    else { let mut text = input.to_owned(); if lf { text.push('\n'); } t.send_text(&text) }
}
fn translate_error(m: &str) -> String { if m.contains("no serial") { "串口未打开".into() } else if m.contains("invalid hex") { format!("无效HEX: {}", m.trim_start_matches("invalid hex input: ")) } else { m.to_owned() } }
fn load_config() -> Option<PersistedConfig> { let t = std::fs::read_to_string(config_path()).ok()?; serde_json::from_str(&t).ok() }
fn config_path() -> PathBuf { std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join("workspace.json") }
fn windows_open_dialog() -> Option<PathBuf> {
    let output = std::process::Command::new("powershell").args(["-Command", r#"Add-Type -AssemblyName System.Windows.Forms; $d=New-Object System.Windows.Forms.OpenFileDialog; $d.Filter='JSONL (*.jsonl)|*.jsonl'; if($d.ShowDialog() -eq 'OK'){Write-Output $d.FileName}"#]).output().ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() { None } else { Some(PathBuf::from(path)) }
}
fn default_recorder_path() -> String { format!("logs/session-{}.jsonl", now_timestamp_ms()) }
fn aicon(a: Activity) -> &'static str { match a { Activity::Devices => "📟", Activity::Replay => "⏪", Activity::Plugins => "🧩", Activity::Settings => "⚙", _ => "" } }
fn ashortcut(a: Activity) -> &'static str { match a { Activity::Devices => "Ctrl+1", Activity::Plugins => "Ctrl+2", Activity::Settings => "Ctrl+3", _ => "" } }
