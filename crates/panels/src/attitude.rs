use crate::theme;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::Value;
use tool_core::{Event, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

pub struct AttitudePanel {
    subscription: Subscription,
    roll: f64,
    pitch: f64,
    yaw: f64,
    samples: usize,
    last_source: String,
}

impl AttitudePanel {
    pub fn new(bus: &DataBus) -> Self {
        Self::new_for_topic(bus, topics::PROTOCOL_IMU_ATTITUDE)
    }

    pub fn new_for_topic(bus: &DataBus, topic: impl Into<String>) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(TopicFilter::exact(topic.into()), 1024),
            roll: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            samples: 0,
            last_source: "none".to_owned(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.ingest();

        ui.horizontal_wrapped(|ui| {
            ui.label(format!("横滚 {:.2}", self.roll));
            ui.label(format!("俯仰 {:.2}", self.pitch));
            ui.label(format!("偏航 {:.2}", self.yaw));
            ui.label(format!("样本 {}", self.samples));
            ui.label(format!("来源 {}", self.last_source));
        });

        let desired = Vec2::new(ui.available_width(), 320.0);
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
                if let Some((roll, pitch, yaw)) = attitude_from_json(&value) {
                    self.roll = roll;
                    self.pitch = pitch;
                    self.yaw = yaw;
                    self.samples += 1;
                    self.last_source = event.source;
                }
            }
            Payload::Text(text) => {
                if let Some((roll, pitch, yaw)) = attitude_from_text(&text) {
                    self.roll = roll;
                    self.pitch = pitch;
                    self.yaw = yaw;
                    self.samples += 1;
                    self.last_source = event.source;
                }
            }
            Payload::Bytes(_) | Payload::Empty => {}
        }
    }

    fn paint(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, theme::ATTITUDE_BG);
        painter.rect_stroke(
            rect,
            4.0,
            Stroke::new(1.0, theme::BORDER_LIGHT),
            egui::StrokeKind::Inside,
        );

        let center = rect.center();
        let radius = rect.width().min(rect.height()) * 0.34;
        let axes = [
            (
                rotate([1.0, 0.0, 0.0], self.roll, self.pitch, self.yaw),
                theme::ATTITUDE_AXIS_X,
                "X",
            ),
            (
                rotate([0.0, 1.0, 0.0], self.roll, self.pitch, self.yaw),
                theme::ATTITUDE_AXIS_Y,
                "Y",
            ),
            (
                rotate([0.0, 0.0, 1.0], self.roll, self.pitch, self.yaw),
                theme::ATTITUDE_AXIS_Z,
                "Z",
            ),
        ];

        painter.circle_stroke(center, radius, Stroke::new(1.0, theme::BORDER_LIGHT));
        for (axis, color, label) in axes {
            let end = project(center, radius, axis);
            painter.line_segment([center, end], Stroke::new(3.0, color));
            painter.circle_filled(end, 4.0, color);
            painter.text(
                end,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        }

        draw_body(&painter, center, radius, self.roll, self.pitch, self.yaw);
    }
}

fn draw_body(painter: &egui::Painter, center: Pos2, radius: f32, roll: f64, pitch: f64, yaw: f64) {
    let points = [
        [-0.55, -0.3, -0.16],
        [0.55, -0.3, -0.16],
        [0.55, 0.3, -0.16],
        [-0.55, 0.3, -0.16],
        [-0.55, -0.3, 0.16],
        [0.55, -0.3, 0.16],
        [0.55, 0.3, 0.16],
        [-0.55, 0.3, 0.16],
    ];
    let projected = points.map(|point| project(center, radius, rotate(point, roll, pitch, yaw)));
    let edges = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];

    for (a, b) in edges {
        painter.line_segment(
            [projected[a], projected[b]],
            Stroke::new(1.5, theme::ATTITUDE_BODY),
        );
    }
}

fn attitude_from_json(value: &Value) -> Option<(f64, f64, f64)> {
    let roll = value.get("roll").and_then(Value::as_f64)?;
    let pitch = value.get("pitch").and_then(Value::as_f64)?;
    let yaw = value.get("yaw").and_then(Value::as_f64)?;
    Some((roll, pitch, yaw))
}

fn attitude_from_text(text: &str) -> Option<(f64, f64, f64)> {
    let mut roll = None;
    let mut pitch = None;
    let mut yaw = None;

    for part in text.split(',') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<f64>() else {
            continue;
        };
        match key.trim() {
            "roll" => roll = Some(value),
            "pitch" => pitch = Some(value),
            "yaw" => yaw = Some(value),
            _ => {}
        }
    }

    Some((roll?, pitch?, yaw?))
}

fn rotate(point: [f64; 3], roll: f64, pitch: f64, yaw: f64) -> [f64; 3] {
    let (sr, cr) = roll.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let (sy, cy) = yaw.to_radians().sin_cos();

    let [x, y, z] = point;
    let y1 = y * cr - z * sr;
    let z1 = y * sr + z * cr;
    let x2 = x * cp + z1 * sp;
    let z2 = -x * sp + z1 * cp;
    let x3 = x2 * cy - y1 * sy;
    let y3 = x2 * sy + y1 * cy;
    [x3, y3, z2]
}

fn project(center: Pos2, radius: f32, point: [f64; 3]) -> Pos2 {
    let x = (point[0] - point[1] * 0.28) as f32;
    let y = (-point[2] + point[1] * 0.28) as f32;
    Pos2::new(center.x + x * radius, center.y + y * radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_attitude_text() {
        assert_eq!(
            attitude_from_text("roll=1.5,pitch=-2,yaw=90"),
            Some((1.5, -2.0, 90.0))
        );
    }

    #[test]
    fn parses_attitude_json() {
        assert_eq!(
            attitude_from_json(&serde_json::json!({
                "roll": 1.0,
                "pitch": 2.0,
                "yaw": 3.0
            })),
            Some((1.0, 2.0, 3.0))
        );
    }
}
