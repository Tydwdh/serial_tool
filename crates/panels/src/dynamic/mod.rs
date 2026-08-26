mod form_render;
mod ingest;
mod schema;

pub use form_render::dynamic_form_ui;
pub use schema::{DynamicField, DynamicFieldKind, FieldFilter, FieldOption, parse_fields};

use crate::{AttitudePanel, ChartPanel, DataTablePanel, GaugePanel, theme};

#[derive(Debug, Clone)]
pub struct PortItem {
    pub port_name: String,
}
use std::collections::BTreeMap;
use tool_core::topics;
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct DynamicPanels {
    bus: DataBus,
    subscription: Subscription,
    remove_subscription: Subscription,
    // UI 状态更新订阅
    set_value_subscription: Subscription,
    set_values_subscription: Subscription,
    set_enabled_subscription: Subscription,
    set_visible_subscription: Subscription,
    file_browse_subscription: Subscription,
    file_selected_subscription: Subscription,
    table_set_rows_subscription: Subscription,
    table_append_rows_subscription: Subscription,
    table_remove_rows_subscription: Subscription,
    table_clear_subscription: Subscription,
    panels: BTreeMap<String, DynamicPanel>,
    last_error: Option<String>,
    ports: Vec<PortItem>,
}

enum DynamicPanel {
    Chart {
        title: String,
        chart: ChartPanel,
        owner_plugin_id: Option<String>,
        card: bool,
    },
    Form {
        title: String,
        fields: Vec<DynamicField>,
        auto_apply: bool,
        owner_plugin_id: Option<String>,
        card: bool,
    },
    Attitude {
        title: String,
        attitude: AttitudePanel,
        owner_plugin_id: Option<String>,
        card: bool,
    },
    Gauge {
        title: String,
        gauge: GaugePanel,
        owner_plugin_id: Option<String>,
        card: bool,
    },
    Table {
        title: String,
        table: DataTablePanel,
        owner_plugin_id: Option<String>,
        card: bool,
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
            | DynamicPanel::Gauge {
                owner_plugin_id, ..
            }
            | DynamicPanel::Table {
                owner_plugin_id, ..
            } => owner_plugin_id.as_deref(),
        }
    }

    fn title(&self) -> &str {
        match self {
            DynamicPanel::Chart { title, .. }
            | DynamicPanel::Form { title, .. }
            | DynamicPanel::Attitude { title, .. }
            | DynamicPanel::Gauge { title, .. }
            | DynamicPanel::Table { title, .. } => title.as_str(),
        }
    }

    fn card(&self) -> bool {
        match self {
            DynamicPanel::Chart { card, .. }
            | DynamicPanel::Form { card, .. }
            | DynamicPanel::Attitude { card, .. }
            | DynamicPanel::Gauge { card, .. }
            | DynamicPanel::Table { card, .. } => *card,
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
            set_values_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_PANEL_SET_VALUES),
                UI_SUB_CAP,
            ),
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
            table_set_rows_subscription: bus
                .subscribe_lossy_bounded(TopicFilter::exact(topics::UI_TABLE_SET_ROWS), UI_SUB_CAP),
            table_append_rows_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_TABLE_APPEND_ROWS),
                UI_SUB_CAP,
            ),
            table_remove_rows_subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(topics::UI_TABLE_REMOVE_ROWS),
                UI_SUB_CAP,
            ),
            table_clear_subscription: bus
                .subscribe_lossy_bounded(TopicFilter::exact(topics::UI_TABLE_CLEAR), UI_SUB_CAP),
            panels: BTreeMap::new(),
            last_error: None,
            ports: Vec::new(),
        }
    }

    pub fn ui_body(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(panel) = self.panels.get_mut(id) else {
            ui.colored_label(theme::red(), "面板未找到");
            return;
        };

        let card = panel.card();
        let title = panel.title().to_owned();

        if card {
            // 无边框嵌入：标题 + 内容直接贴在面板背景上，不套 Frame::group
            ui.set_min_width(ui.available_width());
            ui.label(egui::RichText::new(&title).strong());
            ui.separator();
            render_panel_inner(panel, ui, id, &self.bus, &self.ports);
        } else {
            render_panel_inner(panel, ui, id, &self.bus, &self.ports);
        }
    }

    pub fn title(&self, id: &str) -> Option<&str> {
        self.panels.get(id).map(|panel| panel.title())
    }

    /// 当前所有动态面板 id（供注册表同步面板定义）。
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.panels.keys().map(String::as_str)
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
            match panel {
                DynamicPanel::Chart { chart, .. } => chart.clear(),
                DynamicPanel::Attitude { attitude, .. } => attitude.clear(),
                DynamicPanel::Gauge { gauge, .. } => gauge.clear(),
                _ => {}
            }
        }
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;

        for panel in self.panels.values_mut() {
            match panel {
                DynamicPanel::Chart { chart, .. } => count += chart.ingest_all_pending(),
                DynamicPanel::Attitude { attitude, .. } => count += attitude.ingest_all_pending(),
                DynamicPanel::Gauge { gauge, .. } => count += gauge.ingest_all_pending(),
                _ => {}
            }
        }

        count
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn set_ports(&mut self, ports: &[PortItem]) {
        self.ports = ports.to_vec();
    }

    /// Drain browser/native UI file-browse requests for the composition root.
    ///
    /// Native uses `Workbench`'s shared UI event subscription; Web needs the
    /// same request to open an `<input type=file>` without introducing a
    /// second dynamic-panel implementation.
    pub fn drain_file_browse_requests(&mut self) -> Vec<tool_core::Event> {
        self.file_browse_subscription.drain_limited(500)
    }
}

/// 渲染动态面板的实际内容（不含外层卡片包装）
fn render_panel_inner(
    panel: &mut DynamicPanel,
    ui: &mut egui::Ui,
    id: &str,
    bus: &DataBus,
    ports: &[PortItem],
) {
    match panel {
        DynamicPanel::Chart { chart, .. } => {
            chart.ui(ui);
        }
        DynamicPanel::Form {
            fields, auto_apply, ..
        } => {
            dynamic_form_ui(ui, bus, id, fields, *auto_apply, ports);
        }
        DynamicPanel::Attitude { attitude, .. } => {
            attitude.ui(ui);
        }
        DynamicPanel::Gauge { gauge, .. } => {
            gauge.ui(ui);
        }
        DynamicPanel::Table { table, .. } => {
            table.ui(ui, id, bus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelId, PanelManager};
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
        assert!(manager.tabs().contains(&PanelId::dynamic("pid-chart")));
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
        assert!(manager.tabs().contains(&PanelId::dynamic("pid-form")));
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
        assert!(manager.tabs().contains(&PanelId::dynamic("imu-attitude")));
    }

    #[test]
    fn creates_dynamic_gauge_from_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "a.gauge",
                "title": "温度",
                "kind": "gauge",
                "topic": "widget.showcase.temperature",
                "min": 0,
                "max": 100,
                "unit": "°C",
                "label": "传感器温度",
                "zones": [
                    {"from": 0, "to": 60, "color": "green"},
                    {"from": 60, "to": 80, "color": "yellow"},
                    {"from": 80, "to": 100, "color": "red"}
                ],
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        assert_eq!(panels.count(), 1);
        assert_eq!(panels.title("a.gauge"), Some("温度"));
    }

    #[test]
    fn gauge_set_value_and_status_via_bus_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "a.gauge",
                "title": "温度",
                "kind": "gauge",
                "topic": "widget.showcase.temperature",
                "min": 0,
                "max": 100,
                "zones": [
                    {"from": 0, "to": 60, "color": "green"},
                    {"from": 80, "to": 100, "color": "red"}
                ],
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        // 通过 topic 推送数值
        bus.publish(Event::new(
            "widget.showcase.temperature",
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({"value": 90.0})),
        ));
        panels.ingest_all_pending();

        // set_value 更新 value
        bus.publish(Event::new(
            topics::UI_FORM_SET_VALUE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id": "a.gauge",
                "field_id": "value",
                "value": 90.0,
                "plugin_id": "a"
            })),
        ));
        // set_value 更新 status
        bus.publish(Event::new(
            topics::UI_FORM_SET_VALUE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id": "a.gauge",
                "field_id": "status",
                "value": "过热",
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        if let Some(DynamicPanel::Gauge { gauge, .. }) = panels.panels.get("a.gauge") {
            assert!((gauge.value() - 90.0).abs() < f64::EPSILON);
            assert_eq!(gauge.status_text(), "过热");
        } else {
            panic!("gauge panel not found");
        }
    }

    #[test]
    fn creates_dynamic_chart_with_exact_topic() {
        // chart 用 topic（精确）订阅，不应收到前缀下其他 topic 的数据
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();

        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id": "a.chart",
                "title": "图表",
                "kind": "chart",
                "topic": "widget.showcase.chart.sample",
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        // 精确 topic 的数据应被收入
        bus.publish(Event::new(
            "widget.showcase.chart.sample",
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({"t": 1.0, "sine": 0.5})),
        ));
        // 同前缀但不同 topic 的数据不应被收入
        bus.publish(Event::new(
            "widget.showcase.temperature",
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({"value": 99.0})),
        ));
        panels.ingest_all_pending();

        if let Some(DynamicPanel::Chart { chart, .. }) = panels.panels.get("a.chart") {
            let series_names: Vec<&String> = chart.series_keys();
            assert!(series_names.iter().any(|n| *n == "sine"));
            assert!(!series_names.iter().any(|n| *n == "value"));
        } else {
            panic!("chart panel not found");
        }
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
        assert!(!manager.tabs().contains(&PanelId::dynamic("pid-chart")));
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
    fn label_field_accepts_set_value() {
        // Label 字段应能通过 set_value 更新运行时文本（用于动态计数/状态展示）
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
                    { "id": "samples", "label": "已生成样本", "kind": "label" }
                ],
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        // 初始 Label 字段 value 为空字符串
        if let Some(DynamicPanel::Form { fields, .. }) = panels.panels.get("a.form")
            && let Some(field) = fields.iter().find(|f| f.id == "samples")
        {
            assert_eq!(field.value.as_str(), Some(""));
        }

        // 插件 A 更新 samples 文本 — 应被允许并写入 field.value
        bus.publish(Event::new(
            topics::UI_FORM_SET_VALUE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id": "a.form",
                "field_id": "samples",
                "value": "42",
                "plugin_id": "a"
            })),
        ));
        panels.ingest(&mut manager);

        if let Some(DynamicPanel::Form { fields, .. }) = panels.panels.get("a.form")
            && let Some(field) = fields.iter().find(|f| f.id == "samples")
        {
            assert_eq!(field.value.as_str(), Some("42"));
        } else {
            panic!("samples field not found after update");
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

    #[test]
    fn batch_values_update_multiple_fields_with_one_event() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();
        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id":"a.form","title":"Form","kind":"form",
                "fields":[
                    {"id":"progress","label":"Progress","kind":"number","default":0},
                    {"id":"status","label":"Status","kind":"text","default":"idle"}
                ]
            })),
        ));
        panels.ingest(&mut manager);
        bus.publish(Event::new(
            topics::UI_PANEL_SET_VALUES,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id":"a.form","values":{"progress":100,"status":"done"}
            })),
        ));
        panels.ingest(&mut manager);
        let DynamicPanel::Form { fields, .. } = panels.panels.get("a.form").unwrap() else {
            panic!()
        };
        assert_eq!(
            fields.iter().find(|f| f.id == "progress").unwrap().value,
            100
        );
        assert_eq!(
            fields.iter().find(|f| f.id == "status").unwrap().value,
            "done"
        );
    }

    #[test]
    fn table_rows_are_owned_and_mutable_by_plugin() {
        let bus = DataBus::new();
        let mut panels = DynamicPanels::new(&bus);
        let mut manager = PanelManager::default();
        bus.publish(Event::new(
            topics::UI_PANEL_CREATE,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "id":"a.table","title":"Rows","kind":"table",
                "columns":[{"id":"value","title":"Value"}]
            })),
        ));
        panels.ingest(&mut manager);
        bus.publish(Event::new(
            topics::UI_TABLE_APPEND_ROWS,
            "plugin:a",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "panel_id":"a.table","rows":[{"id":"1","value":42}]
            })),
        ));
        bus.publish(Event::new(
            topics::UI_TABLE_CLEAR,
            "plugin:b",
            Direction::Internal,
            Payload::Json(serde_json::json!({"panel_id":"a.table"})),
        ));
        panels.ingest(&mut manager);
        let DynamicPanel::Table { table, .. } = panels.panels.get("a.table").unwrap() else {
            panic!()
        };
        assert_eq!(table.rows().len(), 1);
    }
}
