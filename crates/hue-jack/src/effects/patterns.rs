//! Test patterns (no audio): `chase`, `strobe`, `rainbow`, `identify`.

use std::str::FromStr;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use super::{Rgb, hsv, x_order};
use crate::hue::Channel;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pattern {
    Chase,
    Strobe,
    Rainbow,
    Identify,
}

impl FromStr for Pattern {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "chase" => Self::Chase,
            "strobe" => Self::Strobe,
            "rainbow" => Self::Rainbow,
            "identify" => Self::Identify,
            _ => bail!("unknown pattern {s:?} (chase, strobe, rainbow, identify)"),
        })
    }
}

impl Pattern {
    pub fn name(self) -> &'static str {
        match self {
            Self::Chase => "chase",
            Self::Strobe => "strobe",
            Self::Rainbow => "rainbow",
            Self::Identify => "identify",
        }
    }
}

/// Seconds each channel stays lit in `identify`.
pub const IDENTIFY_SECS: f32 = 2.0;
/// Seconds per step in `chase`.
pub const CHASE_STEP_SECS: f32 = 0.25;
/// Strobe frequency in Hz (on for a third of each period).
pub const STROBE_HZ: f32 = 3.0;

/// For `identify`: which channel (index into `channels`) is lit at time `t`.
pub fn identify_index(channels: &[Channel], t: f32) -> Option<usize> {
    if channels.is_empty() {
        return None;
    }
    let order = x_order(channels);
    Some(order[(t / IDENTIFY_SECS) as usize % order.len()])
}

/// Renders the pattern at time `t` (seconds since start); one colour per channel, in `channels` order.
pub fn render(pattern: Pattern, channels: &[Channel], t: f32) -> Vec<Rgb> {
    let n = channels.len();
    let mut out = vec![[0.0; 3]; n];
    if n == 0 {
        return out;
    }
    let order = x_order(channels);
    match pattern {
        Pattern::Chase => {
            let pos = t / CHASE_STEP_SECS;
            let head = pos as usize % n;
            let colour = hsv(t * 0.05, 0.9, 1.0);
            for (rank, &i) in order.iter().enumerate() {
                // Distance behind the head, in steps; the tail fades over two steps.
                let behind = ((head + n - rank) % n) as f32 + pos.fract();
                let level = (1.0 - behind / 2.5).max(0.0);
                out[i] = colour.map(|c| c * level);
            }
        }
        Pattern::Strobe => {
            let on = (t * STROBE_HZ).fract() < 1.0 / 3.0;
            let v = if on { 1.0 } else { 0.0 };
            out.fill([v, v, v]);
        }
        Pattern::Rainbow => {
            for (rank, &i) in order.iter().enumerate() {
                out[i] = hsv(t * 0.2 + rank as f32 / n as f32, 1.0, 1.0);
            }
        }
        Pattern::Identify => {
            if let Some(i) = identify_index(channels, t) {
                out[i] = [1.0, 1.0, 1.0];
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chans() -> Vec<Channel> {
        [0.5, -0.5, 0.0]
            .iter()
            .enumerate()
            .map(|(i, &x)| Channel {
                id: i as u8,
                x,
                y: 0.0,
                z: 0.0,
            })
            .collect()
    }

    #[test]
    fn identify_walks_left_to_right() {
        let c = chans();
        assert_eq!(identify_index(&c, 0.1), Some(1)); // x = -0.5
        assert_eq!(identify_index(&c, 2.1), Some(2)); // x = 0.0
        assert_eq!(identify_index(&c, 4.1), Some(0)); // x = 0.5
        let lit: Vec<bool> = render(Pattern::Identify, &c, 0.1)
            .iter()
            .map(|c| c[0] > 0.0)
            .collect();
        assert_eq!(lit, vec![false, true, false]);
    }

    #[test]
    fn strobe_duty() {
        let c = chans();
        assert_eq!(render(Pattern::Strobe, &c, 0.0)[0], [1.0; 3]);
        assert_eq!(render(Pattern::Strobe, &c, 0.2)[0], [0.0; 3]);
    }

    #[test]
    fn chase_head_is_brightest() {
        let c = chans();
        let frame = render(Pattern::Chase, &c, 0.0);
        let max = |rgb: &Rgb| rgb.iter().cloned().fold(0.0, f32::max);
        assert!(max(&frame[1]) > max(&frame[2]) && max(&frame[1]) > max(&frame[0]));
    }
}
