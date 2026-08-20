/// Chart 映射与 Gauge 弧、Attitude 3D 投影的纯数学层
/// 供 Slint Canvas Path 真绘制复用，零依赖 egui，可单测

pub fn map_x(x: f64, x_min: f64, x_max: f64, width: f64) -> f64 {
    if (x_max - x_min).abs() < 1e-9 {
        width * 0.5
    } else {
        (x - x_min) / (x_max - x_min) * width
    }
}
pub fn map_y(y: f64, y_min: f64, y_max: f64, height: f64) -> f64 {
    if (y_max - y_min).abs() < 1e-9 {
        height * 0.5
    } else {
        height - (y - y_min) / (y_max - y_min) * height
    }
}

pub fn gauge_angle(value: f64, min: f64, max: f64) -> f64 {
    const START_DEG: f64 = 135.0;
    const SWEEP_DEG: f64 = 270.0;
    let t = ((value - min) / (max - min + 1e-9)).clamp(0.0, 1.0);
    START_DEG + t * SWEEP_DEG
}
pub fn polar_point(center: (f64, f64), radius: f64, deg: f64) -> (f64, f64) {
    let rad = deg.to_radians();
    (center.0 + radius * rad.cos(), center.1 + radius * rad.sin())
}

/// 3D 旋转：roll X / pitch Y / yaw Z（弧度）
pub fn rotate(mut p: [f64; 3], roll: f64, pitch: f64, yaw: f64) -> [f64; 3] {
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    // Ry * Rx * Rz 顺序近似，满足 attitude 可视即可
    // X roll
    let y1 = p[1] * cr - p[2] * sr;
    let z1 = p[1] * sr + p[2] * cr;
    p[1] = y1;
    p[2] = z1;
    // Y pitch
    let x2 = p[0] * cp + p[2] * sp;
    let z2 = -p[0] * sp + p[2] * cp;
    p[0] = x2;
    p[2] = z2;
    // Z yaw
    let x3 = p[0] * cy - p[1] * sy;
    let y3 = p[0] * sy + p[1] * cy;
    p[0] = x3;
    p[1] = y3;
    p
}

pub fn project(center: (f64, f64), radius: f64, p: [f64; 3]) -> (f64, f64) {
    // 正交投影：xy 直接映射，忽略 z 深度（深度仅作排序）
    (center.0 + p[0] * radius, center.1 - p[1] * radius)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn map_x_center_when_range_zero() {
        assert!((map_x(5.0, 5.0, 5.0, 100.0) - 50.0).abs() < 1e-9);
    }
    #[test]
    fn gauge_angle_mid_is_270() {
        assert!((gauge_angle(50.0, 0.0, 100.0) - 270.0).abs() < 1e-9);
    }
}
