//! Output smoothing: per-channel attack/decay envelopes so nothing changes faster than about 40 ms
//! (the Zigbee step), whatever the effect asks for.

use super::Rgb;

/// Largest rise per 20 ms frame: full brightness in two frames (40 ms).
pub const MAX_RISE: f32 = 0.5;
/// Largest fall per frame.
pub const MAX_FALL: f32 = 0.5;
/// Decay time constant towards a lower target, in seconds.
pub const DECAY_SECS: f32 = 0.08;

#[derive(Clone, Debug, Default)]
pub struct Smoother {
    state: Vec<Rgb>,
}

impl Smoother {
    /// Moves each channel towards its target within the envelope; `dt` is the frame period.
    pub fn apply(&mut self, target: &[Rgb], dt: f32) -> &[Rgb] {
        if self.state.len() != target.len() {
            self.state = target.to_vec();
            return &self.state;
        }
        let k = 1.0 - (-dt / DECAY_SECS).exp();
        for (s, t) in self.state.iter_mut().zip(target) {
            for ch in 0..3 {
                let (cur, want) = (s[ch], t[ch].clamp(0.0, 1.0));
                s[ch] = if want > cur {
                    (cur + MAX_RISE).min(want)
                } else {
                    (cur - ((cur - want) * k).min(MAX_FALL)).max(want)
                };
            }
        }
        &self.state
    }

    pub fn reset(&mut self) {
        self.state.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rise_and_fall_are_limited() {
        let mut s = Smoother::default();
        s.apply(&[[0.0; 3]], 0.02);
        assert_eq!(s.apply(&[[1.0; 3]], 0.02)[0], [0.5; 3]);
        assert_eq!(s.apply(&[[1.0; 3]], 0.02)[0], [1.0; 3]);
        let after = s.apply(&[[0.0; 3]], 0.02)[0][0];
        assert!(after < 1.0 && after > 0.5, "{after}");
    }
}
