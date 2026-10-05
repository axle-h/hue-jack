//! Effects: render per-channel colours from audio features and channel positions.

pub mod patterns;

/// Linear-ish colour, each component 0..1 (perceptual brightness; gamma is applied on output).
pub type Rgb = [f32; 3];

/// Output gamma applied when converting effect colours to bridge values.
pub const GAMMA: f32 = 2.2;

/// Converts an effect colour to bridge RGB16: gamma, then the global brightness cap.
pub fn to_u16(rgb: Rgb, brightness_max: f32) -> [u16; 3] {
    let cap = brightness_max.clamp(0.0, 1.0);
    rgb.map(|c| (c.clamp(0.0, 1.0).powf(GAMMA) * cap * 65535.0).round() as u16)
}

/// HSV (h in turns 0..1, s and v 0..1) to RGB.
pub fn hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor() as i32;
    let f = h - i as f32;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Channel ids ordered by x position (left to right), ties broken by id.
pub fn x_order(channels: &[crate::hue::Channel]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..channels.len()).collect();
    idx.sort_by(|&a, &b| {
        channels[a]
            .x
            .total_cmp(&channels[b].x)
            .then(channels[a].id.cmp(&channels[b].id))
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_conversion() {
        assert_eq!(to_u16([1.0, 0.0, 0.5], 1.0), [65535, 0, 14263]);
        assert_eq!(to_u16([1.0, 1.0, 1.0], 0.5), [32768, 32768, 32768]);
        assert_eq!(to_u16([2.0, -1.0, 0.0], 1.0), [65535, 0, 0]);
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]);
        let g = hsv(1.0 / 3.0, 1.0, 1.0);
        assert!(g[0] < 1e-5 && (g[1] - 1.0).abs() < 1e-5 && g[2] < 1e-5);
    }
}
