//! `chase`: each onset moves a highlight to the next light in left-to-right order; the tail fades.

use super::{Effect, Frame, RenderCtx, Rgb};

const TAIL_SECS: f32 = 0.3;

#[derive(Default)]
pub struct Chase {
    head: usize,
    steps: f32,
    levels: Vec<f32>,
    colours: Vec<f32>,
}

impl Effect for Chase {
    fn name(&self) -> &'static str {
        "chase"
    }

    fn render(&mut self, f: &Frame, ctx: &RenderCtx, out: &mut [Rgb]) {
        let n = ctx.order.len();
        if self.levels.len() != n {
            self.levels = vec![0.0; n];
            self.colours = vec![0.0; n];
            self.head = 0;
        }
        if n == 0 {
            return;
        }
        let decay = (-ctx.dt / TAIL_SECS).exp();
        self.levels.iter_mut().for_each(|l| *l *= decay);
        if f.onset > 0.0 {
            self.head = (self.head + 1) % n;
            self.steps += 1.0;
            self.levels[self.head] = (0.5 + 0.5 * f.onset).max(self.levels[self.head]);
            self.colours[self.head] = self.steps * 0.5;
        }
        let glow = 0.04 + 0.1 * ctx.intensity * f.bands.iter().sum::<f32>() / 5.0;
        for (rank, &i) in ctx.order.iter().enumerate() {
            let level = (glow + self.levels[rank] * (0.3 + 0.7 * ctx.intensity)).min(1.0);
            let c = ctx.palette.at(self.colours[rank] + ctx.palette_offset);
            out[i] = c.map(|v| v * level);
        }
    }
}
