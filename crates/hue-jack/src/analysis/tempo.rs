//! Tempo for effect pacing: autocorrelation of the onset envelope over 6 s, 70–180 BPM, scored
//! with a comb over multiples of each candidate period and refined on a fine lag grid.

use std::collections::VecDeque;

use super::FPS;

const WINDOW: usize = 6 * FPS;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;
/// Re-estimate every this many frames.
const EVERY: usize = 25;
const GRID: f32 = 0.02;

pub struct Tempo {
    env: VecDeque<f32>,
    acf: Vec<f32>,
    frames: usize,
    bpm: Option<f32>,
}

impl Default for Tempo {
    fn default() -> Self {
        Self::new()
    }
}

impl Tempo {
    pub fn new() -> Self {
        Self {
            env: VecDeque::with_capacity(WINDOW),
            acf: vec![0.0; WINDOW],
            frames: 0,
            bpm: None,
        }
    }

    pub fn bpm(&self) -> Option<f32> {
        self.bpm
    }

    /// Pushes one onset-envelope value; returns the current estimate.
    pub fn push(&mut self, value: f32) -> Option<f32> {
        if self.env.len() == WINDOW {
            self.env.pop_front();
        }
        self.env.push_back(value);
        self.frames += 1;
        if self.frames.is_multiple_of(EVERY) && self.env.len() >= 3 * FPS {
            self.bpm = self.estimate();
        }
        self.bpm
    }

    /// Forgets the tempo (after silence).
    pub fn reset(&mut self) {
        self.env.clear();
        self.bpm = None;
    }

    fn estimate(&mut self) -> Option<f32> {
        let n = self.env.len();
        let mean = self.env.iter().sum::<f32>() / n as f32;
        // Smooth with a 5-tap triangle so peaks at fractional lags don't split across two bins.
        let raw: Vec<f32> = self.env.iter().map(|v| v - mean).collect();
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let at = |j: isize| raw.get((i as isize + j) as usize).copied().unwrap_or(0.0);
                (at(-2) + 2.0 * at(-1) + 3.0 * at(0) + 2.0 * at(1) + at(2)) / 9.0
            })
            .collect();
        let energy: f32 = x.iter().map(|v| v * v).sum();
        if energy <= 1e-9 {
            return None;
        }
        let max_lag = n - FPS; // keep at least 1 s of overlap
        for lag in 0..max_lag {
            let s: f32 = x[..n - lag].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum();
            self.acf[lag] = s / (n - lag) as f32;
        }
        let acf = |l: f32| -> f32 {
            let i = l as usize;
            if i + 1 >= max_lag {
                return 0.0;
            }
            let f = l - i as f32;
            self.acf[i] * (1.0 - f) + self.acf[i + 1] * f
        };
        let score = |lag: f32| -> f32 {
            let mut sum = 0.0;
            let mut k = 1.0;
            while k * lag < (max_lag - 1) as f32 && k <= 4.0 {
                sum += acf(k * lag);
                k += 1.0;
            }
            if k > 1.0 { sum / (k - 1.0) } else { 0.0 }
        };
        let lo = 60.0 * FPS as f32 / MAX_BPM;
        let hi = 60.0 * FPS as f32 / MIN_BPM;
        let mut best = (0.0f32, f32::MIN);
        let mut lag = lo;
        while lag <= hi {
            let s = score(lag);
            if s > best.1 {
                best = (lag, s);
            }
            lag += GRID;
        }
        if best.1 <= 0.0 {
            return None;
        }
        // A period's multiples also fit its half-tempo, so prefer the faster tempo when it fits
        // nearly as well (tempo only paces effects; double-time is the safer error).
        let half = best.0 / 2.0;
        if half >= lo {
            let mut refined = (half, score(half));
            let mut l = half - 0.5;
            while l <= half + 0.5 {
                let s = score(l);
                if s > refined.1 {
                    refined = (l, s);
                }
                l += GRID;
            }
            if refined.1 >= 0.85 * best.1 {
                best = refined;
            }
        }
        Some(60.0 * FPS as f32 / best.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_train_tempo() {
        for bpm in [72.0f32, 120.0, 128.0, 174.0] {
            let mut t = Tempo::new();
            let period = 60.0 * FPS as f32 / bpm;
            let mut est = None;
            for i in 0..800 {
                let phase = (i as f32 / period).fract();
                est = t.push(if phase < 1.0 / period { 1.0 } else { 0.0 });
            }
            let est = est.unwrap();
            assert!((est - bpm).abs() < 1.0, "{bpm}: {est}");
        }
    }
}
