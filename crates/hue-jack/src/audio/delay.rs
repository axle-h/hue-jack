//! The delay line between capture and playback.
//!
//! The writer (capture callback) appends frames and publishes an anchor: the frame index of the
//! block's first frame and its capture time. The reader (playback callback) is told when its buffer
//! will be heard, and plays the frames captured `D` earlier, so the end-to-end delay is `D`
//! regardless of quantum sizes or callback order. Lock-free; the reader never allocates.
//!
//! - `D` changes are crossfaded over 20 ms (no clicks).
//! - Drift guard: capture and playback can run off different clocks. If the reader's position drifts
//!   more than 5 ms from where it should be, it drops or repeats one frame per 10 ms until it is
//!   back within 0.5 ms, and counts each correction.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

use super::{CHANNELS, SAMPLE_RATE};

/// Ring capacity in frames (≈2.7 s, comfortably more than the 1000 ms maximum delay).
const CAPACITY: usize = 1 << 17;
pub const MAX_DELAY_MS: u32 = 1000;
const CROSSFADE_FRAMES: i64 = SAMPLE_RATE as i64 / 50;
const DRIFT_START: i64 = SAMPLE_RATE as i64 / 200;
const DRIFT_STOP: i64 = SAMPLE_RATE as i64 / 2000;
const DRIFT_STEP: i64 = SAMPLE_RATE as i64 / 100;
/// Beyond this the reader resynchronises (with a crossfade) instead of nudging.
const RESYNC: i64 = SAMPLE_RATE as i64 / 10;
/// A jump in error between consecutive blocks larger than this is a re-anchoring (the graph's
/// reported latency changed, e.g. when nodes are regrouped), not drift: resynchronise at once.
const STEP: i64 = SAMPLE_RATE as i64 / 1000;

/// Statistics for `/api/status`.
#[derive(Debug, Default)]
pub struct DelayStats {
    pub drift_corrections: AtomicU64,
    pub underruns: AtomicU64,
    pub resyncs: AtomicU64,
    /// Frames between the newest captured frame and the read position.
    pub fill_frames: AtomicI64,
    /// Where the reader should be minus where it is, in frames.
    pub error_frames: AtomicI64,
}

struct Shared {
    buf: Box<[AtomicU32]>,
    written: AtomicU64,
    seq: AtomicU64,
    anchor_frame: AtomicU64,
    anchor_ns: AtomicI64,
    delay_frames: AtomicU32,
    stats: Arc<DelayStats>,
}

impl Shared {
    fn sample(&self, frame: i64, ch: usize, written: u64) -> f32 {
        if frame < 0 || frame as u64 >= written || (written - frame as u64) as usize > CAPACITY {
            return 0.0;
        }
        let idx = (frame as usize % CAPACITY) * CHANNELS + ch;
        f32::from_bits(self.buf[idx].load(Ordering::Relaxed))
    }

    /// Reads the latest anchor consistently (seqlock).
    fn anchor(&self) -> Option<(u64, i64)> {
        for _ in 0..8 {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 == 0 {
                return None;
            }
            if s1 % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let frame = self.anchor_frame.load(Ordering::Acquire);
            let ns = self.anchor_ns.load(Ordering::Acquire);
            if self.seq.load(Ordering::Acquire) == s1 {
                return Some((frame, ns));
            }
        }
        None
    }
}

/// Changes the delay at runtime and reads statistics.
#[derive(Clone)]
pub struct DelayControl {
    shared: Arc<Shared>,
}

impl DelayControl {
    pub fn set_delay_ms(&self, ms: u32) {
        let frames = ms.min(MAX_DELAY_MS) * SAMPLE_RATE / 1000;
        self.shared.delay_frames.store(frames, Ordering::Relaxed);
    }
    pub fn delay_ms(&self) -> u32 {
        self.shared.delay_frames.load(Ordering::Relaxed) * 1000 / SAMPLE_RATE
    }
    pub fn stats(&self) -> &DelayStats {
        &self.shared.stats
    }
    pub fn fill_ms(&self) -> f64 {
        self.shared.stats.fill_frames.load(Ordering::Relaxed) as f64 * 1000.0 / SAMPLE_RATE as f64
    }
}

pub struct DelayWriter {
    shared: Arc<Shared>,
    written: u64,
}

impl DelayWriter {
    /// Appends interleaved stereo frames; `capture_ns` is when the first one was captured.
    pub fn write(&mut self, samples: &[f32], capture_ns: i64) {
        let start = self.written;
        for (i, frame) in samples.as_chunks::<CHANNELS>().0.iter().enumerate() {
            let base = ((start as usize + i) % CAPACITY) * CHANNELS;
            for (ch, s) in frame.iter().enumerate() {
                self.shared.buf[base + ch].store(s.to_bits(), Ordering::Relaxed);
            }
        }
        let s = &self.shared;
        let seq = s.seq.load(Ordering::Relaxed);
        s.seq.store(seq + 1, Ordering::Release);
        s.anchor_frame.store(start, Ordering::Release);
        s.anchor_ns.store(capture_ns, Ordering::Release);
        s.seq.store(seq + 2, Ordering::Release);
        self.written = start + (samples.len() / CHANNELS) as u64;
        s.written.store(self.written, Ordering::Release);
    }
}

pub struct DelayReader {
    shared: Arc<Shared>,
    pos: Option<i64>,
    current_delay: u32,
    fade_from: i64,
    fade_left: i64,
    correcting: bool,
    since_correction: i64,
    prev_error: Option<i64>,
}

/// Creates a delay line with an initial delay.
pub fn delay_line(delay_ms: u32) -> (DelayWriter, DelayReader, DelayControl) {
    let shared = Arc::new(Shared {
        buf: (0..CAPACITY * CHANNELS)
            .map(|_| AtomicU32::new(0))
            .collect(),
        written: AtomicU64::new(0),
        seq: AtomicU64::new(0),
        anchor_frame: AtomicU64::new(0),
        anchor_ns: AtomicI64::new(0),
        delay_frames: AtomicU32::new(0),
        stats: Arc::new(DelayStats::default()),
    });
    let control = DelayControl {
        shared: shared.clone(),
    };
    control.set_delay_ms(delay_ms);
    let reader = DelayReader {
        shared: shared.clone(),
        pos: None,
        current_delay: 0,
        fade_from: 0,
        fade_left: 0,
        correcting: false,
        since_correction: 0,
        prev_error: None,
    };
    (DelayWriter { shared, written: 0 }, reader, control)
}

impl DelayReader {
    /// Fills `out` (interleaved stereo) with the audio to be heard from `play_ns` onwards.
    pub fn read(&mut self, out: &mut [f32], play_ns: i64) {
        let shared = self.shared.clone();
        let s = &*shared;
        let written = s.written.load(Ordering::Acquire);
        let Some((anchor_frame, anchor_ns)) = s.anchor() else {
            out.fill(0.0);
            return;
        };
        let delay = s.delay_frames.load(Ordering::Relaxed);
        let since_anchor = (play_ns - anchor_ns) as f64 * SAMPLE_RATE as f64 / 1e9;
        let desired = anchor_frame as i64 + since_anchor.round() as i64 - delay as i64;

        let mut pos = match self.pos {
            None => {
                self.current_delay = delay;
                desired
            }
            Some(pos) if delay != self.current_delay => {
                // Keep any drift offset; move by the change in delay, crossfading from the old position.
                self.start_fade(pos);
                let moved = pos + self.current_delay as i64 - delay as i64;
                self.current_delay = delay;
                moved
            }
            Some(pos) => pos,
        };
        let mut error = desired - pos;
        let stepped = self
            .prev_error
            .is_some_and(|prev| (error - prev).abs() > STEP);
        if error.abs() > RESYNC || (stepped && error.abs() > DRIFT_STOP) {
            self.start_fade(pos);
            pos = desired;
            error = 0;
            s.stats.resyncs.fetch_add(1, Ordering::Relaxed);
        }
        if error.abs() > DRIFT_START {
            self.correcting = true;
        } else if error.abs() < DRIFT_STOP {
            self.correcting = false;
        }

        let mut underrun = false;
        for frame in out.as_chunks_mut::<CHANNELS>().0.iter_mut() {
            if self.correcting && self.since_correction >= DRIFT_STEP && error != 0 {
                // Behind (error > 0): skip a frame. Ahead: repeat one.
                let step = error.signum();
                pos += step;
                error -= step;
                self.since_correction = 0;
                s.stats.drift_corrections.fetch_add(1, Ordering::Relaxed);
                if error.abs() < DRIFT_STOP {
                    self.correcting = false;
                }
            }
            if pos >= written as i64 {
                underrun = true;
            }
            for (ch, out) in frame.iter_mut().enumerate() {
                let mut v = s.sample(pos, ch, written);
                if self.fade_left > 0 {
                    let a = 1.0 - self.fade_left as f32 / CROSSFADE_FRAMES as f32;
                    v = a * v + (1.0 - a) * s.sample(self.fade_from, ch, written);
                }
                *out = v;
            }
            if self.fade_left > 0 {
                self.fade_left -= 1;
                self.fade_from += 1;
            }
            pos += 1;
            self.since_correction += 1;
        }
        if underrun {
            s.stats.underruns.fetch_add(1, Ordering::Relaxed);
        }
        self.pos = Some(pos);
        self.prev_error = Some(error);
        s.stats
            .fill_frames
            .store(written as i64 - pos, Ordering::Relaxed);
        s.stats.error_frames.store(error, Ordering::Relaxed);
    }

    fn start_fade(&mut self, from: i64) {
        // If a fade is already running, restart from the old stream (it's still mostly audible).
        if self.fade_left == 0 {
            self.fade_from = from;
        }
        self.fade_left = CROSSFADE_FRAMES;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::frames_to_ns;

    /// Simulates capture blocks of `wblk` and playback blocks of `rblk` frames on one timeline.
    /// `input(frame)` gives the mono input; `rate_ppm` makes playback consume faster than capture.
    /// Returns the mono output per playback frame and the reader for inspection.
    fn simulate(
        delay_ms: u32,
        secs: f64,
        wblk: usize,
        rblk: usize,
        rate_ppm: f64,
        input: impl Fn(u64) -> f32,
        mut on_block: impl FnMut(u64, &DelayControl),
    ) -> (Vec<f32>, DelayControl) {
        let (mut w, mut r, control) = delay_line(delay_ms);
        let total = (secs * SAMPLE_RATE as f64) as u64;
        let mut written = 0u64;
        let mut played = 0u64;
        let mut out = Vec::with_capacity(total as usize);
        let mut buf = vec![0.0f32; rblk * CHANNELS];
        let play_rate = SAMPLE_RATE as f64 * (1.0 + rate_ppm / 1e6);
        while played < total {
            let play_ns = (played as f64 / play_rate * 1e9) as i64;
            // Capture everything up to "now" (block-wise).
            while frames_to_ns((written + wblk as u64) as i64)
                <= play_ns + frames_to_ns(rblk as i64)
            {
                let block: Vec<f32> = (0..wblk as u64)
                    .flat_map(|i| [input(written + i); 2])
                    .collect();
                w.write(&block, frames_to_ns(written as i64));
                written += wblk as u64;
            }
            on_block(played, &control);
            r.read(&mut buf, play_ns);
            out.extend(buf.as_chunks::<2>().0.iter().map(|f| f[0]));
            played += rblk as u64;
        }
        (out, control)
    }

    #[test]
    fn exact_sample_delay() {
        for (delay_ms, wblk, rblk) in [
            (150, 256, 128),
            (300, 1024, 256),
            (20, 480, 480),
            (999, 128, 1024),
        ] {
            let ramp = |n: u64| (n % 10_000) as f32;
            let (out, control) = simulate(delay_ms, 3.0, wblk, rblk, 0.0, ramp, |_, _| {});
            let d = (delay_ms * 48) as usize;
            for (i, &v) in out.iter().enumerate().skip(d + 1) {
                assert_eq!(v, ramp((i - d) as u64), "delay {delay_ms} ms, frame {i}");
            }
            assert!(
                out[..d].iter().all(|&v| v == 0.0),
                "silence before the first delayed sample"
            );
            assert_eq!(control.stats().drift_corrections.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn delay_change_is_smooth() {
        let sine = |n: u64| 0.5 * (std::f32::consts::TAU * 100.0 * n as f32 / 48_000.0).sin();
        let (out, control) = simulate(150, 3.0, 256, 256, 0.0, sine, |played, c| {
            if played == 48_000 {
                c.set_delay_ms(300);
            }
            if played == 96_000 {
                c.set_delay_ms(40);
            }
        });
        let natural = std::f32::consts::TAU * 100.0 / 48_000.0 * 0.5;
        let max_jump = out
            .windows(2)
            .skip(150 * 48 + 1)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(
            max_jump < natural + 2.0 / CROSSFADE_FRAMES as f32,
            "max jump {max_jump}"
        );
        // After the crossfade the new delay is exact.
        for (i, v) in out.iter().enumerate().take(144_000).skip(98_000) {
            assert!((v - sine((i - 40 * 48) as u64)).abs() < 1e-6, "frame {i}");
        }
        assert_eq!(control.delay_ms(), 40);
    }

    #[test]
    fn drift_guard_tracks_a_faster_playback_clock() {
        // Playback consumes 0.1 % faster than capture: 48 frames/s of drift.
        let (_, control) = simulate(150, 60.0, 256, 256, 1000.0, |_| 0.1, |_, _| {});
        let stats = control.stats();
        let corrections = stats.drift_corrections.load(Ordering::Relaxed);
        let error = stats.error_frames.load(Ordering::Relaxed);
        assert!(error.abs() <= DRIFT_START + 2, "error {error} frames");
        // ~48 corrections per second once the 5 ms threshold is reached after ~5 s.
        assert!(
            (2000..=3000).contains(&corrections),
            "{corrections} corrections"
        );
        assert_eq!(stats.resyncs.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn drift_guard_tracks_a_slower_playback_clock() {
        let (_, control) = simulate(150, 30.0, 512, 128, -500.0, |_| 0.1, |_, _| {});
        let error = control.stats().error_frames.load(Ordering::Relaxed);
        assert!(error.abs() <= DRIFT_START + 2, "error {error} frames");
        assert!(control.stats().drift_corrections.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn latency_step_resyncs_quickly() {
        // The capture side's reported latency jumps by 256 frames: the reader follows within a block.
        let (mut w, mut r, control) = delay_line(150);
        let ramp = |n: u64| (n % 10_000) as f32;
        let mut out = vec![0.0f32; 512];
        let mut got = Vec::new();
        for block in 0..400u64 {
            let start = block * 256;
            let frames: Vec<f32> = (start..start + 256).flat_map(|i| [ramp(i); 2]).collect();
            let shift = if block >= 200 { 256 } else { 0 }; // capture now reported 256 frames earlier
            w.write(&frames, frames_to_ns(start as i64 - shift));
            r.read(&mut out, frames_to_ns(start as i64));
            got.extend(out.as_chunks::<2>().0.iter().map(|f| f[0]));
        }
        assert_eq!(control.stats().resyncs.load(Ordering::Relaxed), 1);
        // Well after the step, output = input delayed by D - 256 frames (the capture was older than assumed).
        let d = 150 * 48 - 256;
        for (i, &v) in got.iter().enumerate().skip(260 * 256) {
            assert_eq!(v, ramp((i - d) as u64), "frame {i}");
        }
    }

    #[test]
    fn no_input_gives_silence() {
        let (_, mut r, _) = delay_line(100);
        let mut out = vec![1.0; 64];
        r.read(&mut out, 0);
        assert!(out.iter().all(|&s| s == 0.0));
    }
}
