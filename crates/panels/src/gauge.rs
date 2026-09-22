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
        let value = match &event.payload {
            Payload::Json(value) => gauge_value_from_json(value),
            Payload::Text(text) => gauge_value_from_text(text),
            Payload::Bytes(_) | Payload::Empty => None,
        };
        if let Some(value) = value {
            self.value = value;
            self.samples += 1;
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
        self.zone_for_value()
            .map_or(ZoneKind::None, |zone| zone.kind)
    }

    /// 当前值对应的色区：优先取值落在 `[from, to]` 内的那个。
    ///
    /// 越界时不返回 `None`：低于所有色区取首个、高于取末个，让颜色和状态都跟着端点
    /// 色区走，避免超限时突变成一条无关的默认色。没有任何色区时才返回 `None`。
    fn zone_for_value(&self) -> Option<&GaugeZone> {
        for zone in &self.zones {
            if self.value >= zone.from && self.value <= zone.to {
                return Some(zone);
            }
        }
        let first = self.zones.first()?;
        if self.value < first.from {
            return Some(first);
        }
        let last = self.zones.last()?;
        if self.value > last.to {
            return Some(last);
        }
        None
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
            &ArcSpec {
                center,
                radius,
                start_deg: start_angle_deg,
                sweep_deg,
                from_t: 0.0,
                to_t: 1.0,
            },
            arc_steps,
            Stroke::new(6.0, theme::gauge_arc()),
        );

        // 画色区弧（叠在底轨道上，覆盖各自区间）
        for zone in &self.zones {
            let from_t = value_to_fraction(zone.from, self.min, self.max) as f32;
            let to_t = value_to_fraction(zone.to, self.min, self.max) as f32;
            draw_arc(
                &painter,
                &ArcSpec {
                    center,
                    radius,
                    start_deg: start_angle_deg,
                    sweep_deg,
                    from_t,
                    to_t,
                },
                arc_steps,
                Stroke::new(6.0, zone.color),
            );
        }

        // 画当前值对应的彩色弧段（叠在色区轨道上，更粗更亮）
        let value_t = value_to_fraction(self.value, self.min, self.max).clamp(0.0, 1.0) as f32;
        let value_color = self.value_color();
        draw_arc(
            &painter,
            &ArcSpec {
                center,
                radius,
                start_deg: start_angle_deg,
                sweep_deg,
                from_t: 0.0,
                to_t: value_t,
            },
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

    /// 当前值的颜色：取 [`Self::zone_for_value`] 命中的色区颜色，
    /// 没有任何色区可用时退回默认的值颜色。
    fn value_color(&self) -> Color32 {
        self.zone_for_value()
            .map_or(theme::gauge_value(), |zone| zone.color)
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
    let usable = rect.height() - 2.0 * STROKE_HALF;
    let by_height = (usable - TEXT_BOTTOM).min(usable / 1.707).max(MIN_RADIUS);
    by_width.min(by_height)
}

/// 一段弧的纯几何输入，与画笔和线宽无关 —— 拆出来是为了让折点这步能被单测直接驱动。
///
/// **两套单位混在一起，务必按字段名对齐**：`start_deg` / `sweep_deg` 是**角度（度）**，
/// `from_t` / `to_t` 是 [0, 1] 的**归一化位置**（0 = 起始角，1 = 终止角）。
/// 两者同为 `f32`，换位编译照过；把 `sweep_deg`（270.0）当成 `from_t` 送进去，
/// 弧度会退化成反向的一小段，图上完全走形。
pub(crate) struct ArcSpec {
    /// 圆心（屏幕坐标）。
    center: Pos2,
    /// 半径（像素），即弧上每一点到 `center` 的距离。
    radius: f32,
    /// 弧起始角度（度）。egui 屏幕坐标 y 向下，故角度递增 = 顺时针。
    start_deg: f32,
    /// 从起始角顺时针扫过的角度（度），不是终止角本身。
    sweep_deg: f32,
    /// 本段弧起点在整条弧上的归一化位置，[0, 1]。
    from_t: f32,
    /// 本段弧终点在整条弧上的归一化位置，[0, 1]。
    to_t: f32,
}

/// 按 `steps` 段把 `spec` 描述的弧折成屏幕坐标点列（共 `steps + 1` 个点）。
pub(crate) fn arc_points(spec: &ArcSpec, steps: usize) -> Vec<Pos2> {
    let from_deg = spec.start_deg + spec.sweep_deg * spec.from_t;
    let to_deg = spec.start_deg + spec.sweep_deg * spec.to_t;
    let span = to_deg - from_deg;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = (from_deg + span * t).to_radians();
        points.push(Pos2::new(
            spec.center.x + spec.radius * angle.cos(),
            spec.center.y + spec.radius * angle.sin(),
        ));
    }
    points
}

fn draw_arc(painter: &egui::Painter, spec: &ArcSpec, steps: usize, stroke: Stroke) {
    let points = arc_points(spec, steps);
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
        // 区域颜色在 parse_zones 里就解析完了，断言时读的是**当时**的调色板。
        let _theme = crate::theme::lock_theme_globals();
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
        let _theme = crate::theme::lock_theme_globals();
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

    // ── 弧线几何：把 arc_points 的字段语义逐个钉死 ────────────────────────────
    //
    // `paint` 里三条弧共用同一组常量：满轨从 135° 起、顺时针扫 270°，底部
    // 45°..135° 留 90° 缺口。下面每个期望角度/期望距离都是**手算写死**的字面量，
    // 不从 spec 现推 —— 否则字段接错会连期望值一起改掉，测试就跟着错下去。

    /// 断言容差。量级权衡（四条变异实测）：
    /// - f32 在 ~1e2 像素这个量级上的舍入噪声不超过 1e-5；
    /// - 而字段换位造成的最小偏差是 **90° 弧度偏移**（`from_t`↔`to_t` 把满轨端点
    ///   从 45° 挪到 135°）与 **105 像素半径偏差**（`radius`↔`start_deg` 让弧上
    ///   的点到圆心 135 而非 30）。
    ///
    /// 两边相差约六个数量级：取 1e-4 既不会被浮点噪声误触发，也不会放过任一种换位。
    const EPS: f32 = 1e-4;

    /// 把折出来的点换算回「相对圆心的角度（度，归一到 [0, 360)）」。
    /// 走 atan2，与被测实现的 cos/sin 方向相反，不共享它的中间量。
    fn angle_at(p: Pos2, center: Pos2) -> f32 {
        (p.y - center.y)
            .atan2(p.x - center.x)
            .to_degrees()
            .rem_euclid(360.0)
    }

    /// 点到圆心的距离 —— 也就是 `radius` 的定义本身。
    fn distance_at(p: Pos2, center: Pos2) -> f32 {
        ((p.x - center.x).powi(2) + (p.y - center.y).powi(2)).sqrt()
    }

    /// 满轨：`from_t = 0.0`、`to_t = 1.0`，首尾点必须落在仪表盘真正使用的
    /// 两个角度上（min 左下 135°、max 右下 405°）。
    #[test]
    fn full_track_runs_from_the_min_angle_to_the_max_angle() {
        let center = Pos2::new(40.0, 60.0);
        let points = arc_points(
            &ArcSpec {
                center,
                radius: 25.0,
                start_deg: 135.0,
                sweep_deg: 270.0,
                from_t: 0.0,
                to_t: 1.0,
            },
            8,
        );
        assert_eq!(points.len(), 9);

        let first = angle_at(points[0], center);
        assert!(
            (first - 135.0).abs() < EPS,
            "起点应是起始角 135°（min，屏幕左下），实测 {first}"
        );
        let last = angle_at(points[points.len() - 1], center);
        assert!(
            (last - 45.0).abs() < EPS,
            "终点应是 135+270=405° 即 45°（max，屏幕右下），实测 {last}"
        );
        // 角度不能整体偏移，也不能越过底部 90° 缺口。
        for point in &points {
            let deg = angle_at(*point, center);
            assert!(
                deg <= 45.0 + EPS || deg >= 135.0 - EPS,
                "折点 {deg}° 落进了底部缺口 45°..135°，弧线不该经过这里"
            );
        }
    }

    /// 局部色区弧：`from_t`/`to_t` 都严格落在 (0, 1) 内，角度与距离双重约束。
    #[test]
    fn partial_zone_stays_inside_its_own_swept_sector() {
        let center = Pos2::new(-12.5, 30.0);
        let points = arc_points(
            &ArcSpec {
                center,
                radius: 33.0,
                start_deg: 135.0,
                sweep_deg: 270.0,
                from_t: 0.25,
                to_t: 0.60,
            },
            6,
        );
        assert_eq!(points.len(), 7);

        // 期望值按定义手算：135 + 270*0.25 = 202.5，135 + 270*0.60 = 297。
        let start_deg = 202.5;
        let end_deg = 297.0;
        let first = angle_at(points[0], center);
        let last = angle_at(points[points.len() - 1], center);
        assert!(
            (first - start_deg).abs() < EPS,
            "25% 处应是 {start_deg}°，实测 {first}"
        );
        assert!(
            (last - end_deg).abs() < EPS,
            "60% 处应是 {end_deg}°，实测 {last}"
        );
        // 逐点：距离恒为 radius，角度在 [202.5, 297] 内顺时针严格推进。
        // `sweep_deg`(270.0) 若占了 `from_t` 的槽位，to_deg 退化成
        // 135 + 0.25*0.60 = 135.15，弧会倒扫回满轨起点之外，这里立刻越界。
        for window in points.windows(2) {
            let a = angle_at(window[0], center);
            let b = angle_at(window[1], center);
            assert!(b > a, "折点应顺时针严格推进，实测 {a} -> {b}");
            assert!(
                (start_deg - EPS..=end_deg + EPS).contains(&a),
                "折点 {a}° 越出色区自己的扇形 [{start_deg}, {end_deg}]"
            );
        }
        for point in &points {
            let d = distance_at(*point, center);
            assert!((d - 33.0).abs() < EPS, "色区弧的半径应保持 33，实测 {d}");
        }
    }

    /// `radius` 只由「到圆心的距离」定义，与任何角度无关。
    #[test]
    fn every_point_sits_exactly_radius_from_the_center() {
        let center = Pos2::new(7.0, -3.0);
        let points = arc_points(
            &ArcSpec {
                center,
                radius: 30.0,
                start_deg: 135.0,
                sweep_deg: 270.0,
                from_t: 0.0,
                to_t: 1.0,
            },
            12,
        );
        assert_eq!(points.len(), 13);
        for point in &points {
            let d = distance_at(*point, center);
            assert!(
                (d - 30.0).abs() < EPS,
                "radius=30 的弧上出现了距圆心 {d} 的点：radius 被当成角度用了"
            );
        }
    }

    /// `steps` 的形状：点列长度恒为 `steps + 1`，首尾正好是 `from_t`/`to_t`。
    #[test]
    fn point_count_is_always_steps_plus_one() {
        let center = Pos2::new(0.0, 0.0);
        let spec = ArcSpec {
            center,
            radius: 10.0,
            start_deg: 135.0,
            sweep_deg: 270.0,
            from_t: 0.25,
            to_t: 0.60,
        };
        for steps in [1usize, 2, 7, 60, 101] {
            let points = arc_points(&spec, steps);
            assert_eq!(points.len(), steps + 1, "steps={steps} 的点列长度变了");
            assert!(
                (angle_at(points[0], center) - 202.5).abs() < EPS,
                "steps={steps} 时起点漂了"
            );
            let last = points.len() - 1;
            assert!(
                (angle_at(points[last], center) - 297.0).abs() < EPS,
                "steps={steps} 时终点漂了"
            );
        }
    }

    /// `steps = 0` 的既有形状：`t = 0/0 = NaN`，只折出一个非有限点。
    ///
    /// 本轮**刻意不修**它。这是抽取之前就有的算术，而 `paint` 里三条弧传的都是
    /// `let arc_steps = 60`（函数内唯一绑定、非 cfg 分支、没有再赋值），
    /// 所以 0 到今天都不可达；即便将来有调用者传 0，`draw_arc` 的
    /// `points.len() >= 2` 会让它什么都不画，NaN 坐标也漏不到画笔上。
    /// 改它属于未被要求的运行时行为变更，因此这里只把现状钉住：
    /// 谁要加保护（或放行 0 段），都会先撞上这条断言，被提醒做一次权衡。
    #[test]
    fn zero_steps_folds_a_single_non_finite_point() {
        let spec = ArcSpec {
            center: Pos2::new(0.0, 0.0),
            radius: 10.0,
            start_deg: 135.0,
            sweep_deg: 270.0,
            from_t: 0.0,
            to_t: 1.0,
        };
        let points = arc_points(&spec, 0);
        assert_eq!(
            points.len(),
            1,
            "steps=0 的点列长度变了，0/0 的形状被改动了"
        );
        assert!(
            !points[0].x.is_finite() && !points[0].y.is_finite(),
            "steps=0 仍应折出非有限点（现状），实测 {:?}",
            points[0]
        );
    }
}
