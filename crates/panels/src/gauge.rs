use crate::theme;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::Value;
use tool_core::{Event, Payload};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct GaugePanel {
    subscription: Subscription,
    value: f64,
    min: f64,
    max: f64,
    unit: String,
    zones: Vec<GaugeZone>,
    label: String,
    /// 运行时状态文本（通过 set_value(panel, "status", text) 更新），
    /// 为空则根据当前值所在色区自动生成（正常/预警/异常）。
    status: String,
    samples: usize,
}

pub(crate) struct GaugeZone {
    from: f64,
    to: f64,
    color: Color32,
    kind: ZoneKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum ZoneKind {
    Safe,
    Warn,
    Danger,
    None,
}

impl GaugePanel {
    pub fn new(bus: &DataBus, topic: impl Into<String>) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(TopicFilter::exact(topic.into()), 1024),
            value: 0.0,
            min: 0.0,
            max: 100.0,
            unit: String::new(),
            zones: Vec::new(),
            label: String::new(),
            status: String::new(),
            samples: 0,
        }
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn from_config(
        bus: &DataBus,
        topic: impl Into<String>,
        min: f64,
        max: f64,
        unit: String,
        zones: Vec<GaugeZone>,
        label: String,
    ) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(TopicFilter::exact(topic.into()), 1024),
            value: min,
            min,
            max,
            unit,
            zones,
            label,
            status: String::new(),
            samples: 0,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.ingest();

        let desired = Vec2::new(ui.available_width(), 200.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        self.paint(ui, rect);
    }

    fn ingest(&mut self) {
        for event in self.subscription.drain_limited(500) {
            self.push_event(event);
        }
    }

    fn push_event(&mut self, event: Event) {
        match event.payload {
            Payload::Json(value) => {
                if let Some(v) = gauge_value_from_json(&value) {
                    self.value = v;
                    self.samples += 1;
                }
            }
            Payload::Text(text) => {
                if let Some(v) = gauge_value_from_text(&text) {
                    self.value = v;
                    self.samples += 1;
                }
            }
            Payload::Bytes(_) | Payload::Empty => {}
        }
    }

    pub fn set_value(&mut self, value: f64) {
        self.value = value;
    }

    /// 当前值（供测试与诊断）。
    pub fn value(&self) -> f64 {
        self.value
    }

    /// 设置运行时状态文本（空字符串则回退到按色区自动生成）。
    pub fn set_status(&mut self, status: String) {
        self.status = status;
    }

    /// 返回当前应显示的状态文本：优先运行时设置，否则按值所在色区自动生成。
    pub fn status_text(&self) -> String {
        if !self.status.is_empty() {
            return self.status.clone();
        }
        match self.zone_kind_for_value() {
            ZoneKind::Safe => "正常".to_owned(),
            ZoneKind::Warn => "预警".to_owned(),
            ZoneKind::Danger => "异常".to_owned(),
            ZoneKind::None => "—".to_owned(),
        }
    }

    fn status_color(&self) -> Color32 {
        match self.zone_kind_for_value() {
            ZoneKind::Safe => theme::green(),
            ZoneKind::Warn => theme::yellow(),
            ZoneKind::Danger => theme::red(),
            ZoneKind::None => theme::text_secondary(),
        }
    }

    fn zone_kind_for_value(&self) -> ZoneKind {
        for zone in &self.zones {
            if self.value >= zone.from && self.value <= zone.to {
                return zone.kind;
            }
        }
        // 越界：低于所有 zone 取首个，高于取末个，与 value_color 保持一致
        if let (Some(first), Some(last)) = (self.zones.first(), self.zones.last()) {
            if self.value < first.from {
                return first.kind;
            }
            if self.value > last.to {
                return last.kind;
            }
        }
        ZoneKind::None
    }

    pub fn clear(&mut self) {
        while self.subscription.try_recv().is_some() {}
        self.value = self.min;
        self.samples = 0;
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;
        while let Some(event) = self.subscription.try_recv() {
            self.push_event(event);
            count += 1;
        }
        count
    }

    fn paint(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        // 无边框嵌入：不画背景填充与外边框，直接在面板背景上绘制弧线与指针

        // 几何布局：圆心在顶部留白下方一个半径处。270° 弧顶部在 center.y - radius，
        // 底部弧端点在 center.y + 0.707*radius，数值/状态/标签文本落在 center.y + 22..+74。
        // 四周均预留描边半宽，避免弧线被 rect 裁切。
        let radius = gauge_radius(rect);
        let center = Pos2::new(rect.center().x, rect.top() + radius + STROKE_HALF);

        // 弧形参数：270° 弧，底部留 90° 缺口
        // egui 屏幕坐标 y 向下，角度递增 = 顺时针：0°=右，90°=下，180°=左，270°=上
        // min 在左下（135°），顺时针经左/上/右到 max 右下（45°=405°），缺口落在底部 45°..135°
        let start_angle_deg = 135.0_f32; // 左下角（min）
        let sweep_deg = 270.0_f32; // 顺时针扫 270°
        let arc_steps = 60; // 弧的线段数

        // 先画整圈灰色底轨道，确保色区未覆盖的区段（如 zones 未延伸到 max）也有底色
        draw_arc(
            &painter,
            center,
            radius,
            start_angle_deg,
            sweep_deg,
            0.0,
            1.0,
            arc_steps,
            Stroke::new(6.0, theme::gauge_arc()),
        );

        // 画色区弧（叠在底轨道上，覆盖各自区间）
        for zone in &self.zones {
            let from_t = value_to_fraction(zone.from, self.min, self.max) as f32;
            let to_t = value_to_fraction(zone.to, self.min, self.max) as f32;
            draw_arc(
                &painter,
                center,
                radius,
                start_angle_deg,
                sweep_deg,
                from_t,
                to_t,
                arc_steps,
                Stroke::new(6.0, zone.color),
            );
        }

        // 画当前值对应的彩色弧段（叠在色区轨道上，更粗更亮）
        let value_t = value_to_fraction(self.value, self.min, self.max).clamp(0.0, 1.0) as f32;
        let value_color = self.value_color();
        draw_arc(
            &painter,
            center,
            radius,
            start_angle_deg,
            sweep_deg,
            0.0,
            value_t,
            arc_steps,
            Stroke::new(10.0, value_color),
        );

        // 画指针（角度与弧一致：start + sweep*value_t，顺时针）
        let value_angle = (start_angle_deg + sweep_deg * value_t).to_radians();
        let needle_len = radius - 14.0;
        let needle_end = Pos2::new(
            center.x + needle_len * value_angle.cos(),
            center.y + needle_len * value_angle.sin(),
        );
        painter.line_segment(
            [center, needle_end],
            Stroke::new(2.0, theme::text_primary()),
        );
        painter.circle_filled(center, 4.0, theme::text_primary());

        // 中心数值
        let value_text = if self.unit.is_empty() {
            format!("{:.1}", self.value)
        } else {
            format!("{:.1} {}", self.value, self.unit)
        };
        painter.text(
            Pos2::new(center.x, center.y + 22.0),
            egui::Align2::CENTER_TOP,
            value_text,
            egui::FontId::proportional(18.0),
            theme::text_primary(),
        );

        // 状态提示（数值下方，颜色跟随色区）
        let status_text = self.status_text();
        painter.text(
            Pos2::new(center.x, center.y + 44.0),
            egui::Align2::CENTER_TOP,
            &status_text,
            egui::FontId::proportional(12.0),
            self.status_color(),
        );

        // 标签
        if !self.label.is_empty() {
            painter.text(
                Pos2::new(center.x, center.y + 62.0),
                egui::Align2::CENTER_TOP,
                &self.label,
                egui::FontId::proportional(11.0),
                theme::text_secondary(),
            );
        }

        // min/max 标签：min 在起始角（左下），max 在终止角（右下）
        let min_angle = start_angle_deg.to_radians();
        let max_angle = (start_angle_deg + sweep_deg).to_radians();
        let min_pos = Pos2::new(
            center.x + (radius + 14.0) * min_angle.cos(),
            center.y + (radius + 14.0) * min_angle.sin(),
        );
        let max_pos = Pos2::new(
            center.x + (radius + 14.0) * max_angle.cos(),
            center.y + (radius + 14.0) * max_angle.sin(),
        );
        let tick_font = egui::FontId::proportional(10.0);
        painter.text(
            min_pos,
            egui::Align2::CENTER_CENTER,
            format!("{:.0}", self.min),
            tick_font.clone(),
            theme::text_dimmed(),
        );
        painter.text(
            max_pos,
            egui::Align2::CENTER_CENTER,
            format!("{:.0}", self.max),
            tick_font,
            theme::text_dimmed(),
        );
    }

    /// 根据当前值所在色区返回颜色：
    /// - 值在某个 zone 内 → 该 zone 颜色
    /// - 值低于所有 zone → 第一个 zone 的颜色
    /// - 值高于所有 zone → 最后一个 zone 的颜色
    /// - 无 zone → 默认值颜色
    fn value_color(&self) -> Color32 {
        for zone in &self.zones {
            if self.value >= zone.from && self.value <= zone.to {
                return zone.color;
            }
        }
        // 不在任何 zone 内：按越界方向取端点 zone 颜色，避免超限时突变为无关色
        if let (Some(first), Some(last)) = (self.zones.first(), self.zones.last()) {
            if self.value < first.from {
                return first.color;
            }
            if self.value > last.to {
                return last.color;
            }
        }
        theme::gauge_value()
    }
}

/// 仪表盘最粗弧线的半宽（值弧 10px），用于四周留白避免裁切。
const STROKE_HALF: f32 = 5.0;

/// 计算仪表盘半径，使其完整落在 rect 内：
/// - 宽度方向：2*radius + 两侧描边半宽 + min/max 标签余量
/// - 高度方向：弧顶到圆心（radius）+ 圆心到底部文本末尾（约 0.707*radius 与 74 取大）+ 上下描边半宽
fn gauge_radius(rect: Rect) -> f32 {
    const LABEL_MARGIN: f32 = 18.0;
    const TEXT_BOTTOM: f32 = 74.0; // 圆心到最底文本（label）底沿的距离
    const MIN_RADIUS: f32 = 40.0;

    // 宽度：center.x ± r，再加描边半宽和标签余量
    let by_width = (rect.width() * 0.5 - LABEL_MARGIN - STROKE_HALF).max(MIN_RADIUS);
    // 高度：top_clearance(r+stroke) + max(0.707r, text) + bottom_stroke <= height
    //   => r + STROKE_HALF + max(0.707r, TEXT_BOTTOM) + STROKE_HALF <= height
    //   0.707r 在 r>=105 时才超过 74，通常 text 占主导；用两者都满足的保守上界。
    let by_height = ((rect.height() - 2.0 * STROKE_HALF - TEXT_BOTTOM) / 1.0)
        .min((rect.height() - 2.0 * STROKE_HALF) / 1.707)
        .max(MIN_RADIUS);
    by_width.min(by_height)
}

/// `start_deg` 是弧起始角度，`sweep_deg` 是顺时针扫过的角度。
/// `from_t` 和 `to_t` 是 [0, 1] 的归一化位置（0 = 起始角，1 = 终止角）。
#[allow(clippy::too_many_arguments)]
fn draw_arc(
    painter: &egui::Painter,
    center: Pos2,
    radius: f32,
    start_deg: f32,
    sweep_deg: f32,
    from_t: f32,
    to_t: f32,
    steps: usize,
    stroke: Stroke,
) {
    let from_deg = start_deg + sweep_deg * from_t;
    let to_deg = start_deg + sweep_deg * to_t;
    let span = to_deg - from_deg;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = (from_deg + span * t).to_radians();
        points.push(Pos2::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }
    if points.len() >= 2 {
        painter.line(points, stroke);
    }
}

/// 将值归一化到 [0, 1]
fn value_to_fraction(value: f64, min: f64, max: f64) -> f64 {
    if (max - min).abs() < f64::EPSILON {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

/// 从 JSON 提取数值：优先 "value" 字段，否则取第一个数值字段
fn gauge_value_from_json(value: &Value) -> Option<f64> {
    let obj = value.as_object()?;
    // 优先 "value" 字段
    if let Some(v) = obj.get("value").and_then(Value::as_f64) {
        return Some(v);
    }
    // 取第一个数值字段（排除时间戳）
    for (key, val) in obj {
        if matches!(key.as_str(), "t" | "time" | "timestamp") {
            continue;
        }
        if let Some(v) = val.as_f64() {
            return Some(v);
        }
    }
    None
}

/// 从文本解析：`value=42.5` 或 `temperature=42.5`
fn gauge_value_from_text(text: &str) -> Option<f64> {
    for part in text.split(',') {
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        if key.trim() == "value" {
            return val.trim().parse().ok();
        }
    }
    // 取第一个数值
    for part in text.split(',') {
        let Some((_key, val)) = part.split_once('=') else {
            continue;
        };
        if let Ok(v) = val.trim().parse::<f64>() {
            return Some(v);
        }
    }
    None
}

/// 从 Lua 配置的 zones 数组解析色区
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn parse_zones(zones_value: Option<&Value>) -> Vec<GaugeZone> {
    let Some(arr) = zones_value.and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    for zone_val in arr {
        let obj = match zone_val.as_object() {
            Some(o) => o,
            None => continue,
        };
        let from = obj.get("from").and_then(Value::as_f64).unwrap_or(0.0);
        let to = obj.get("to").and_then(Value::as_f64).unwrap_or(100.0);
        let color_str = obj.get("color").and_then(Value::as_str).unwrap_or("green");
        let color = parse_zone_color(color_str);
        result.push(GaugeZone {
            from,
            to,
            color,
            kind: parse_zone_kind(color_str),
        });
    }
    result
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn parse_zone_color(s: &str) -> Color32 {
    match s.to_lowercase().as_str() {
        "green" => theme::green(),
        "yellow" | "warn" | "warning" => theme::yellow(),
        "red" | "error" | "danger" => theme::red(),
        "blue" | "info" => theme::blue(),
        "cyan" => theme::cyan(),
        "purple" | "magenta" => theme::purple(),
        "orange" => theme::orange(),
        _ => theme::green(),
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn parse_zone_kind(s: &str) -> ZoneKind {
    match s.to_lowercase().as_str() {
        "green" => ZoneKind::Safe,
        "yellow" | "warn" | "warning" => ZoneKind::Warn,
        "red" | "error" | "danger" => ZoneKind::Danger,
        _ => ZoneKind::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_value_from_json_prefers_value_field() {
        let json = serde_json::json!({"value": 42.5, "other": 99.0});
        assert_eq!(gauge_value_from_json(&json), Some(42.5));
    }

    #[test]
    fn gauge_value_from_json_falls_back_to_first_number() {
        let json = serde_json::json!({"temperature": 23.5});
        assert_eq!(gauge_value_from_json(&json), Some(23.5));
    }

    #[test]
    fn gauge_value_from_json_skips_timestamp() {
        let json = serde_json::json!({"t": 1000, "voltage": 3.3});
        assert_eq!(gauge_value_from_json(&json), Some(3.3));
    }

    #[test]
    fn gauge_value_from_text_parses_value_key() {
        assert_eq!(gauge_value_from_text("value=42.5"), Some(42.5));
    }

    #[test]
    fn gauge_value_from_text_falls_back() {
        assert_eq!(gauge_value_from_text("temp=23.5,hum=60"), Some(23.5));
    }

    #[test]
    fn value_to_fraction_basic() {
        assert!((value_to_fraction(50.0, 0.0, 100.0) - 0.5).abs() < f64::EPSILON);
        assert!((value_to_fraction(0.0, 0.0, 100.0)).abs() < f64::EPSILON);
        assert!((value_to_fraction(100.0, 0.0, 100.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn value_to_fraction_clamps() {
        assert!((value_to_fraction(-10.0, 0.0, 100.0)).abs() < f64::EPSILON);
        assert!((value_to_fraction(150.0, 0.0, 100.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_zones_basic() {
        let json = serde_json::json!([
            {"from": 0, "to": 60, "color": "green"},
            {"from": 60, "to": 80, "color": "yellow"},
            {"from": 80, "to": 100, "color": "red"}
        ]);
        let zones = parse_zones(Some(&json));
        assert_eq!(zones.len(), 3);
        assert_eq!(zones[0].from, 0.0);
        assert_eq!(zones[2].to, 100.0);
        assert_eq!(zones[0].color, theme::green());
    }

    #[test]
    fn status_text_auto_from_zone() {
        let zones = parse_zones(Some(&serde_json::json!([
            {"from": 0, "to": 60, "color": "green"},
            {"from": 60, "to": 80, "color": "yellow"},
            {"from": 80, "to": 100, "color": "red"}
        ])));
        let bus = tool_databus::DataBus::new();
        let mut gauge =
            GaugePanel::from_config(&bus, "t", 0.0, 100.0, String::new(), zones, String::new());

        gauge.set_value(40.0);
        assert_eq!(gauge.status_text(), "正常");
        gauge.set_value(70.0);
        assert_eq!(gauge.status_text(), "预警");
        gauge.set_value(90.0);
        assert_eq!(gauge.status_text(), "异常");
    }

    #[test]
    fn status_text_prefers_runtime_override() {
        let zones = parse_zones(Some(&serde_json::json!([
            {"from": 0, "to": 60, "color": "green"}
        ])));
        let bus = tool_databus::DataBus::new();
        let mut gauge =
            GaugePanel::from_config(&bus, "t", 0.0, 100.0, String::new(), zones, String::new());

        gauge.set_value(40.0); // 色区为 green -> 自动 "正常"
        assert_eq!(gauge.status_text(), "正常");

        gauge.set_status("预热中".to_owned());
        assert_eq!(gauge.status_text(), "预热中");

        gauge.set_status(String::new()); // 清空后回退到自动
        assert_eq!(gauge.status_text(), "正常");
    }

    #[test]
    fn value_color_uses_endmost_zone_outside_range() {
        // zones 未覆盖到 max：超限时取端点 zone 颜色，而非突变蓝色
        let zones = parse_zones(Some(&serde_json::json!([
            {"from": 0, "to": 2.5, "color": "red"},
            {"from": 2.5, "to": 3.0, "color": "yellow"},
            {"from": 3.0, "to": 3.6, "color": "green"}
        ])));
        let bus = tool_databus::DataBus::new();
        let mut gauge =
            GaugePanel::from_config(&bus, "t", 0.0, 5.0, String::new(), zones, String::new());

        gauge.set_value(4.5); // 超过最后 zone(3.6)，低于 max(5)
        assert_eq!(gauge.value_color(), theme::green()); // 取末个 zone 颜色
        assert_eq!(gauge.status_text(), "正常"); // green -> 正常

        gauge.set_value(-1.0); // 低于首个 zone
        assert_eq!(gauge.value_color(), theme::red()); // 取首个 zone 颜色
    }
}
