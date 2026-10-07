//! `spectrum`: lights sorted left to right map to bands from low to high; each light's brightness
//! is its band's energy.

use super::{Effect, Frame, RenderCtx, Rgb};

#[derive(Default)]
pub struct Spectrum;

impl Effect for Spectrum {
    fn name(&self) -> &'static str {
        "spectrum"
    }

    fn render(&mut self, f: &Frame, ctx: &RenderCtx, out: &mut [Rgb]) {
        let n = ctx.order.len();
        for (rank, &i) in ctx.order.iter().enumerate() {
            // Position along the five bands, interpolating when there are more or fewer lights.
            let pos = if n > 1 {
                rank as f32 * 4.0 / (n - 1) as f32
            } else {
                1.0
            };
            let lo = pos.floor() as usize;
            let hi = (lo + 1).min(4);
            let frac = pos - lo as f32;
            let energy = f.bands[lo] * (1.0 - frac) + f.bands[hi] * frac;
            let level = (0.1 + 0.9 * ctx.intensity * energy.powf(0.8)).min(1.0);
            let c = ctx.palette.at(ctx.palette_offset
                + rank as f32 * ctx.palette.colors.len() as f32 / n.max(1) as f32);
            out[i] = c.map(|v| v * level);
        }
    }
}
