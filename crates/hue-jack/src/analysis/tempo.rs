//! Tempo for effect pacing: autocorrelation of the onset envelope over 6 s, 70–180 BPM, scored
//! with a comb over multiples of each candidate period and refined on a fine lag grid.
//!
//! The estimate is held through breakdowns and octave ambiguity: while the held period still
//! fits the window nearly as well as the best candidate it stays (following small drifts), a
//! weakly periodic window changes nothing, and a different tempo has to win for
//! [`SWITCH_AFTER`] estimates in a row before it replaces the held one.

use std::collections::VecDeque;

use super::FPS;

const WINDOW: usize = 6 * FPS;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;
/// Re-estimate every this many frames.
const EVERY: usize = 25;
const GRID: f32 = 0.02;
/// Minimum comb score, relative to the window's variance, for an estimate to count.
const MIN_CONFIDENCE: f32 = 0.15;
/// The held period stays while its score is at least this fraction of the best candidate's.
const HOLD_RATIO: f32 = 0.7;
/// Relative lag range searched around the held period, so a slowly changing tempo is followed.
const FOLLOW: f32 = 0.03;
/// Consecutive agreeing estimates (≈ 3 s) needed to switch to a different tempo.
const SWITCH_AFTER: usize = 12;
/// Two estimates within this relative distance count as the same tempo.
const SAME: f32 = 0.04;

pub struct Tempo {
    env: VecDeque<f32>,
    acf: Vec<f32>,
    max_lag: usize,
    frames: usize,
    bpm: Option<f32>,
    /// A competing tempo and how many estimates in a row it has won.
    pending: Option<(f32, usize)>,
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
            max_lag: 0,
            frames: 0,
            bpm: None,
            pending: None,
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
            self.update();
        }
        self.bpm
    }

    /// Forgets the tempo (after silence).
    pub fn reset(&mut self) {
        self.env.clear();
        self.bpm = None;
        self.pending = None;
    }

    fn update(&mut self) {
        if !self.autocorrelate() {
            return;
        }
        let Some((lag, score)) = self.candidate() else {
            return;
        };
        if score < MIN_CONFIDENCE * self.acf[0] {
            return;
        }
        let Some(held) = self.bpm else {
            self.bpm = Some(to_bpm(lag));
            return;
        };
        let held_lag = to_lag(held);
        let (follow_lag, follow_score) = self.refine(held_lag, FOLLOW * held_lag);
        if follow_score >= HOLD_RATIO * score {
            self.bpm = Some(to_bpm(follow_lag));
            self.pending = None;
            return;
        }
        let bpm = to_bpm(lag);
        let wins = match self.pending {
            Some((p, n)) if (bpm - p).abs() <= SAME * p => n + 1,
            _ => 1,
        };
        if wins >= SWITCH_AFTER {
            self.bpm = Some(bpm);
            self.pending = None;
        } else {
            self.pending = Some((bpm, wins));
        }
    }

    /// Fills `acf` from the current window; false if the window is flat.
    fn autocorrelate(&mut self) -> bool {
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
            return false;
        }
        self.max_lag = n - FPS; // keep at least 1 s of overlap
        for lag in 0..self.max_lag {
            let s: f32 = x[..n - lag].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum();
            self.acf[lag] = s / (n - lag) as f32;
        }
        true
    }

    fn acf_at(&self, l: f32) -> f32 {
        let i = l as usize;
        if i + 1 >= self.max_lag {
            return 0.0;
        }
        let f = l - i as f32;
        self.acf[i] * (1.0 - f) + self.acf[i + 1] * f
    }

    /// Mean autocorrelation over the first four multiples of `lag`.
    fn score(&self, lag: f32) -> f32 {
        let mut sum = 0.0;
        let mut k = 1.0;
        while k * lag < (self.max_lag - 1) as f32 && k <= 4.0 {
            sum += self.acf_at(k * lag);
            k += 1.0;
        }
        if k > 1.0 { sum / (k - 1.0) } else { 0.0 }
    }

    /// The best-scoring lag within `radius` of `centre`, kept inside the tempo range.
    fn refine(&self, centre: f32, radius: f32) -> (f32, f32) {
        let (lo, hi) = (to_lag(MAX_BPM), to_lag(MIN_BPM));
        let mut best = (centre, self.score(centre));
        let mut l = (centre - radius).max(lo);
        while l <= (centre + radius).min(hi) {
            let s = self.score(l);
            if s > best.1 {
                best = (l, s);
            }
            l += GRID;
        }
        best
    }

    /// The best period in range as `(lag, score)`, if any lag correlates positively.
    fn candidate(&self) -> Option<(f32, f32)> {
        let lo = to_lag(MAX_BPM);
        let hi = to_lag(MIN_BPM);
        let mut best = (0.0f32, f32::MIN);
        let mut lag = lo;
        while lag <= hi {
            let s = self.score(lag);
            if s > best.1 {
                best = (lag, s);
            }
            lag += GRID;
        }
        // A maximum at either end of the range is a slope, not a peak (e.g. a fade-out).
        if best.1 <= 0.0 || best.0 < lo + GRID || best.0 > hi - GRID {
            return None;
        }
        // A period's multiples also fit its half-tempo, so prefer the faster tempo when it fits
        // nearly as well (tempo only paces effects; double-time is the safer error).
        let half = best.0 / 2.0;
        if half >= lo {
            let refined = self.refine(half, 0.5);
            if refined.1 >= 0.85 * best.1 {
                best = refined;
            }
        }
        Some(best)
    }
}

fn to_bpm(lag: f32) -> f32 {
    60.0 * FPS as f32 / lag
}

fn to_lag(bpm: f32) -> f32 {
    60.0 * FPS as f32 / bpm
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pulses(bpm: f32, frames: std::ops::Range<usize>) -> impl Iterator<Item = f32> {
        let period = to_lag(bpm);
        frames.map(move |i| {
            let phase = (i as f32 / period).fract();
            if phase < 1.0 / period { 1.0 } else { 0.0 }
        })
    }

    #[test]
    fn pulse_train_tempo() {
        for bpm in [72.0f32, 120.0, 128.0, 174.0] {
            let mut t = Tempo::new();
            let mut est = None;
            for v in pulses(bpm, 0..800) {
                est = t.push(v);
            }
            let est = est.unwrap();
            assert!((est - bpm).abs() < 1.0, "{bpm}: {est}");
        }
    }

    #[test]
    fn holds_through_a_breakdown() {
        // 20 s at 124 BPM, 10 s of irregular noise, then the beat again.
        let mut t = Tempo::new();
        // xorshift32: an LCG's low-order structure correlates at power-of-two lags.
        let mut seed = 0x2545_f491u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut during = Vec::new();
        for v in pulses(124.0, 0..2000) {
            t.push(v);
        }
        for _ in 0..1000 {
            during.extend(t.push(noise()));
        }
        for v in pulses(124.0, 3000..3500) {
            during.extend(t.push(v));
        }
        assert!(during.iter().all(|b| (b - 124.0).abs() < 1.5), "{during:?}");
    }

    #[test]
    fn keeps_the_octave_when_half_time_fits() {
        // Strong beats at 87 BPM with weaker off-beats: 87 and 174 both fit. Once 174 is held,
        // a stretch where the off-beats fade doesn't drop it to 87.
        let period = to_lag(174.0);
        let mut t = Tempo::new();
        let mut seen = Vec::new();
        for i in 0..4000usize {
            let beat = (i as f32 / period) as usize;
            let on = (i as f32 / period).fract() < 1.0 / period;
            let off_beat_level = if (1500..2500).contains(&i) { 0.3 } else { 0.9 };
            let v = match (on, beat % 2) {
                (false, _) => 0.0,
                (true, 0) => 1.0,
                (true, _) => off_beat_level,
            };
            seen.extend(t.push(v));
        }
        let first = seen[0];
        assert!((first - 174.0).abs() < 1.5, "{first}");
        assert!(seen.iter().all(|b| (b - first).abs() < 1.5), "{seen:?}");
    }

    #[test]
    fn switches_to_a_new_tempo() {
        let mut t = Tempo::new();
        for v in pulses(100.0, 0..1500) {
            t.push(v);
        }
        assert!((t.bpm().unwrap() - 100.0).abs() < 1.0);
        let mut est = None;
        for v in pulses(140.0, 1500..3000) {
            est = t.push(v);
        }
        let est = est.unwrap();
        assert!((est - 140.0).abs() < 1.0, "{est}");
    }
}
