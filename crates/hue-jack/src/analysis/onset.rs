//! Onset detection: spectral flux on log magnitudes, adaptive threshold (moving median + k·MAD),
//! and look-ahead peak picking (a frame is an onset only if it's the maximum within ±40 ms).

use std::collections::VecDeque;

use super::agc::LinearPeak;
use super::{FPS, LOOKAHEAD};

/// History used for the adaptive threshold, in frames (past side).
const HISTORY: usize = 50;
const K_MAD: f32 = 3.0;
/// Default floor relative to the recent peak flux, so quiet noise never triggers.
pub const REL_FLOOR: f32 = 0.08;
/// Floor for bass onsets: low-frequency flux also rises when overlapping tones stop beating, so
/// only clearly stronger rises count.
pub const BASS_REL_FLOOR: f32 = 0.2;
/// Minimum time between onsets, in frames.
const MIN_GAP: usize = 6;

/// Detects onsets in a flux sequence with `LOOKAHEAD` frames of delay.
pub struct OnsetPicker {
    window: VecDeque<f32>,
    peak: LinearPeak,
    since_last: usize,
    scratch: Vec<f32>,
    rel_floor: f32,
}

impl Default for OnsetPicker {
    fn default() -> Self {
        Self::new(REL_FLOOR)
    }
}

/// The decision for the frame `LOOKAHEAD` frames ago.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pick {
    /// Onset strength 0..1 (0 = no onset).
    pub strength: f32,
    /// Sub-frame position of the peak in frames (-0.5..0.5), from parabolic interpolation.
    pub offset: f32,
}

impl OnsetPicker {
    pub fn new(rel_floor: f32) -> Self {
        Self {
            rel_floor,
            window: VecDeque::with_capacity(HISTORY + 2 * LOOKAHEAD + 1),
            peak: LinearPeak::new(5.0, FPS as f32, 1e-6),
            since_last: usize::MAX / 2,
            scratch: Vec::with_capacity(HISTORY + 2 * LOOKAHEAD + 1),
        }
    }

    /// Pushes the newest flux value; returns the decision for the frame `LOOKAHEAD` frames earlier
    /// (None until enough frames have been seen).
    pub fn push(&mut self, flux: f32) -> Option<Pick> {
        self.window.push_back(flux);
        if self.window.len() > HISTORY + 2 * LOOKAHEAD + 1 {
            self.window.pop_front();
        }
        let n = self.window.len();
        if n < 2 * LOOKAHEAD + 1 {
            return None;
        }
        let c = n - 1 - LOOKAHEAD;
        let x = self.window[c];
        let peak = self.peak.update(x);
        self.since_last = self.since_last.saturating_add(1);

        let is_max = (c - LOOKAHEAD..=c + LOOKAHEAD)
            .all(|i| i == c || self.window[i] < x || (i > c && self.window[i] <= x));
        if !is_max || self.since_last < MIN_GAP {
            return Some(Pick::default());
        }
        // Moving median + k·MAD over the whole window (past and look-ahead).
        self.scratch.clear();
        self.scratch.extend(self.window.iter().copied());
        let median = median(&mut self.scratch);
        for v in self.scratch.iter_mut() {
            *v = (*v - median).abs();
        }
        let mad = median_of(&mut self.scratch);
        let threshold = median + K_MAD * mad + self.rel_floor * peak;
        if x <= threshold {
            return Some(Pick::default());
        }
        self.since_last = 0;
        let (y0, y1, y2) = (self.window[c - 1], x, self.window[c + 1]);
        let denom = y0 - 2.0 * y1 + y2;
        let offset = if denom.abs() > 1e-12 {
            (0.5 * (y0 - y2) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some(Pick {
            strength: ((x - threshold) / (peak - threshold).max(1e-9)).clamp(0.05, 1.0),
            offset,
        })
    }
}

fn median(v: &mut [f32]) -> f32 {
    median_of(v)
}

fn median_of(v: &mut [f32]) -> f32 {
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_isolated_peaks_with_lookahead() {
        let mut p = OnsetPicker::default();
        let mut picks = Vec::new();
        for i in 0..300 {
            let flux = if i % 50 == 25 {
                1.0
            } else {
                0.01 * ((i * 7) % 3) as f32
            };
            if let Some(pick) = p.push(flux) {
                picks.push((i - LOOKAHEAD, pick.strength));
            }
        }
        let onsets: Vec<usize> = picks
            .iter()
            .filter(|(_, s)| *s > 0.0)
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(onsets, vec![25, 75, 125, 175, 225, 275]);
    }
}
