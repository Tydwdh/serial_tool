mod form_render;
mod ingest;
mod schema;

use crate::{AttitudePanel, ChartPanel, theme};
use egui::RichText;
use form_render::dynamic_form_ui;
use schema::DynamicField;
use std::collections::{BTreeMap, VecDeque};
use tool_core::{LogLevel, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};
use tool_transport::SerialPortDescriptor;

pub struct DynamicPanels {
    bus: DataBus,
    subscription: Subscription,
    remove_subscription: Subscription,
    // UI 状态更新订阅
    set_value_subscription: Subscription,
    set_enabled_subscription: Subscription,
    set_visible_subscription: Subscription,
    file_browse_subscription: Subscription,
    file_selected_subscription: Subscription,
    log_append_subscription: Subscription,
    panels: BTreeMap<String, DynamicPanel>,
    last_error: Option<String>,
    ports: Vec<SerialPortDescriptor>,
}

struct LogEntry {
    timestamp_ms: u64,
    level: LogLevel,
    message: String,
}

enum DynamicPanel {
    Chart {
        title: String,
        chart: ChartPanel,
        owner_plugin_id: Option<String>,
    },
    Form {
        title: String,
        fields: Vec<DynamicField>,
        auto_apply: bool,
        owner_plugin_id: Option<String>,
    },
    Attitude {
        title: String,
        attitude: AttitudePanel,
        owner_plugin_id: Option<String>,
    },
    Log {
        title: String,
        entries: VecDeque<LogEntry>,
        max_entries: usize,
        owner_plugin_id: Option<String>,
    },
}

impl DynamicPanel {
    fn owner_plugin_id(&self) -> Option<&str> {
        match self {
            DynamicPanel::Chart {
                owner_plugin_id, ..
            }
            | DynamicPanel::Form {
                owner_plugin_id, ..
            }
            | DynamicPanel::Attitude {
                owner_plugin_id, ..
            }
            | DynamicPanel::Log {
                owner_plugin_id, ..
            } => owner_plugin_id.as_deref(),
        }
    }

    fn title(&self) -> &str {
        match self {
            DynamicPanel::Chart { title, .. }
            | DynamicPanel::Form { title, .. }
            | DynamicPanel::Attitude { title, .. }
            | DynamicPanel::Log { title, .. } => title.as_str(),
        }
    }
}

impl DynamicPanels {
    pub fn new(bus: &DataBus) -> Self {
        // UI 面板使用有界订阅（容量 1024），与 LogPanel/TerminalPanel 保持一致
        // 防止 UI 来不及消费时内存无限增长
        const UI_SUB_CAP: usize = 1024;
        Self {
            bus: bus.clone(),
            subscription: bus
                .subscribe_lossy_bounded(TopicFilter::exact(topics::UI_PANEL_CREATE), UI_SUB_CAP),
            remove_subscription: bus
                .subscribe_lossy_bounded(TopicFilter::exact(topics::UI_PANEL_REMOVE), UI_SUB_CAP),
            set_value_subscription: bus
                .subscribe_lossy_bounded(TopicFilter::exact(topics::UI_FORM_SET_VALUE), UI_SUB_CAP),
            set_enabled_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_FORM_SET_ENABLED),
                UI_SUB_CAP,
            ),
            set_visible_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_FORM_SET_VISIBLE),
                UI_SUB_CAP,
            ),
            file_browse_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_FORM_FILE_BROWSE),
                UI_SUB_CAP,
            ),
            file_selected_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_FORM_FILE_SELECTED),
                UI_SUB_CAP,
            ),
            log_append_subscription: bus
                .subscribe_lossy_bounded(TopicFilter::exact(topics::UI_LOG_APPEND), UI_SUB_CAP),
            panels: BTreeMap::new(),
            last_error: None,
            ports: Vec::new(),
        }
    }

    pub fn ui_body(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(panel) = self.panels.get_mut(id) else {
            ui.colored_label(theme::RED, "面板未找到");
            return;
        };

        match panel {
            DynamicPanel::Chart { chart, .. } => {
                chart.ui(ui);
            }
            DynamicPanel::Form {
                fields, auto_apply, ..
            } => {
                dynamic_form_ui(ui, &self.bus, id, fields, *auto_apply, &self.ports);
            }
            DynamicPanel::Attitude { attitude, .. } => {
                attitude.ui(ui);
            }
            DynamicPanel::Log { title, entries, .. } => {
                ui.label(RichText::new(title.as_str()).strong());
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for entry in entries.iter() {
                            let color = crate::level_color(entry.level);
                            // 使用 fmt_ts 保持与内置 LogPanel 一致的时间戳精度（含毫秒）
                            let ts = crate::fmt_ts(entry.timestamp_ms);
                            let text = format!("[{ts}] {} {}", entry.level.as_str(), entry.message);
                            ui.colored_label(color, RichText::new(text));
                        }
                    });
            }
        }
    }

    pub fn title(&self, id: &str) -> Option<&str> {
        self.panels.get(id).map(|panel| panel.title())
    }

    pub fn count(&self) -> usize {
        self.panels.len()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.panels.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.panels.remove(id).is_some()
    }

    pub fn remove_by_plugin(&mut self, plugin_id: &str) -> Vec<String> {
        let ids: Vec<String> = self
            .panels
            .iter()
            .filter(|(_, panel)| panel.owner_plugin_id() == Some(plugin_id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.panels.remove(id);
        }
        ids
    }

    pub fn panel_owner(&self, panel_id: &str) -> Option<&str> {
        self.panels.get(panel_id).and_then(|p| p.owner_plugin_id())
    }

    pub fn clear_charts(&mut self) {
        for panel in self.panels.values_mut() {
            if let DynamicPanel::Chart { chart, .. } = panel {
                chart.clear();
            }
        }
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;

        for panel in self.panels.values_mut() {
            if let DynamicPanel::Chart { chart, .. } = panel {
                count += chart.ingest_all_pending();
            }
        }

        count
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn set_ports(&mut self, ports: &[SerialPortDescriptor]) {
        self.ports = ports.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelKind, PanelManager};
    use tool_core::{Direction, Event, Payload};

    #[test]
    fn creates_dynamic_chart_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "pid-chart",
                "title": "PID Chart",
                "kind": "chart",
                "topic_prefix": "protocol.pid."
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("pid-chart"), Some("PID Chart"));
        assert!(
            manager
                .tabs()
                .contains(&PanelKind::Dynamic("pid-chart".to_owned()))
        );
    }

    #[test]
    fn replay_panel_create_preserves_plugin_owner() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(
            Event::new(
                topics::UI_PANEL_CREATE,
                "replay:plugin:demo",
                Direction::Internal,
                Payload::Json(serde_json::json!({
                    "id": "demo-chart",
                    "title": "Demo Chart",
                    "kind": "chart",
                    "topic_prefix": "protocol.demo."
                })),
            )
            .with_metadata(serde_json::json!({
                "replay": true,
                "origin": "replay",
                "original_source": "plugin:demo"
            })),
        );

        panels.ingest(&mut manager);

        assert_eq!(panels.panel_owner("demo-chart"), Some("demo"));
    }

    #[test]
    fn creates_dynamic_form_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "pid-form",
                "title": "PID Form",
                "kind": "form",
                "fields": [
                    { "id": "kp", "label": "Kp", "kind": "number", "default": 1.0 },
                    {
                        "id": "mode",
                        "label": "模式",
                        "kind": "select",
                        "default": "auto",
                        "options": [
                            { "label": "自动", "value": "auto" },
                            { "label": "手动", "value": "manual" }
                        ]
                    }
                ]
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("pid-form"), Some("PID Form"));
        assert!(
            manager
                .tabs()
                .contains(&PanelKind::Dynamic("pid-form".to_owned()))
        );
    }

    #[test]
    fn creates_dynamic_attitude_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "imu-attitude",
                "title": "IMU Attitude",
                "kind": "attitude",
                "topic": "protocol.imu.attitude"
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("imu-attitude"), Some("IMU Attitude"));
        assert!(
            manager
                .tabs()
                .contains(&PanelKind::Dynamic("imu-attitude".to_owned()))
        );
    }

    #[test]
    fn removes_dynamic_panel_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "pid-chart",
                "title": "PID Chart",
                "kind": "chart"
            })),
        ));

        panels.ingest(&mut manager);

        bus.publish(Event::new(
            topics::UI_PANEL_REMOVE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({ "id": "pid-chart" })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 0);
        assert!(
            !manager
                .tabs()
                .contains(&PanelKind::Dynamic("pid-chart".to_owned()))
        );
    }

    #[test]
    fn creates_form_with_label_and_separator_without_id() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "test",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "file-tool-panel",
                "title": "文件工具",
                "kind": "form",
                "fields": [
                    { "kind": "label", "text": "请选择文件" },
                    { "kind": "separator" },
                    { "id": "file_path", "label": "文件", "kind": "file" },
                    { "id": "load", "label": "加载", "kind": "button" }
                ]
            })),
        ));

        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("file-tool-panel"), Some("文件工具"));
    }

    #[test]
    fn rejects_field_update_from_different_owner() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        // 插件 A 创建面板
        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "a.form",
                "title": "A Form",
                "kind": "form",
                "fields": [
                    { "id": "val", "label": "Val", "kind": "number", "default": 0.0 }
                ],
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        // 插件 B 尝试修改插件 A 的面板字段 — 应被拒绝
        bus.publish(Event::new(
            topics::UI_FORM_SET_VALUE,
            "plugin:b",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id": "a.form",
                "field_id": "val",
                "value": 999.0,
                "plugin_id": "b"
            })),
        ));

        panels.ingest(&mut manager);

        // 值不应被修改（仍是默认值 0.0）
        if let Some(DynamicPanel::Form { fields, .. }) = panels.panels.get("a.form")
            && let Some(field) = fields.iter().find(|f| f.id == "val")
        {
            assert_eq!(field.value.as_f64(), Some(0.0));
        }
    }

    #[test]
    fn allows_field_update_from_same_owner() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "a.form",
                "title": "A Form",
                "kind": "form",
                "fields": [
                    { "id": "val", "label": "Val", "kind": "number", "default": 0.0 }
                ],
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        // 插件 A 修改自己的面板 — 应被允许
        bus.publish(Event::new(
            topics::UI_FORM_SET_VALUE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id": "a.form",
                "field_id": "val",
                "value": 42.0,
                "plugin_id": "a"
            })),
        ));

        panels.ingest(&mut manager);

        if let Some(DynamicPanel::Form { fields, .. }) = panels.panels.get("a.form")
            && let Some(field) = fields.iter().find(|f| f.id == "val")
        {
            assert_eq!(field.value.as_f64(), Some(42.0));
        }
    }

    #[test]
    fn system_panel_rejects_plugin_modification() {
        // 系统面板（无 owner）不能被任何插件修改
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "ui.replay",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "sys.chart",
                "title": "System Chart",
                "kind": "chart",
                "topic_prefix": "protocol."
            })),
        ));
        panels.ingest(&mut manager);

        // 插件尝试修改系统面板 — 应被拒绝
        bus.publish(Event::new(
            topics::UI_FORM_SET_VALUE,
            "plugin:evil",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id": "sys.chart",
                "field_id": "nonexistent",
                "value": "hacked",
                "plugin_id": "evil"
            })),
        ));

        panels.ingest(&mut manager);

        // 系统面板仍存在
        assert!(panels.panels.contains_key("sys.chart"));
    }
}
