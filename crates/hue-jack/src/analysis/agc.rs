//! Slow automatic gain control: a peak follower with instant attack and exponential release.

/// Peak follower in the dB domain: jumps up to any louder value, then falls at a fixed rate
/// (equivalent to an exponential release of the linear level).
#[derive(Clone, Debug)]
pub struct PeakFollower {
    peak_db: f32,
    release_db_per_frame: f32,
    floor_db: f32,
}

impl PeakFollower {
    /// `release_secs`: time constant of the linear-level release; `fps`: update rate.
    pub fn new(release_secs: f32, fps: f32, floor_db: f32) -> Self {
        // Linear decay by e every release_secs = 8.686 dB per release_secs.
        let release_db_per_frame = 20.0 * std::f32::consts::LOG10_E / (release_secs * fps);
        Self {
            peak_db: floor_db,
            release_db_per_frame,
            floor_db,
        }
    }

    /// Feeds one value (dB) and returns the current peak (dB).
    pub fn update(&mut self, db: f32) -> f32 {
        self.peak_db = (self.peak_db - self.release_db_per_frame)
            .max(db)
            .max(self.floor_db);
        self.peak_db
    }

    pub fn peak_db(&self) -> f32 {
        self.peak_db
    }
}

/// Peak follower on linear values (for normalising onset strength).
#[derive(Clone, Debug)]
pub struct LinearPeak {
    peak: f32,
    decay: f32,
    floor: f32,
}

impl LinearPeak {
    pub fn new(release_secs: f32, fps: f32, floor: f32) -> Self {
        Self {
            peak: floor,
            decay: (-1.0 / (release_secs * fps)).exp(),
            floor,
        }
    }
    pub fn update(&mut self, x: f32) -> f32 {
        self.peak = (self.peak * self.decay).max(x).max(self.floor);
        self.peak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instant_attack_slow_release() {
        let mut p = PeakFollower::new(10.0, 100.0, -100.0);
        assert_eq!(p.update(-20.0), -20.0);
        // After 10 s the peak has fallen by ~8.7 dB.
        let mut last = 0.0;
        for _ in 0..1000 {
            last = p.update(-90.0);
        }
        assert!((last - (-20.0 - 8.686)).abs() < 0.05, "{last}");
        assert_eq!(p.update(0.0), 0.0);
    }
}
