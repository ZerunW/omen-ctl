//! Pure curve-editing logic (no GUI), so it can be unit tested.

use omenctl_core::curve::{CurvePoint, LEVEL_MAX, LEVEL_MIN};

/// Editor axis range for temperature (C).
pub const T_MIN: f32 = 20.0;
pub const T_MAX: f32 = 100.0;

/// Move point `i` to (temp, level) while keeping the curve valid:
/// temperatures strictly increasing (>= 1 C apart), levels non-decreasing,
/// everything inside the editor / fan table range. Temperatures snap to 0.5 C.
pub fn drag_point(points: &mut [CurvePoint], i: usize, temp: f32, level: f32) {
    let n = points.len();
    let lo_t = if i > 0 { points[i - 1].temp + 1.0 } else { T_MIN };
    let hi_t = if i + 1 < n { points[i + 1].temp - 1.0 } else { T_MAX };
    let lo_l = if i > 0 { points[i - 1].level } else { LEVEL_MIN };
    let hi_l = if i + 1 < n { points[i + 1].level } else { LEVEL_MAX };
    let t = ((temp * 2.0).round() / 2.0).clamp(lo_t, hi_t.max(lo_t));
    let l = (level.round() as i32).clamp(lo_l as i32, hi_l.max(lo_l) as i32) as u8;
    points[i].temp = t;
    points[i].level = l;
}

#[cfg(test)]
mod tests {
    use super::*;
    use omenctl_core::curve::FanCurve;

    #[test]
    fn drag_keeps_curve_valid() {
        let mut c = FanCurve::bios_default_cpu().validated().unwrap();
        // try to drag point 3 past both neighbours, up and down
        drag_point(&mut c.points, 3, 99.0, 60.0);
        assert!(c.clone().validated().is_ok());
        assert_eq!(c.points[3].temp, c.points[4].temp - 1.0);
        assert_eq!(c.points[3].level, c.points[4].level);
        drag_point(&mut c.points, 3, 0.0, 0.0);
        assert!(c.clone().validated().is_ok());
        assert_eq!(c.points[3].temp, c.points[2].temp + 1.0);
        assert_eq!(c.points[3].level, c.points[2].level);
        // ends stay in range
        drag_point(&mut c.points, 0, -50.0, 0.0);
        assert_eq!((c.points[0].temp, c.points[0].level), (T_MIN, LEVEL_MIN));
        let last = c.points.len() - 1;
        drag_point(&mut c.points, last, 500.0, 99.0);
        assert_eq!((c.points[last].temp, c.points[last].level), (T_MAX, LEVEL_MAX));
        // snapping
        drag_point(&mut c.points, 5, 61.26, 30.4);
        assert_eq!((c.points[5].temp, c.points[5].level), (61.5, 30));
        assert!(c.validated().is_ok());
    }
}
