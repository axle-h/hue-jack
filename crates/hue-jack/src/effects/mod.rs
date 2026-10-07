//! Effects: render per-channel colours from audio features and channel positions.

pub mod chase;
pub mod palette;
pub mod patterns;
pub mod pulse;
pub mod smoothing;
pub mod spectrum;

use crate::hue::Channel;
pub use palette::{PALETTES, Palette, palette};

/// Effect colour, each component 0..1 (perceptual). The bridge maps RGB straight onto the Hue
/// brightness scale, which is already perceptual, so no gamma is applied on output.
pub type Rgb = [f32; 3];

pub const EFFECTS: [&str; 3] = ["pulse", "spectrum", "chase"];

/// The audio features for one 20 ms light frame (two analysis frames merged: levels from the
/// latest, onsets the max of both so none are lost).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub t: f64,
    pub bands: [f32; 5],
    pub onset: f32,
    pub bass_onset: f32,
    pub bpm: Option<f32>,
    pub rms_db: f32,
    pub silent: bool,
}

/// Per-frame context shared by effects.
pub struct RenderCtx<'a> {
    pub channels: &'a [Channel],
    /// Indices into `channels`, left to right.
    pub order: &'a [usize],
    pub palette: &'a Palette,
    /// Palette rotation (in palette entries), advanced by the beat.
    pub palette_offset: f32,
    pub intensity: f32,
    /// Seconds per frame.
    pub dt: f32,
}

pub trait Effect: Send {
    fn name(&self) -> &'static str;
    /// Writes one colour per channel into `out` (indexed like `ctx.channels`).
    fn render(&mut self, f: &Frame, ctx: &RenderCtx, out: &mut [Rgb]);
}

pub fn create(name: &str) -> Option<Box<dyn Effect>> {
    Some(match name {
        "pulse" => Box::<pulse::Pulse>::default(),
        "spectrum" => Box::<spectrum::Spectrum>::default(),
        "chase" => Box::<chase::Chase>::default(),
        _ => return None,
    })
}

/// Converts an effect colour to bridge RGB16 with the global brightness cap.
pub fn to_u16(rgb: Rgb, brightness_max: f32) -> [u16; 3] {
    let cap = brightness_max.clamp(0.0, 1.0);
    rgb.map(|c| (c.clamp(0.0, 1.0) * cap * 65535.0).round() as u16)
}

/// Converts an effect colour to 8-bit for on-screen previews (screens apply their own gamma).
pub fn to_u8(rgb: Rgb, brightness_max: f32) -> [u8; 3] {
    let cap = brightness_max.clamp(0.0, 1.0);
    rgb.map(|c| (c.clamp(0.0, 1.0) * cap * 255.0).round() as u8)
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

/// Channel indices ordered by x position (left to right), ties broken by id.
pub fn x_order(channels: &[Channel]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..channels.len()).collect();
    idx.sort_by(|&a, &b| {
        channels[a]
            .x
            .total_cmp(&channels[b].x)
            .then(channels[a].id.cmp(&channels[b].id))
    });
    idx
}

/// A plausible 6-light living-room layout, used without a bridge (`--virtual`, `simulate`).
pub fn fake6() -> Vec<Channel> {
    [
        (-0.8, 0.8, 0.0),
        (0.0, 1.0, 0.4),
        (0.8, 0.8, 0.0),
        (-0.8, -0.6, 0.0),
        (0.0, -0.8, 0.4),
        (0.8, -0.6, 0.0),
    ]
    .iter()
    .enumerate()
    .map(|(i, &(x, y, z))| Channel {
        id: i as u8,
        x,
        y,
        z,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_conversion() {
        assert_eq!(to_u16([1.0, 0.0, 0.5], 1.0), [65535, 0, 32768]);
        assert_eq!(to_u16([1.0, 1.0, 1.0], 0.5), [32768, 32768, 32768]);
        assert_eq!(to_u16([2.0, -1.0, 0.0], 1.0), [65535, 0, 0]);
        assert_eq!(to_u8([1.0, 0.5, 0.0], 1.0), [255, 128, 0]);
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]);
        let g = hsv(1.0 / 3.0, 1.0, 1.0);
        assert!(g[0] < 1e-5 && (g[1] - 1.0).abs() < 1e-5 && g[2] < 1e-5);
    }

    #[test]
    fn all_effects_exist() {
        for name in EFFECTS {
            assert_eq!(create(name).unwrap().name(), name);
        }
        assert!(create("nope").is_none());
    }
}
