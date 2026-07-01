use crate::theme;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::Value;
use tool_core::{Event, Payload, topics};
use std::sync::Arc;
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
        // 零 clone 批量消费：取 Arc<Event> 引用。
        for arc in self.subscription.drain_limited_arc(500) {
            self.push_event(&arc);
        }
    }

    fn push_event(&mut self, event: &Arc<Event>) {
        match &event.payload {
            Payload::Json(value) => {
                if let Some((roll, pitch, yaw)) = attitude_from_json(value) {
                    self.roll = roll;
                    self.pitch = pitch;
                    self.yaw = yaw;
                    self.samples += 1;
                    self.last_source = event.source.clone();
                }
            }
            Payload::Text(text) => {
                if let Some((roll, pitch, yaw)) = attitude_from_text(text) {
                    self.roll = roll;
                    self.pitch = pitch;
                    self.yaw = yaw;
                    self.samples += 1;
                    self.last_source = event.source.clone();
                }
            }
            Payload::Bytes(_) | Payload::Empty => {}
        }
    }

    pub fn clear(&mut self) {
        while self.subscription.try_recv_arc().is_some() {}
        self.roll = 0.0;
        self.pitch = 0.0;
        self.yaw = 0.0;
        self.samples = 0;
    }

    pub fn ingest_all_pending(&mut self) -> usize {
        let mut count = 0;
        for arc in self.subscription.drain_arc() {
            self.push_event(&arc);
            count += 1;
        }
        count
    }

    fn paint(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        // 无边框嵌入：不画背景填充与外边框，直接在面板背景上绘制参考圆与机体

        let center = rect.center();
        let radius = rect.width().min(rect.height()) * 0.34;

        // 画参考圆
        painter.circle_stroke(center, radius, Stroke::new(1.0, theme::BORDER_LIGHT));

        // 画机体（填充面 + 边框，画家算法深度排序）
        draw_body(&painter, center, radius, self.roll, self.pitch, self.yaw);

        // 画坐标轴（按深度排序，远的先画）
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

        // 按深度排序（y 分量 = 深度方向），远的先画
        let mut sorted_axes: Vec<_> = axes.to_vec();
        sorted_axes.sort_by(|a, b| {
            a.0[1]
                .partial_cmp(&b.0[1])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for (axis, color, label) in sorted_axes {
            let (end, _depth) = project(center, radius, axis);
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
    }
}

/// 机体面定义：6 个面，每个面 4 个顶点索引
const BODY_FACES: [[usize; 4]; 6] = [
    [0, 1, 2, 3], // 底面 (z = -0.16)
    [4, 5, 6, 7], // 顶面 (z = 0.16)
    [0, 1, 5, 4], // 前面 (y = -0.3)
    [2, 3, 7, 6], // 后面 (y = 0.3)
    [0, 3, 7, 4], // 左面 (x = -0.55)
    [1, 2, 6, 5], // 右面 (x = 0.55)
];

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

    // 旋转到世界坐标（保留 3D 用于法线计算），再投影到屏幕
    let rotated: Vec<[f64; 3]> = points
        .iter()
        .map(|&p| rotate(p, roll, pitch, yaw))
        .collect();
    let projected: Vec<(Pos2, f32)> = rotated
        .iter()
        .map(|&p| project(center, radius, p))
        .collect();

    // 每个面的法线（世界坐标叉积），取其 Y 分量判断朝向：观察者沿 +Y 看，Y>0 为朝前。
    let face_facing: Vec<f32> = BODY_FACES
        .iter()
        .map(|face| {
            let a = rotated[face[0]];
            let b = rotated[face[1]];
            let c = rotated[face[2]];
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            cross(ab, ac)[1] as f32
        })
        .collect();

    // 按平均深度排序（远的先画）
    let mut faces: Vec<(usize, f32)> = BODY_FACES
        .iter()
        .enumerate()
        .map(|(i, face)| {
            let avg_depth = face.iter().map(|&idx| projected[idx].1).sum::<f32>() / 4.0;
            (i, avg_depth)
        })
        .collect();
    faces.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // 画家算法：填充用三角扇 Mesh，绕开 egui closed path 的 miter/feather（避免退化面尖角爆炸）
    for (face_idx, depth) in &faces {
        let face = BODY_FACES[*face_idx];
        let raw_points: Vec<Pos2> = face.iter().map(|&idx| projected[idx].0).collect();

        // 严格清洗：过滤非有限点、去相邻重合点、去近共线中间点；
        // 并用“相邻边叉积（=三角形面积两倍）”判定局部退化——这正是 miter 发散的充要条件。
        let Some(clean) = clean_face(&raw_points) else {
            // 退化面：交给外轮廓边逻辑绘制，不填充
            continue;
        };

        let brightness = ((depth + 1.0) / 2.0).clamp(0.0, 1.0);
        let face_color = Color32::from_rgb(
            (theme::ATTITUDE_BODY.r() as f32 * (0.3 + 0.7 * brightness)) as u8,
            (theme::ATTITUDE_BODY.g() as f32 * (0.3 + 0.7 * brightness)) as u8,
            (theme::ATTITUDE_BODY.b() as f32 * (0.3 + 0.7 * brightness)) as u8,
        );

        painter.add(fill_triangle_fan(&clean, face_color));
    }

    // 画机体外轮廓：一条边相邻两面若一面朝前、一面朝后，则为轮廓边，只画一次。
    let edge_stroke = Stroke::new(1.5, theme::ATTITUDE_BODY_EDGE);
    for (a, b, f1, f2) in body_edges() {
        let facing_diff = face_facing[f1] * face_facing[f2];
        if facing_diff <= 0.0 {
            painter.line_segment([projected[a].0, projected[b].0], edge_stroke);
        }
    }
}

/// 返回机体的所有无向边及其两个相邻面索引 (a, b, face1, face2)。
fn body_edges() -> Vec<(usize, usize, usize, usize)> {
    use std::collections::HashMap;
    let mut edge_faces: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (fi, face) in BODY_FACES.iter().enumerate() {
        for i in 0..4 {
            let a = face[i];
            let b = face[(i + 1) % 4];
            let key = if a < b { (a, b) } else { (b, a) };
            edge_faces.entry(key).or_default().push(fi);
        }
    }
    edge_faces
        .into_iter()
        .filter_map(|((a, b), fs)| {
            if fs.len() == 2 {
                Some((a, b, fs[0], fs[1]))
            } else {
                None
            }
        })
        .collect()
}

/// 三维向量叉积
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// 多边形有向面积（屏幕坐标，结果绝对值即面积）
fn polygon_area_2d(points: &[Pos2]) -> f32 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        s += a.x * b.y - b.x * a.y;
    }
    s * 0.5
}

/// 清洗一个面的投影顶点，返回稳定的多边形；退化时返回 None。
///
/// 清洗步骤：
/// 1. 过滤非有限坐标；
/// 2. 去除相邻重合点（距离 < `DEDUP_EPS`）；
/// 3. 迭代去除“近共线中间点”——某顶点前后两条边的叉积绝对值（=该处三角形面积两倍）
///    小于 `MIN_TURN_AREA` 时，说明该顶点处近平 180°，egui miter join 会发散，去掉该点；
/// 4. 剩余点 < 3 或总面积过小 → 视为退化。
///
/// 这里用“局部转角”而非“总面积”判定，因为长瘦面面积可能过阈值，但局部仍会炸。
fn clean_face(points: &[Pos2]) -> Option<Vec<Pos2>> {
    const DEDUP_EPS: f32 = 0.5;
    /// 相邻边叉积绝对值下界（平方像素量级）。低于此值视为该顶点近平共线。
    /// 1.0 对应一个底~1px、高~2px 的三角形，足够小但能挡住退化顶点。
    const MIN_TURN_AREA: f32 = 1.0;
    /// 整面面积下界，防止极小残留面
    const MIN_FACE_AREA: f32 = 1.0;

    // 1. 有限性 + 去重
    let mut pts: Vec<Pos2> = Vec::with_capacity(points.len());
    for &p in points {
        if !p.x.is_finite() || !p.y.is_finite() {
            continue;
        }
        match pts.last() {
            Some(&q) if q.distance(p) < DEDUP_EPS => continue,
            _ => pts.push(p),
        }
    }
    // 首尾重合也去
    if pts.len() > 1
        && pts
            .first()
            .zip(pts.last())
            .map(|(&a, &b)| a.distance(b) < DEDUP_EPS)
            .unwrap_or(false)
    {
        pts.pop();
    }
    if pts.len() < 3 {
        return None;
    }

    // 3. 迭代去近共线中间点
    loop {
        let n = pts.len();
        if n < 3 {
            return None;
        }
        let mut to_remove: Option<usize> = None;
        for i in 0..n {
            let prev = pts[(i + n - 1) % n];
            let cur = pts[i];
            let next = pts[(i + 1) % n];
            // 叉积 = |prev->cur × cur->next| = 该顶点处三角形面积的两倍
            let cross = (cur.x - prev.x) * (next.y - cur.y) - (cur.y - prev.y) * (next.x - cur.x);
            if cross.abs() < MIN_TURN_AREA {
                to_remove = Some(i);
                break;
            }
        }
        match to_remove {
            Some(i) => {
                pts.remove(i);
            }
            None => break,
        }
    }

    if pts.len() < 3 {
        return None;
    }
    // 4. 整体面积下界
    if polygon_area_2d(&pts).abs() < MIN_FACE_AREA {
        return None;
    }
    Some(pts)
}

/// 用三角扇填充凸多边形：以首点为中心，向其余相邻点对发射三角形。
/// 直接构造 `epaint::Mesh`，绕开 egui closed path 的 miter/feather 网格化，
/// 退化输入由调用方 `clean_face` 保证已剔除。
fn fill_triangle_fan(points: &[Pos2], color: Color32) -> egui::epaint::Mesh {
    let mut mesh = egui::epaint::Mesh::default();
    let center = points[0];
    mesh.colored_vertex(center, color);
    for p in &points[1..] {
        mesh.colored_vertex(*p, color);
    }
    // 中心 = index 0，其余点 1..n。三角形 (0, i, i+1)
    let n = points.len() as u32;
    for i in 1..n.saturating_sub(1) {
        mesh.add_triangle(0, i, i + 1);
    }
    mesh
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

/// Roll-Pitch-Yaw 旋转（内旋 ZYX = 外旋 XYZ）
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

/// 斜投影：返回 (屏幕坐标, Y深度分量)
fn project(center: Pos2, radius: f32, point: [f64; 3]) -> (Pos2, f32) {
    let x = (point[0] - point[1] * 0.28) as f32;
    let y = (-point[2] + point[1] * 0.28) as f32;
    let depth = point[1] as f32; // Y 分量作为深度（正值 = 远离观察者）
    (
        Pos2::new(center.x + x * radius, center.y + y * radius),
        depth,
    )
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

    #[test]
    fn body_edges_covers_all_cuboid_edges() {
        // 立方体有 12 条棱，每条棱恰好两个相邻面
        let edges = body_edges();
        assert_eq!(edges.len(), 12, "cuboid should have 12 edges");
        for (_, _, f1, f2) in &edges {
            assert_ne!(f1, f2);
        }
    }

    #[test]
    fn cross_product_basic() {
        let z = cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert!((z[0]).abs() < 1e-9 && (z[1]).abs() < 1e-9 && (z[2] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn polygon_area_degenerate_is_zero() {
        // 共线四点面积为 0
        let p = [
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(2.0, 0.0),
            Pos2::new(3.0, 0.0),
        ];
        assert!(polygon_area_2d(&p).abs() < 1e-6);
        // 正方形面积 4
        let q = [
            Pos2::new(0.0, 0.0),
            Pos2::new(2.0, 0.0),
            Pos2::new(2.0, 2.0),
            Pos2::new(0.0, 2.0),
        ];
        assert!((polygon_area_2d(&q).abs() - 4.0).abs() < 1e-6);
    }

    #[test]
    fn clean_face_rejects_degenerate_collinear() {
        // 四点共线 -> 退化
        let p = [
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(2.0, 0.0),
            Pos2::new(3.0, 0.0),
        ];
        assert!(clean_face(&p).is_none());
    }

    #[test]
    fn clean_face_rejects_coincident_neighbors() {
        // 含重合相邻点，退化到不足 3 个有效点
        let p = [
            Pos2::new(0.0, 0.0),
            Pos2::new(0.1, 0.0),
            Pos2::new(0.2, 0.0),
            Pos2::new(0.3, 0.0),
        ];
        assert!(clean_face(&p).is_none());
    }

    #[test]
    fn clean_face_keeps_stable_quadrilateral() {
        // 正常四边形应保留 4 个点
        let p = [
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(0.0, 10.0),
        ];
        let clean = clean_face(&p).expect("stable quad should not be rejected");
        assert_eq!(clean.len(), 4);
    }

    #[test]
    fn clean_face_removes_near_collinear_vertex() {
        // 一个明显共线的中间点应被移除，剩余三点仍能成面
        let p = [
            Pos2::new(0.0, 0.0),
            Pos2::new(5.0, 0.0), // 近共线中间点（与前后几乎在一条线上）
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
        ];
        let clean = clean_face(&p).expect("should reduce to a valid triangle");
        assert!(clean.len() >= 3);
    }

    #[test]
    fn fill_triangle_fan_emits_n_minus_two_triangles() {
        // 四边形三角扇 = 2 个三角形 = 6 个索引
        let p = vec![
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(0.0, 10.0),
        ];
        let mesh = fill_triangle_fan(&p, Color32::RED);
        assert_eq!(mesh.indices.len(), 6);
        assert_eq!(mesh.vertices.len(), 4);
        assert!(mesh.is_valid());
    }
}
