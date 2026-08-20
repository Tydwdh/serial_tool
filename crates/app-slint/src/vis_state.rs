use std::collections::{BTreeMap, VecDeque};

use serde_json::Value;
use tool_databus::{DataBus, Subscription, TopicFilter};

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub x: f64,
    pub y: f64,
}

// ── Chart ────────────────────────────────────────────────────
pub struct ChartState {
    subscription: Subscription,
    pub series: BTreeMap<String, VecDeque<Sample>>,
    pub paused: bool,
    pub auto_scale: bool,
    pub y_min: f64,
    pub y_max: f64,
    pub sample_window: usize,
    pub max_samples: usize,
    pub dropped_while_paused: u64,
}
impl ChartState {
    pub fn subscription_dropped(&self) -> u64 {
        self.subscription.dropped_count()
    }
}
impl ChartState {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(TopicFilter::prefix(String::from("protocol.")), 4096),
            series: BTreeMap::new(),
            paused: false,
            auto_scale: true,
            y_min: 0.0,
            y_max: 100.0,
            sample_window: 600,
            max_samples: 2000,
            dropped_while_paused: 0,
        }
    }
    pub fn ingest(&mut self) {
        if self.paused {
            for _ in 0..1000 {
                if self.subscription.try_recv().is_none() {
                    break;
                }
                self.dropped_while_paused += 1;
            }
            return;
        }
        for event in self.subscription.drain_limited(1000) {
            let x = event.timestamp_ms as f64;
            match &event.payload {
                tool_core::Payload::Json(v) => self.push_json(x, v),
                tool_core::Payload::Text(t) => self.push_text(x, t),
                _ => {}
            }
        }
    }
    fn push_json(&mut self, fallback_x: f64, value: &Value) {
        let Some(obj) = value.as_object() else {
            return;
        };
        let x = obj
            .get("t")
            .or_else(|| obj.get("time"))
            .or_else(|| obj.get("timestamp"))
            .and_then(Value::as_f64)
            .unwrap_or(fallback_x);
        for (name, v) in obj {
            if matches!(name.as_str(), "t" | "time" | "timestamp") {
                continue;
            }
            if let Some(y) = v.as_f64() {
                self.push_sample(name, Sample { x, y });
            }
        }
    }
    fn push_text(&mut self, fallback_x: f64, text: &str) {
        let mut x = fallback_x;
        let mut vals = Vec::new();
        for part in text.split(',') {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            let k = k.trim();
            let Ok(v) = v.trim().parse::<f64>() else {
                continue;
            };
            if matches!(k, "t" | "time" | "timestamp") {
                x = v;
            } else {
                vals.push((k.to_owned(), v));
            }
        }
        for (name, y) in vals {
            self.push_sample(&name, Sample { x, y });
        }
    }
    fn push_sample(&mut self, name: &str, s: Sample) {
        let q = self.series.entry(name.to_owned()).or_default();
        q.push_back(s);
        while q.len() > self.max_samples {
            q.pop_front();
        }
    }
    pub fn clear(&mut self) {
        while self.subscription.try_recv().is_some() {}
        self.series.clear();
        self.dropped_while_paused = 0;
    }
    pub fn bounds_y(&self) -> (f64, f64) {
        if self.auto_scale {
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            for q in self.series.values() {
                let take = q.len().min(self.sample_window);
                for s in q.iter().skip(q.len().saturating_sub(take)) {
                    min = min.min(s.y);
                    max = max.max(s.y);
                }
            }
            if min.is_finite() && max.is_finite() {
                if (max - min).abs() < 1e-9 {
                    return (min - 1.0, max + 1.0);
                }
                let pad = (max - min) * 0.1;
                return (min - pad, max + pad);
            }
            (0.0, 100.0)
        } else {
            (self.y_min, self.y_max)
        }
    }
}

// ── Attitude ─────────────────────────────────────────────────
pub struct AttitudeState {
    subscription: Subscription,
    pub roll: f64,
    pub pitch: f64,
    pub yaw: f64,
    pub samples: usize,
    pub last_source: String,
}
impl AttitudeState {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe_lossy_bounded(
                TopicFilter::exact(String::from(tool_core::topics::PROTOCOL_IMU_ATTITUDE)),
                1024,
            ),
            roll: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            samples: 0,
            last_source: "none".to_owned(),
        }
    }
    pub fn ingest(&mut self) {
        for e in self.subscription.drain_limited(500) {
            let parsed = match &e.payload {
                tool_core::Payload::Json(v) => attitude_from_json(v),
                tool_core::Payload::Text(t) => attitude_from_text(t),
                _ => None,
            };
            if let Some((r, p, y)) = parsed {
                self.roll = r;
                self.pitch = p;
                self.yaw = y;
                self.samples += 1;
                self.last_source = e.source.clone();
            }
        }
    }
    pub fn clear(&mut self) {
        while self.subscription.try_recv().is_some() {}
        self.roll = 0.0;
        self.pitch = 0.0;
        self.yaw = 0.0;
        self.samples = 0;
    }
}
fn attitude_from_json(v: &Value) -> Option<(f64, f64, f64)> {
    let roll = v.get("roll").and_then(Value::as_f64)?;
    let pitch = v.get("pitch").and_then(Value::as_f64)?;
    let yaw = v.get("yaw").and_then(Value::as_f64)?;
    Some((roll, pitch, yaw))
}
fn attitude_from_text(t: &str) -> Option<(f64, f64, f64)> {
    let mut r = None;
    let mut p = None;
    let mut y = None;
    for part in t.split(',') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let Ok(f) = v.trim().parse::<f64>() else {
            continue;
        };
        match k.trim() {
            "roll" => r = Some(f),
            "pitch" => p = Some(f),
            "yaw" => y = Some(f),
            _ => {}
        }
    }
    Some((r?, p?, y?))
}

// ── Gauge ────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    Safe,
    Warn,
    Danger,
    None,
}
pub struct GaugeZone {
    pub from: f64,
    pub to: f64,
    pub color: [u8; 4], // rgba
    pub kind: ZoneKind,
}
pub struct GaugeState {
    subscription: Subscription,
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub zones: Vec<GaugeZone>,
    pub label: String,
    pub status: String,
    pub samples: usize,
}
impl GaugeState {
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
    pub fn ingest(&mut self) {
        for e in self.subscription.drain_limited(500) {
            let v = match &e.payload {
                tool_core::Payload::Json(j) => gauge_value_from_json(j),
                tool_core::Payload::Text(t) => gauge_value_from_text(t),
                _ => None,
            };
            if let Some(v) = v {
                self.value = v;
                self.samples += 1;
            }
        }
    }
    pub fn zone_kind(&self) -> ZoneKind {
        for z in &self.zones {
            if self.value >= z.from && self.value <= z.to {
                return z.kind;
            }
        }
        if let (Some(f), Some(l)) = (self.zones.first(), self.zones.last()) {
            if self.value < f.from {
                return f.kind;
            }
            if self.value > l.to {
                return l.kind;
            }
        }
        ZoneKind::None
    }
    pub fn status_text(&self) -> String {
        if !self.status.is_empty() {
            return self.status.clone();
        }
        match self.zone_kind() {
            ZoneKind::Safe => "正常".to_owned(),
            ZoneKind::Warn => "预警".to_owned(),
            ZoneKind::Danger => "异常".to_owned(),
            ZoneKind::None => "—".to_owned(),
        }
    }
    pub fn clear(&mut self) {
        while self.subscription.try_recv().is_some() {}
        self.value = self.min;
        self.samples = 0;
    }
}
fn gauge_value_from_json(v: &Value) -> Option<f64> {
    v.get("value")
        .and_then(Value::as_f64)
        .or_else(|| v.as_f64())
}
fn gauge_value_from_text(t: &str) -> Option<f64> {
    for part in t.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == "value" {
                if let Ok(f) = v.trim().parse::<f64>() {
                    return Some(f);
                }
            }
        }
    }
    t.trim().parse::<f64>().ok()
}
