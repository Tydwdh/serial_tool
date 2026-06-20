use crate::theme;
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
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
            ui.checkbox(&mut self.paused, "暂停");
            ui.checkbox(&mut self.auto_scale, "自动");
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
            ui.add(egui::Slider::new(&mut self.sample_window, 60..=2_000).text("窗口"));
            if ui.button("清空").clicked() {
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
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        self.paint_chart(ui, rect);
        self.legend(ui);
    }

    fn ingest(&mut self) {
        if self.paused {
            // 暂停时 drain 掉积压事件，避免恢复后补处理大量旧数据
            for _ in 0..MAX_CHART_EVENTS_PER_FRAME {
                if self.subscription.try_recv().is_none() {
                    break;
                }
                self.dropped_while_paused += 1;
            }
            return;
        }

        for _ in 0..MAX_CHART_EVENTS_PER_FRAME {
            let Some(event) = self.subscription.try_recv() else {
                break;
            };

            self.push_event(event);
        }
    }

    fn push_event(&mut self, event: Event) {
        match event.payload {
            Payload::Json(value) => self.push_json(event.timestamp_ms as f64, value),
            Payload::Text(text) => self.push_text(event.timestamp_ms as f64, &text),
            Payload::Bytes(_) | Payload::Empty => {}
        }
    }

    fn push_json(&mut self, fallback_x: f64, value: Value) {
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
        self.series.clear();
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        if self.paused {
            // 暂停时 drain 积压事件，避免恢复后补处理大量旧数据
            let mut drained = 0;
            while self.subscription.try_recv().is_some() {
                drained += 1;
            }
            self.dropped_while_paused += drained;
            return 0;
        }

        let mut count = 0;

        while let Some(event) = self.subscription.try_recv() {
            self.push_event(event);
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

    fn paint_chart(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, theme::CHART_BG);
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, theme::BORDER_LIGHT),
            egui::StrokeKind::Inside,
        );

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

        // 直接迭代缓存，避免每帧 collect 分配 Vec
        let samples: Vec<(&String, &Vec<Sample>)> = self.cached_window.iter().collect();

        let bounds = chart_bounds(&samples, self.auto_scale, self.y_min, self.y_max);
        draw_grid(&painter, rect);
        draw_axis_labels(&painter, rect, bounds);

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

fn draw_axis_labels(painter: &egui::Painter, rect: Rect, bounds: (f64, f64, f64, f64)) {
    let (min_x, max_x, min_y, max_y) = bounds;
    let font = egui::FontId::proportional(11.0);
    let color = theme::TEXT_DIMMED;

    // Y 轴标签 (顶部和底部)
    let top_label = format!("{max_y:.1}");
    let bottom_label = format!("{min_y:.1}");
    painter.text(
        Pos2::new(rect.left() + 4.0, rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        top_label,
        font.clone(),
        color,
    );
    painter.text(
        Pos2::new(rect.left() + 4.0, rect.bottom() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        bottom_label,
        font.clone(),
        color,
    );

    // X 轴标签 (左侧和右侧)
    let left_label = format!("{min_x:.0}");
    let right_label = format!("{max_x:.0}");
    painter.text(
        Pos2::new(rect.left() + 4.0, rect.bottom() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        left_label,
        font.clone(),
        color,
    );
    painter.text(
        Pos2::new(rect.right() - 4.0, rect.bottom() - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        right_label,
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
}
