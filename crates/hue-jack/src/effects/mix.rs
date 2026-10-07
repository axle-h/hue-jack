//! `cycle` and `random`: play the other effects in turn (`cycle`: in order; `random`: a different one
//! each time), switching on a beat every [`PHRASE_BEATS`] and crossfading over [`CROSSFADE_SECS`].

use super::{Effect, Frame, RenderCtx, Rgb, create};

/// The effects being mixed.
const BASE: [&str; 3] = ["pulse", "spectrum", "chase"];
/// 16 bars of 4/4.
pub const PHRASE_BEATS: f32 = 64.0;
/// Switch anyway if no onset lands within this many beats after the phrase ends.
const ONSET_GRACE_BEATS: f32 = 4.0;
const CROSSFADE_SECS: f32 = 1.5;

pub struct Mix {
    random: bool,
    /// xorshift64 state; 0 until seeded from the first audible frame.
    rng: u64,
    index: usize,
    current: Box<dyn Effect>,
    /// The outgoing effect and how far through the crossfade we are (seconds).
    previous: Option<(Box<dyn Effect>, f32)>,
    beats: f32,
    buf: Vec<Rgb>,
}

impl Mix {
    pub fn new(random: bool) -> Self {
        Self {
            random,
            rng: 0,
            index: 0,
            current: create(BASE[0]).expect("base effect"),
            previous: None,
            beats: 0.0,
            buf: Vec::new(),
        }
    }

    fn next_random(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn switch_to(&mut self, index: usize) {
        let incoming = create(BASE[index]).expect("base effect");
        let outgoing = std::mem::replace(&mut self.current, incoming);
        self.previous = Some((outgoing, 0.0));
        self.index = index;
    }
}

impl Effect for Mix {
    fn name(&self) -> &'static str {
        if self.random { "random" } else { "cycle" }
    }

    fn render(&mut self, f: &Frame, ctx: &RenderCtx, out: &mut [Rgb]) {
        // Seed from the music, so runs on the same input repeat exactly but real sessions differ.
        if self.random && self.rng == 0 && !f.silent {
            self.rng = f
                .bands
                .iter()
                .fold(0x9e37_79b9_7f4a_7c15u64, |h, b| {
                    (h ^ u64::from(b.to_bits())).wrapping_mul(0x0100_0000_01b3)
                })
                .max(1);
            let first = (self.next_random() % BASE.len() as u64) as usize;
            if first != self.index {
                self.current = create(BASE[first]).expect("base effect");
                self.index = first;
            }
        }
        if !f.silent {
            self.beats += f.bpm.unwrap_or(120.0) / 60.0 * ctx.dt;
        }
        let on_beat = f.bass_onset > 0.0 || f.onset > 0.0;
        if self.beats >= PHRASE_BEATS && (on_beat || self.beats >= PHRASE_BEATS + ONSET_GRACE_BEATS)
        {
            let next = if self.random {
                let step = 1 + (self.next_random() % (BASE.len() as u64 - 1)) as usize;
                (self.index + step) % BASE.len()
            } else {
                (self.index + 1) % BASE.len()
            };
            self.switch_to(next);
            // Keep the phrase grid: a switch that waited for an onset doesn't push the next one later.
            self.beats = (self.beats - PHRASE_BEATS).max(0.0);
        }

        self.current.render(f, ctx, out);
        if let Some((outgoing, elapsed)) = &mut self.previous {
            self.buf.resize(out.len(), [0.0; 3]);
            outgoing.render(f, ctx, &mut self.buf);
            let w = (*elapsed / CROSSFADE_SECS).min(1.0);
            for (o, p) in out.iter_mut().zip(&self.buf) {
                for k in 0..3 {
                    o[k] = p[k] + (o[k] - p[k]) * w;
                }
            }
            *elapsed += ctx.dt;
            if *elapsed >= CROSSFADE_SECS {
                self.previous = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{fake6, palette, x_order};

    /// 120 BPM with an onset on every beat; returns the base effect playing at each beat.
    fn effects_per_beat(random: bool, beats: usize) -> Vec<usize> {
        let channels = fake6();
        let order = x_order(&channels);
        let pal = palette("sunset").unwrap();
        let dt = 0.02;
        let ctx = RenderCtx {
            channels: &channels,
            order: &order,
            palette: pal,
            palette_offset: 0.0,
            intensity: 1.0,
            dt,
        };
        let mut mix = Mix::new(random);
        let mut out = vec![[0.0; 3]; channels.len()];
        let mut seen = Vec::new();
        for i in 0..beats * 25 {
            let onset = if i % 25 == 0 { 1.0 } else { 0.0 };
            let f = Frame {
                t: i as f64 * 0.02,
                bands: [0.6, 0.5, 0.4, 0.3, 0.2],
                onset,
                bass_onset: onset,
                bpm: Some(120.0),
                rms_db: -20.0,
                silent: false,
            };
            mix.render(&f, &ctx, &mut out);
            if i % 25 == 0 {
                seen.push(mix.index);
            }
        }
        seen
    }

    #[test]
    fn cycle_steps_through_every_effect_once_per_phrase() {
        let seen = effects_per_beat(false, 64 * 4);
        let changes: Vec<(usize, usize)> = seen
            .windows(2)
            .enumerate()
            .filter(|(_, w)| w[0] != w[1])
            .map(|(beat, w)| (beat + 1, w[1]))
            .collect();
        assert_eq!(
            changes.iter().map(|c| c.1).collect::<Vec<_>>(),
            vec![1, 2, 0]
        );
        for (beat, _) in &changes {
            assert_eq!(beat % 64, 0, "switched on beat {beat}: {changes:?}");
        }
    }

    #[test]
    fn random_never_repeats_and_is_reproducible() {
        let a = effects_per_beat(true, 64 * 8);
        assert_eq!(a, effects_per_beat(true, 64 * 8));
        let phrases: Vec<usize> = a.chunks(64).map(|c| c[1]).collect();
        assert!(phrases.windows(2).all(|w| w[0] != w[1]), "{phrases:?}");
    }
}
