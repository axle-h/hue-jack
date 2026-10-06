//! `pulse`: all lights breathe with the bass; a bass onset flashes to the next palette colour,
//! decaying over ~150 ms.

use super::{Effect, Frame, RenderCtx, Rgb};

const FLASH_DECAY_SECS: f32 = 0.15;

#[derive(Default)]
pub struct Pulse {
    colour: f32,
    flash: f32,
}

impl Effect for Pulse {
    fn name(&self) -> &'static str {
        "pulse"
    }

    fn render(&mut self, f: &Frame, ctx: &RenderCtx, out: &mut [Rgb]) {
        self.flash *= (-ctx.dt / FLASH_DECAY_SECS).exp();
        if f.bass_onset > 0.0 {
            self.colour += 1.0;
            self.flash = (0.5 + 0.5 * f.bass_onset).max(self.flash);
        }
        let bass = f.bands[0].max(f.bands[1]);
        let breathe = 0.06 + 0.6 * ctx.intensity * bass * bass;
        let level = breathe
            .max(self.flash * (0.4 + 0.6 * ctx.intensity))
            .min(1.0);
        let n = ctx.order.len().max(1) as f32;
        for (rank, &i) in ctx.order.iter().enumerate() {
            // A slight spread across the room keeps neighbouring lights from being identical.
            let c = ctx
                .palette
                .at(self.colour + ctx.palette_offset + 0.3 * rank as f32 / n);
            out[i] = c.map(|v| v * level);
        }
    }
}
