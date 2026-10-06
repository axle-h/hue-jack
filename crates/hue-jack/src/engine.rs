//! The engine: feature frames (100/s) → effect → [`LightFrame`] (50/s).
//!
//! Two analysis frames make one light frame. The palette rotates once every
//! [`BEATS_PER_ROTATION`] beats (tempo paces it; timing comes from the audio). Calibration mode
//! flashes every light white on each click; test patterns override the effect.

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::analysis::FeatureFrame;
use crate::effects::patterns::{self, Pattern};
use crate::effects::smoothing::Smoother;
use crate::effects::{self, Effect, Frame, Palette, RenderCtx, Rgb, x_order};
use crate::hue::Channel;

/// Light frames per second.
pub const LIGHT_FPS: f64 = 50.0;
pub const BEATS_PER_ROTATION: f32 = 8.0;
const CALIBRATION_DECAY_SECS: f32 = 0.1;

/// One frame of light output: per channel, 16-bit RGB as sent to the bridge.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LightFrame {
    pub channels: Vec<(u8, [u16; 3])>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EngineSettings {
    pub effect: String,
    pub palette: String,
    pub intensity: f32,
    pub brightness_max: f32,
}

impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            effect: "pulse".into(),
            palette: "sunset".into(),
            intensity: 0.8,
            brightness_max: 1.0,
        }
    }
}

/// A rendered light frame plus what the UI needs.
#[derive(Clone, Debug)]
pub struct Rendered {
    pub t: f64,
    pub frame: LightFrame,
    /// 8-bit colours per channel (same order as the engine's channels) for previews.
    pub preview: Vec<[u8; 3]>,
    pub features: Frame,
}

pub struct Engine {
    channels: Vec<Channel>,
    order: Vec<usize>,
    effect: Box<dyn Effect>,
    palette: &'static Palette,
    settings: EngineSettings,
    smoother: Smoother,
    beat_phase: f32,
    pending: Option<Frame>,
    count: u64,
    calibration: bool,
    calibration_flash: f32,
    pattern: Option<(Pattern, Option<f64>)>,
    buf: Vec<Rgb>,
}

fn merge(a: &Frame, b: &FeatureFrame) -> Frame {
    Frame {
        t: b.t,
        bands: b.bands,
        onset: a.onset.max(b.onset),
        bass_onset: a.bass_onset.max(b.bass_onset),
        bpm: b.bpm,
        rms_db: a.rms_db.max(b.rms_db),
        silent: b.silent,
    }
}

fn single(b: &FeatureFrame) -> Frame {
    Frame {
        t: b.t,
        bands: b.bands,
        onset: b.onset,
        bass_onset: b.bass_onset,
        bpm: b.bpm,
        rms_db: b.rms_db,
        silent: b.silent,
    }
}

impl Engine {
    pub fn new(channels: Vec<Channel>, settings: &EngineSettings) -> Result<Self> {
        let effect = effects::create(&settings.effect)
            .ok_or_else(|| anyhow!("unknown effect {:?}", settings.effect))?;
        let palette = effects::palette(&settings.palette)
            .ok_or_else(|| anyhow!("unknown palette {:?}", settings.palette))?;
        Ok(Self {
            order: x_order(&channels),
            buf: vec![[0.0; 3]; channels.len()],
            channels,
            effect,
            palette,
            settings: settings.clone(),
            smoother: Smoother::default(),
            beat_phase: 0.0,
            pending: None,
            count: 0,
            calibration: false,
            calibration_flash: 0.0,
            pattern: None,
        })
    }

    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    pub fn set_channels(&mut self, channels: Vec<Channel>) {
        if channels != self.channels {
            self.order = x_order(&channels);
            self.buf = vec![[0.0; 3]; channels.len()];
            self.channels = channels;
            self.smoother.reset();
        }
    }

    pub fn set_settings(&mut self, settings: &EngineSettings) -> Result<()> {
        if settings.effect != self.settings.effect {
            self.effect = effects::create(&settings.effect)
                .ok_or_else(|| anyhow!("unknown effect {:?}", settings.effect))?;
        }
        if settings.palette != self.settings.palette {
            self.palette = effects::palette(&settings.palette)
                .ok_or_else(|| anyhow!("unknown palette {:?}", settings.palette))?;
        }
        self.settings = settings.clone();
        Ok(())
    }

    pub fn settings(&self) -> &EngineSettings {
        &self.settings
    }

    pub fn set_calibration(&mut self, on: bool) {
        self.calibration = on;
    }

    /// Starts (or with `None` stops) a test pattern; it starts at the next light frame.
    pub fn set_pattern(&mut self, pattern: Option<Pattern>) {
        if pattern.is_none() && self.pattern.is_some() {
            self.smoother.reset();
        }
        self.pattern = pattern.map(|p| (p, None));
    }

    /// The smoothed colours of the last frame (before gamma and the brightness cap).
    pub fn smoothed(&self) -> &[Rgb] {
        &self.buf
    }

    /// Feeds one analysis frame; every second one completes a light frame.
    pub fn push(&mut self, f: &FeatureFrame) -> Option<Rendered> {
        self.count += 1;
        match self.pending.take() {
            None => {
                self.pending = Some(single(f));
                None
            }
            Some(prev) => Some(self.render(merge(&prev, f))),
        }
    }

    fn render(&mut self, f: Frame) -> Rendered {
        let dt = (1.0 / LIGHT_FPS) as f32;
        if !f.silent {
            self.beat_phase += f.bpm.unwrap_or(120.0) / 60.0 * dt;
        }
        let ctx = RenderCtx {
            channels: &self.channels,
            order: &self.order,
            palette: self.palette,
            palette_offset: self.beat_phase / BEATS_PER_ROTATION,
            intensity: self.settings.intensity.clamp(0.0, 1.0),
            dt,
        };
        let mut target = vec![[0.0f32; 3]; self.channels.len()];
        let mut smooth = true;
        if let Some((pattern, start)) = &mut self.pattern {
            let start = *start.get_or_insert(f.t);
            target = patterns::render(*pattern, &self.channels, (f.t - start) as f32);
            smooth = false;
        } else if self.calibration {
            self.calibration_flash *= (-dt / CALIBRATION_DECAY_SECS).exp();
            if f.onset > 0.0 {
                self.calibration_flash = 1.0;
            }
            target.fill([self.calibration_flash; 3]);
            // Crisp edges: the flash's onset is what gets lined up with the click by eye.
            smooth = false;
        } else {
            self.effect.render(&f, &ctx, &mut target);
        }
        if smooth {
            self.buf.copy_from_slice(self.smoother.apply(&target, dt));
        } else {
            self.buf.copy_from_slice(&target);
        }
        let cap = self.settings.brightness_max;
        let frame = LightFrame {
            channels: self
                .channels
                .iter()
                .zip(&self.buf)
                .map(|(c, rgb)| (c.id, effects::to_u16(*rgb, cap)))
                .collect(),
        };
        let preview = self
            .buf
            .iter()
            .map(|rgb| effects::to_u8(*rgb, cap))
            .collect();
        Rendered {
            t: f.t,
            frame,
            preview,
            features: f,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze_mono;
    use crate::audio::testgen;
    use crate::effects::{EFFECTS, fake6, smoothing};

    fn run(effect: &str, brightness_max: f32) -> (Vec<Rendered>, Vec<Vec<Rgb>>) {
        let (audio, _) = testgen::drums(8.0, 128.0);
        let frames = analyze_mono(&audio);
        let settings = EngineSettings {
            effect: effect.into(),
            palette: "neon".into(),
            intensity: 1.0,
            brightness_max,
        };
        let mut engine = Engine::new(fake6(), &settings).unwrap();
        let mut out = Vec::new();
        let mut smoothed = Vec::new();
        for f in &frames {
            if let Some(r) = engine.push(f) {
                out.push(r);
                smoothed.push(engine.smoothed().to_vec());
            }
        }
        (out, smoothed)
    }

    #[test]
    fn deterministic() {
        for effect in EFFECTS {
            let (a, _) = run(effect, 1.0);
            let (b, _) = run(effect, 1.0);
            assert_eq!(a.len(), b.len());
            assert!(
                a.iter().zip(&b).all(|(x, y)| x.frame == y.frame),
                "{effect} not deterministic"
            );
            assert!(
                (a.len() as i64 - 400).abs() <= 3,
                "{} frames for 8 s",
                a.len()
            );
        }
    }

    #[test]
    fn effects_react_to_the_music() {
        for effect in EFFECTS {
            let (frames, _) = run(effect, 1.0);
            let brightness: Vec<u32> = frames
                .iter()
                .map(|r| {
                    r.frame
                        .channels
                        .iter()
                        .map(|(_, c)| c.iter().map(|&v| v as u32).sum::<u32>())
                        .sum()
                })
                .collect();
            let (min, max) = (
                brightness.iter().min().unwrap(),
                brightness.iter().max().unwrap(),
            );
            assert!(
                *max > 4 * (*min).max(1),
                "{effect}: brightness range {min}..{max}"
            );
        }
    }

    #[test]
    fn envelope_is_respected() {
        for effect in EFFECTS {
            let (_, smoothed) = run(effect, 1.0);
            for w in smoothed.windows(2) {
                for (a, b) in w[0].iter().zip(&w[1]) {
                    for ch in 0..3 {
                        let d = b[ch] - a[ch];
                        assert!(
                            d <= smoothing::MAX_RISE + 1e-6 && -d <= smoothing::MAX_FALL + 1e-6,
                            "{effect}: step {d}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn brightness_cap_is_respected() {
        for effect in EFFECTS {
            let (frames, _) = run(effect, 0.4);
            let max = frames
                .iter()
                .flat_map(|r| r.frame.channels.iter().flat_map(|(_, c)| *c))
                .max()
                .unwrap();
            assert!(max as f32 <= 0.4 * 65535.0 + 1.0, "{effect}: {max}");
            assert!(max > 0);
        }
    }

    #[test]
    fn calibration_flashes_white_on_clicks() {
        let (audio, _) = testgen::clicks(4.0, 80.0);
        let mut engine = Engine::new(fake6(), &EngineSettings::default()).unwrap();
        engine.set_calibration(true);
        let rendered: Vec<Rendered> = analyze_mono(&audio)
            .iter()
            .filter_map(|f| engine.push(f))
            .collect();
        let lit = rendered
            .iter()
            .filter(|r| {
                r.frame
                    .channels
                    .iter()
                    .all(|(_, c)| c[0] > 60_000 && c[0] == c[1] && c[1] == c[2])
            })
            .count();
        assert!(lit >= 4, "{lit} white frames");
    }

    #[test]
    fn test_pattern_overrides_the_effect() {
        let mut engine = Engine::new(fake6(), &EngineSettings::default()).unwrap();
        engine.set_pattern(Some(Pattern::Identify));
        let f = FeatureFrame::default();
        let r = engine.push(&f).or_else(|| engine.push(&f)).unwrap();
        let lit = r.frame.channels.iter().filter(|(_, c)| c[0] > 0).count();
        assert_eq!(lit, 1);
    }
}
