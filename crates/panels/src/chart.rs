use crate::theme;
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tool_core::{Event, Payload};
use tool_databus::{DataBus, Subscription, TopicFilter};

const MAX_CHART_EVENTS_PER_FRAME: usize = 1_000;

pub struct ChartPanel {
    subscription: Subscription,
    series: BTreeMap<String, VecDeque<Sample>>,
    /// 缓存窗口化后的数据，避免每帧重复分配 Vec。
    /// Key 为 series 名称，Value 为窗口内样本（已按 x 排序）。
    cached_window: BTreeMap<String, Vec<Sample>>,
    paused: bool,
    auto_scale: bool,
    y_min: f64,
    y_max: f64,
    sample_window: usize,
    max_samples: usize,
    dropped_while_paused: u64,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    x: f64,
    y: f64,
}

impl ChartPanel {
    pub fn new(bus: &DataBus) -> Self {
        Self::new_with_filter(bus, TopicFilter::prefix("protocol."))
    }

    pub fn new_for_topic_prefix(bus: &DataBus, topic_prefix: impl Into<String>) -> Self {
        Self::new_with_filter(bus, TopicFilter::prefix(topic_prefix))
    }

    /// 精确订阅单个 topic（不匹配前缀下的其他 topic）。
    pub fn new_for_topic(bus: &DataBus, topic: impl Into<String>) -> Self {
        Self::new_with_filter(bus, TopicFilter::exact(topic))
    }

    fn new_with_filter(bus: &DataBus, filter: TopicFilter) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(filter, 4096),
            series: BTreeMap::new(),
            cached_window: BTreeMap::new(),
            paused: false,
            auto_scale: true,
            y_min: 0.0,
            y_max: 100.0,
            sample_window: 600,
            max_samples: 2_000,
            dropped_while_paused: 0,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.ingest();
        self.rebuild_cache();

        ui.horizontal(|ui| {
            ui.checkbox(&mut self.paused, "暂停").on_hover_text(
                "暂停采集：冻结图表，暂停期间到达的新样本会被丢弃。与终端的「暂停接收」行为一致。",
            );
            ui.checkbox(&mut self.auto_scale, "自动")
                .on_hover_text("自动缩放 Y 轴以完整显示所有可见样本。关闭后可手动设 min/max。");
            if !self.auto_scale {
                ui.label("Y 轴");
                ui.add(
                    egui::DragValue::new(&mut self.y_min)
                        .speed(1.0)
                        .prefix("min "),
                );
                ui.add(
                    egui::DragValue::new(&mut self.y_max)
                        .speed(1.0)
                        .prefix("max "),
                );
            }
            ui.add(egui::Slider::new(&mut self.sample_window, 60..=2_000).text("窗口"))
                .on_hover_text("X 轴显示的最近样本数。值越小，曲线刷新越频繁。");
            if ui
                .button("清空")
                .on_hover_text("清除图表中的所有样本数据")
                .clicked()
            {
                self.series.clear();
            }
            if self.dropped_while_paused > 0 {
                ui.label(
                    RichText::new(format!("暂停期间跳过 {} 个样本", self.dropped_while_paused))
                        .color(theme::TEXT_SECONDARY),
                );
            }
            let dropped = self.subscription.dropped_count();
            if dropped > 0 {
                ui.colored_label(
                    theme::YELLOW,
                    format!("队列溢出丢弃 {dropped} 条，曲线可能不完整"),
                );
            }
        });

        let desired = Vec2::new(ui.available_width(), 280.0);
        let (rect, response) = ui.allocate_exact_size(desired, Sense::hover());
        self.paint_chart(ui, rect, &response);
        self.legend(ui);
    }

    fn ingest(&mut self) {
        if self.paused {
            for _ in 0..MAX_CHART_EVENTS_PER_FRAME {
                if self.subscription.try_recv_arc().is_none() {
                    break;
                }
                self.dropped_while_paused += 1;
            }
            return;
        }

        // 零 clone 批量消费：取 Arc<Event> 引用，避免每事件深克隆 payload。
        let events = self
            .subscription
            .drain_limited_arc(MAX_CHART_EVENTS_PER_FRAME);
        for arc in events {
            self.push_event(&arc);
        }
    }

    fn push_event(&mut self, event: &Arc<Event>) {
        match &event.payload {
            Payload::Json(value) => self.push_json(event.timestamp_ms as f64, value),
            Payload::Text(text) => self.push_text(event.timestamp_ms as f64, text),
            Payload::Bytes(_) | Payload::Empty => {}
        }
    }

    fn push_json(&mut self, fallback_x: f64, value: &Value) {
        let Some(object) = value.as_object() else {
            return;
        };
        let x = object
            .get("t")
            .or_else(|| object.get("time"))
            .or_else(|| object.get("timestamp"))
            .and_then(Value::as_f64)
            .unwrap_or(fallback_x);

        for (name, value) in object {
            if matches!(name.as_str(), "t" | "time" | "timestamp") {
                continue;
            }
            if let Some(y) = value.as_f64() {
                self.push_sample(name, Sample { x, y });
            }
        }
    }

    fn push_text(&mut self, fallback_x: f64, text: &str) {
        let mut x = fallback_x;
        let mut values = Vec::new();

        for part in text.split(',') {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let Ok(value) = value.trim().parse::<f64>() else {
                continue;
            };
            if matches!(key, "t" | "time" | "timestamp") {
                x = value;
            } else {
                values.push((key.to_owned(), value));
            }
        }

        for (name, y) in values {
            self.push_sample(&name, Sample { x, y });
        }
    }

    pub fn clear(&mut self) {
        while self.subscription.try_recv().is_some() {}
        self.series.clear();
        self.cached_window.clear();
        self.dropped_while_paused = 0;
    }

    /// 返回当前所有 series 名称（供测试与诊断）。
    pub fn series_keys(&self) -> Vec<&String> {
        self.series.keys().collect()
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        if self.paused {
            let mut drained = 0;
            while self.subscription.try_recv_arc().is_some() {
                drained += 1;
            }
            self.dropped_while_paused += drained;
            return 0;
        }

        let mut count = 0;
        for arc in self.subscription.drain_arc() {
            self.push_event(&arc);
            count += 1;
        }

        count
    }

    fn push_sample(&mut self, name: &str, sample: Sample) {
        let samples = self.series.entry(name.to_owned()).or_default();
        samples.push_back(sample);
        while samples.len() > self.max_samples {
            samples.pop_front();
        }
    }

    fn rebuild_cache(&mut self) {
        self.cached_window.clear();
        for (name, samples) in &self.series {
            let window: Vec<Sample> = samples
                .iter()
                .rev()
                .take(self.sample_window)
                .rev()
                .copied()
                .collect();
            self.cached_window.insert(name.clone(), window);
        }
    }

    fn paint_chart(&self, ui: &egui::Ui, rect: Rect, response: &egui::Response) {
        let painter = ui.painter_at(rect);
        // 无边框嵌入：不画背景填充与外边框，直接在面板背景上绘制网格与曲线

        if self.cached_window.values().all(|values| values.len() < 2) {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "无采样数据",
                egui::FontId::proportional(14.0),
                theme::TEXT_SECONDARY,
            );
            return;
        }

        let samples: Vec<(&String, &Vec<Sample>)> = self.cached_window.iter().collect();

        let bounds = chart_bounds(&samples, self.auto_scale, self.y_min, self.y_max);
        draw_grid(&painter, rect);
        draw_y_axis_labels(&painter, rect, bounds);
        draw_x_axis_labels(&painter, rect, bounds);

        for (index, (_, values)) in samples.iter().enumerate() {
            if values.len() < 2 {
                continue;
            }
            let points = values
                .iter()
                .map(|sample| map_point(rect, *sample, bounds))
                .collect::<Vec<_>>();
            painter.line(points, Stroke::new(2.0, palette(index)));
        }

        // ── 十字线 + Tooltip ──
        if let Some(hover_pos) = response.hover_pos()
            && rect.contains(hover_pos)
        {
            // 十字线
            painter.line_segment(
                [
                    Pos2::new(hover_pos.x, rect.top()),
                    Pos2::new(hover_pos.x, rect.bottom()),
                ],
                Stroke::new(1.0, theme::CHART_CROSSHAIR),
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left(), hover_pos.y),
                    Pos2::new(rect.right(), hover_pos.y),
                ],
                Stroke::new(1.0, theme::CHART_CROSSHAIR),
            );

            // 找到 hover X 对应的数据值
            let (min_x, max_x, min_y, max_y) = bounds;
            let hover_x_ratio = ((hover_pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let hover_x_val = min_x + hover_x_ratio as f64 * (max_x - min_x);
            let _hover_y_val = min_y
                + (1.0 - ((hover_pos.y - rect.top()) / rect.height()) as f64) * (max_y - min_y);

            // 收集各 series 在 hover X 附近的值
            let mut tooltip_lines = vec![format!("x: {hover_x_val:.1}")];
            for (index, (name, values)) in samples.iter().enumerate() {
                // 二分查找最近的样本
                if let Some(closest) = find_closest_sample(values, hover_x_val) {
                    tooltip_lines.push(format!("{}: {:.3}", name, closest.y));
                    // 在数据点上画高亮圆点
                    let point = map_point(rect, *closest, bounds);
                    painter.circle_filled(point, 4.0, palette(index));
                }
            }

            // 绘制 Tooltip 背景 + 文字
            let tooltip_lines = tooltip_lines;
            let font = egui::FontId::proportional(11.0);
            let line_height = 14.0;
            let tooltip_width = 100.0;
            let tooltip_height = tooltip_lines.len() as f32 * line_height + 6.0;

            // Tooltip 位置：优先右上方，溢出则左移
            let mut tooltip_pos = Pos2::new(hover_pos.x + 8.0, hover_pos.y - tooltip_height - 4.0);
            if tooltip_pos.x + tooltip_width > rect.right() {
                tooltip_pos.x = hover_pos.x - tooltip_width - 8.0;
            }
            if tooltip_pos.y < rect.top() {
                tooltip_pos.y = hover_pos.y + 8.0;
            }

            let tooltip_rect =
                Rect::from_min_size(tooltip_pos, Vec2::new(tooltip_width, tooltip_height));
            painter.rect_filled(tooltip_rect, 4.0, theme::CHART_TOOLTIP_BG);
            painter.rect_stroke(
                tooltip_rect,
                4.0,
                Stroke::new(1.0, theme::BORDER_LIGHT),
                egui::StrokeKind::Inside,
            );

            for (i, line) in tooltip_lines.iter().enumerate() {
                let color = if i == 0 {
                    theme::TEXT_DIMMED
                } else {
                    palette(i - 1)
                };
                painter.text(
                    Pos2::new(
                        tooltip_rect.left() + 4.0,
                        tooltip_rect.top() + 3.0 + i as f32 * line_height,
                    ),
                    egui::Align2::LEFT_TOP,
                    line,
                    font.clone(),
                    color,
                );
            }
        }
    }

    fn legend(&self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (index, (name, samples)) in self.series.iter().enumerate() {
                let latest = samples.back().map(|sample| sample.y).unwrap_or_default();
                ui.label(RichText::new("--").color(palette(index)));
                ui.label(format!("{name}: {latest:.3}"));
            }
        });
    }
}

/// 二分查找 x 值最近的样本
fn find_closest_sample(values: &[Sample], target_x: f64) -> Option<&Sample> {
    if values.is_empty() {
        return None;
    }
    let pos = values.partition_point(|s| s.x < target_x);
    match (pos.checked_sub(1), values.get(pos)) {
        (Some(prev), Some(next)) => {
            if (values[prev].x - target_x).abs() <= (next.x - target_x).abs() {
                Some(&values[prev])
            } else {
                Some(next)
            }
        }
        (Some(prev), None) => Some(&values[prev]),
        (None, Some(next)) => Some(next),
        (None, None) => None,
    }
}

fn chart_bounds(
    samples: &[(&String, &Vec<Sample>)],
    auto_scale: bool,
    y_min: f64,
    y_max: f64,
) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let (manual_min_y, manual_max_y) = if y_min <= y_max {
        (y_min, y_max)
    } else {
        (y_max, y_min)
    };
    let mut min_y = if auto_scale {
        f64::INFINITY
    } else {
        manual_min_y
    };
    let mut max_y = if auto_scale {
        f64::NEG_INFINITY
    } else {
        manual_max_y
    };

    for (_, values) in samples {
        for sample in *values {
            min_x = min_x.min(sample.x);
            max_x = max_x.max(sample.x);
            if auto_scale {
                min_y = min_y.min(sample.y);
                max_y = max_y.max(sample.y);
            }
        }
    }

    if !min_x.is_finite() || !max_x.is_finite() || (max_x - min_x).abs() < f64::EPSILON {
        min_x = 0.0;
        max_x = 1.0;
    }
    if !min_y.is_finite() || !max_y.is_finite() || (max_y - min_y).abs() < f64::EPSILON {
        min_y -= 1.0;
        max_y += 1.0;
    }

    (min_x, max_x, min_y, max_y)
}

fn map_point(rect: Rect, sample: Sample, bounds: (f64, f64, f64, f64)) -> Pos2 {
    let (min_x, max_x, min_y, max_y) = bounds;
    let x = ((sample.x - min_x) / (max_x - min_x)) as f32;
    let y = ((sample.y - min_y) / (max_y - min_y)) as f32;
    Pos2::new(
        egui::lerp(rect.left()..=rect.right(), x),
        egui::lerp(rect.bottom()..=rect.top(), y),
    )
}

fn draw_grid(painter: &egui::Painter, rect: Rect) {
    for index in 1..5 {
        let t = index as f32 / 5.0;
        let x = egui::lerp(rect.left()..=rect.right(), t);
        let y = egui::lerp(rect.top()..=rect.bottom(), t);
        let stroke = Stroke::new(1.0, theme::CHART_GRID);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            stroke,
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            stroke,
        );
    }
}

/// 绘制 Y 轴 5 个均匀刻度标签
fn draw_y_axis_labels(painter: &egui::Painter, rect: Rect, bounds: (f64, f64, f64, f64)) {
    let (_, _, min_y, max_y) = bounds;
    let font = egui::FontId::proportional(11.0);
    let color = theme::TEXT_DIMMED;

    for i in 0..5 {
        let t = i as f32 / 4.0; // 0, 0.25, 0.5, 0.75, 1.0
        let y = egui::lerp(rect.bottom()..=rect.top(), t);
        let val = min_y + (max_y - min_y) * t as f64;
        let label = format!("{val:.1}");
        painter.text(
            Pos2::new(rect.left() + 4.0, y),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
    }
}

/// 绘制 X 轴标签（左下角 min，右下角 max）
fn draw_x_axis_labels(painter: &egui::Painter, rect: Rect, bounds: (f64, f64, f64, f64)) {
    let (min_x, max_x, _, _) = bounds;
    let font = egui::FontId::proportional(11.0);
    let color = theme::TEXT_DIMMED;

    painter.text(
        Pos2::new(rect.left() + 4.0, rect.bottom() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{min_x:.0}"),
        font.clone(),
        color,
    );
    painter.text(
        Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("{max_x:.0}"),
        font,
        color,
    );
}

fn palette(index: usize) -> Color32 {
    theme::CHART_COLORS[index % theme::CHART_COLORS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_core::Event;
    use tool_databus::DataBus;

    #[test]
    fn manual_y_axis_is_normalized() {
        let name = "actual".to_owned();
        let data = vec![Sample { x: 0.0, y: 10.0 }, Sample { x: 1.0, y: 20.0 }];
        let samples: Vec<(&String, &Vec<Sample>)> = vec![(&name, &data)];

        let (_, _, min_y, max_y) = chart_bounds(&samples, false, 100.0, 0.0);

        assert_eq!((min_y, max_y), (0.0, 100.0));
    }

    #[test]
    fn equal_y_axis_expands_to_visible_range() {
        let name = "actual".to_owned();
        let data = vec![Sample { x: 0.0, y: 10.0 }, Sample { x: 1.0, y: 10.0 }];
        let samples: Vec<(&String, &Vec<Sample>)> = vec![(&name, &data)];

        let (_, _, min_y, max_y) = chart_bounds(&samples, false, 10.0, 10.0);

        assert!(min_y < 10.0);
        assert!(max_y > 10.0);
    }

    #[test]
    fn clear_drains_pending_chart_events() {
        let bus = DataBus::new();
        let mut panel = ChartPanel::new(&bus);

        bus.publish(Event::json(
            tool_core::topics::PROTOCOL_PID_SAMPLE,
            "test",
            serde_json::json!({ "t": 1, "value": 2.0 }),
        ));
        panel.clear();

        assert_eq!(panel.ingest_all_pending(), 0);
        assert!(panel.series.is_empty());
    }

    #[test]
    fn find_closest_sample_finds_nearest() {
        let samples = vec![
            Sample { x: 1.0, y: 10.0 },
            Sample { x: 2.0, y: 20.0 },
            Sample { x: 3.0, y: 30.0 },
        ];
        assert_eq!(find_closest_sample(&samples, 1.8).map(|s| s.y), Some(20.0));
        assert_eq!(find_closest_sample(&samples, 2.5).map(|s| s.y), Some(20.0)); // 等距时取前者
        assert_eq!(find_closest_sample(&samples, 0.5).map(|s| s.y), Some(10.0));
        assert_eq!(find_closest_sample(&samples, 3.5).map(|s| s.y), Some(30.0));
    }
}
