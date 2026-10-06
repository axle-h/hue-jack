//! Synthetic test audio (`gen-test-audio`): clicks, drums with ground truth, a sweep, and a silence gap.
//! Deterministic: the same output every run.

use std::f32::consts::TAU;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use super::SAMPLE_RATE;
use super::wav::write_wav;

const SR: f32 = SAMPLE_RATE as f32;

/// Deterministic white noise in -1..1.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1 << 23) as f32 - 1.0
    }
}

fn add(buf: &mut [f32], at: f64, sound: &[f32]) {
    let start = (at * SAMPLE_RATE as f64).round() as usize;
    // 10 ms fade-out so a truncated decay doesn't add a click of its own.
    let fade = (SR * 0.01) as usize;
    let n = sound.len();
    for (i, s) in sound.iter().enumerate() {
        let s = if i + fade > n {
            s * (n - i) as f32 / fade as f32
        } else {
            *s
        };
        if let Some(b) = buf.get_mut(start + i) {
            *b += s;
        }
    }
}

fn click(noise: &mut Noise) -> Vec<f32> {
    (0..(0.005 * SR) as usize)
        .map(|i| 0.8 * noise.next() * (-(i as f32) / SR / 0.001).exp())
        .collect()
}

fn kick() -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..(0.35 * SR) as usize)
        .map(|i| {
            let t = i as f32 / SR;
            let f = 50.0 + 100.0 * (-t / 0.03).exp();
            phase += TAU * f / SR;
            0.9 * phase.sin() * (-t / 0.18).exp()
        })
        .collect()
}

fn snare(noise: &mut Noise) -> Vec<f32> {
    (0..(0.2 * SR) as usize)
        .map(|i| {
            let t = i as f32 / SR;
            0.35 * noise.next() * (-t / 0.08).exp()
                + 0.25 * (TAU * 330.0 * t).sin() * (-t / 0.06).exp()
        })
        .collect()
}

fn hat(noise: &mut Noise) -> Vec<f32> {
    let mut prev = 0.0;
    (0..(0.06 * SR) as usize)
        .map(|i| {
            let x = noise.next();
            let hp = x - prev;
            prev = x;
            0.12 * hp * (-(i as f32) / SR / 0.02).exp()
        })
        .collect()
}

/// Ground truth for [`drums`]: onset times in seconds.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DrumTruth {
    pub bpm: f64,
    pub kicks: Vec<f64>,
    pub snares: Vec<f64>,
    pub hats: Vec<f64>,
}

/// Clicks at `bpm` from 0.25 s for `secs`; mono. Returns the samples and the click times.
pub fn clicks(secs: f64, bpm: f64) -> (Vec<f32>, Vec<f64>) {
    let mut buf = vec![0.0; (secs * SAMPLE_RATE as f64) as usize];
    let mut noise = Noise(7);
    let mut times = Vec::new();
    let mut t = 0.25;
    while t < secs - 0.05 {
        add(&mut buf, t, &click(&mut noise));
        times.push(t);
        t += 60.0 / bpm;
    }
    (buf, times)
}

/// Kick on every beat, snare on 2 and 4, closed hats on the off-beats, and a sine bass line; mono.
pub fn drums(secs: f64, bpm: f64) -> (Vec<f32>, DrumTruth) {
    let n = (secs * SAMPLE_RATE as f64) as usize;
    let mut buf = vec![0.0; n];
    let mut noise = Noise(42);
    let mut truth = DrumTruth {
        bpm,
        ..Default::default()
    };
    let beat = 60.0 / bpm;
    let kick = kick();
    let t0 = 0.25;
    let mut i = 0;
    loop {
        let t = t0 + i as f64 * beat;
        if t > secs - 0.05 {
            break;
        }
        add(&mut buf, t, &kick);
        truth.kicks.push(t);
        if i % 2 == 1 {
            add(&mut buf, t, &snare(&mut noise));
            truth.snares.push(t);
        }
        let off = t + beat / 2.0;
        if off < secs - 0.05 {
            add(&mut buf, off, &hat(&mut noise));
            truth.hats.push(off);
        }
        i += 1;
    }
    // Bass line: one note per bar, retriggered on the bar's first kick.
    let notes = [55.0f32, 55.0, 65.41, 49.0];
    let bar = 4.0 * beat;
    for (k, b) in buf.iter_mut().enumerate() {
        let t = k as f64 / SAMPLE_RATE as f64;
        if t < t0 {
            continue;
        }
        let bar_idx = ((t - t0) / bar) as usize;
        let in_bar = ((t - t0) - bar_idx as f64 * bar) as f32;
        let env = (in_bar / 0.02).min(1.0) * ((bar as f32 - in_bar) / 0.05).min(1.0);
        *b += 0.2 * env * (TAU * notes[bar_idx % notes.len()] * t as f32).sin();
    }
    let peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.9 {
        buf.iter_mut().for_each(|s| *s *= 0.9 / peak);
    }
    (buf, truth)
}

/// Logarithmic sine sweep from 20 Hz to 16 kHz over `secs`; mono.
pub fn sweep(secs: f64) -> Vec<f32> {
    let (f0, f1) = (20.0f64, 16_000.0f64);
    let k = (f1 / f0).ln();
    let n = (secs * SAMPLE_RATE as f64) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / SAMPLE_RATE as f64;
            let phase = std::f64::consts::TAU * f0 * secs / k * ((t / secs * k).exp() - 1.0);
            let fade = (t / 0.01).min((secs - t) / 0.01).min(1.0);
            (0.5 * fade * phase.sin()) as f32
        })
        .collect()
}

/// `drums` (128 BPM) for `a` s, silence for `gap` s, drums for `b` s; mono. Returns samples and the
/// silent interval (the second segment's first sound is 0.25 s into it).
pub fn silence_gap(a: f64, gap: f64, b: f64) -> (Vec<f32>, (f64, f64)) {
    let (mut buf, _) = drums(a, 128.0);
    buf.resize(((a + gap) * SAMPLE_RATE as f64) as usize, 0.0);
    buf.extend(drums(b, 128.0).0);
    (buf, (a, a + gap + 0.25))
}

fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&s| [s, s]).collect()
}

/// Writes the four test files (and their ground truth JSON) into `dir`.
pub fn write_all(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (c, click_times) = clicks(30.0, 120.0);
    write_wav(&dir.join("clicks_120.wav"), 2, &stereo(&c))?;
    std::fs::write(
        dir.join("clicks_120.onsets.json"),
        serde_json::to_vec_pretty(&serde_json::json!({ "bpm": 120, "clicks": click_times }))?,
    )?;
    let (d, truth) = drums(60.0, 128.0);
    write_wav(&dir.join("drums_128.wav"), 2, &stereo(&d))?;
    std::fs::write(
        dir.join("drums_128.onsets.json"),
        serde_json::to_vec_pretty(&truth)?,
    )?;
    write_wav(&dir.join("sweep.wav"), 2, &stereo(&sweep(20.0)))?;
    let (g, (gap_start, gap_end)) = silence_gap(10.0, 30.0, 10.0);
    write_wav(&dir.join("silence_gap.wav"), 2, &stereo(&g))?;
    std::fs::write(
        dir.join("silence_gap.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({ "gap_start": gap_start, "gap_end": gap_end }),
        )?,
    )?;
    for f in [
        "clicks_120.wav",
        "drums_128.wav",
        "sweep.wav",
        "silence_gap.wav",
    ] {
        println!("{}", dir.join(f).display());
    }
    Ok(())
}
