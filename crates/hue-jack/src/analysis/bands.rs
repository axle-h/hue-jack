//! Band energies (sub, bass, mid, high, air) with per-band AGC.

use super::agc::PeakFollower;
use super::{FPS, WINDOW};
use crate::audio::SAMPLE_RATE;

/// Band edges in Hz: sub 20–60, bass 60–250, mid 250–2k, high 2k–8k, air 8k–16k.
pub const BAND_EDGES: [f32; 6] = [20.0, 60.0, 250.0, 2000.0, 8000.0, 16000.0];
pub const BAND_NAMES: [&str; 5] = ["sub", "bass", "mid", "high", "air"];
/// Dynamic range mapped onto 0..1 below each band's (AGC) peak.
const RANGE_DB: f32 = 30.0;
/// Bands quieter than the loudest band's peak by more than this read as 0 (stops the AGC from
/// amplifying leakage in bands with no real content).
const CROSS_BAND_DB: f32 = 36.0;
const FLOOR_DB: f32 = -90.0;

fn bin_of(hz: f32) -> usize {
    (hz * WINDOW as f32 / SAMPLE_RATE as f32).round() as usize
}

pub struct Bands {
    ranges: [(usize, usize); 5],
    agc: [PeakFollower; 5],
}

impl Default for Bands {
    fn default() -> Self {
        Self::new()
    }
}

impl Bands {
    pub fn new() -> Self {
        let ranges = std::array::from_fn(|i| {
            let lo = bin_of(BAND_EDGES[i]).max(1);
            let hi = bin_of(BAND_EDGES[i + 1]).max(lo + 1);
            (lo, hi)
        });
        Self {
            ranges,
            agc: std::array::from_fn(|_| PeakFollower::new(10.0, FPS as f32, FLOOR_DB)),
        }
    }

    /// Bin range `[lo, hi)` of a band.
    pub fn range(&self, band: usize) -> (usize, usize) {
        self.ranges[band]
    }

    /// From the power spectrum (normalised so a full-scale sine peaks near 1): raw band energy in dB
    /// and AGC-normalised 0..1 values.
    pub fn process(&mut self, power: &[f32]) -> ([f32; 5], [f32; 5]) {
        let db: [f32; 5] = std::array::from_fn(|b| {
            let (lo, hi) = self.ranges[b];
            let e: f32 = power[lo..hi.min(power.len())].iter().sum();
            10.0 * (e + 1e-12).log10()
        });
        let peaks: [f32; 5] = std::array::from_fn(|b| self.agc[b].update(db[b]));
        let loudest = peaks.iter().cloned().fold(FLOOR_DB, f32::max);
        let norm = std::array::from_fn(|b| {
            let reference = peaks[b].max(loudest - CROSS_BAND_DB);
            (1.0 + (db[b] - reference) / RANGE_DB).clamp(0.0, 1.0)
        });
        (db, norm)
    }
}
