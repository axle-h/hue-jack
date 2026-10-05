//! Audio I/O: the delay line, capture processing, and PipeWire / WAV backends.
//!
//! Format throughout: interleaved stereo f32 at 48 kHz.

pub mod delay;
#[cfg(feature = "pipewire")]
pub mod pipewire;
pub mod testgen;
pub mod wav;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub use delay::{DelayControl, DelayReader, DelayStats, DelayWriter, delay_line};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;

/// `CLOCK_MONOTONIC` in nanoseconds: the clock PipeWire timestamps use.
pub fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer to a timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// Converts frames at 48 kHz to nanoseconds.
pub fn frames_to_ns(frames: i64) -> i64 {
    frames * 1_000_000_000 / SAMPLE_RATE as i64
}

/// Calibration click: 1 kHz, 10 ms, every 750 ms.
pub const CLICK_PERIOD_FRAMES: u64 = SAMPLE_RATE as u64 * 750 / 1000;
pub const CLICK_FRAMES: u64 = SAMPLE_RATE as u64 / 100;

/// The calibration click sample at frame `n` of the click period.
fn click_sample(n: u64) -> f32 {
    if n >= CLICK_FRAMES {
        return 0.0;
    }
    let t = n as f32 / SAMPLE_RATE as f32;
    // Short raised-cosine fade at each end so the click itself doesn't add a broadband pop.
    let fade = (n.min(CLICK_FRAMES - 1 - n) as f32 / 48.0).min(1.0);
    0.8 * fade * (std::f32::consts::TAU * 1000.0 * t).sin()
}

/// Shared switches the capture side reads every block.
#[derive(Debug, Default)]
pub struct CaptureFlags {
    /// Calibration mode: replace the input with clicks.
    pub calibration: AtomicBool,
    /// Analysis samples dropped because the analysis thread fell behind.
    pub analysis_overruns: AtomicU64,
}

/// Runs in the capture callback: optional click injection, then into the delay line and the analysis ring.
/// Never allocates or locks.
pub struct CaptureProcessor {
    writer: DelayWriter,
    analysis: rtrb::Producer<f32>,
    flags: Arc<CaptureFlags>,
    click_phase: u64,
    scratch: Vec<f32>,
}

impl CaptureProcessor {
    pub fn new(
        writer: DelayWriter,
        analysis: rtrb::Producer<f32>,
        flags: Arc<CaptureFlags>,
    ) -> Self {
        Self {
            writer,
            analysis,
            flags,
            click_phase: 0,
            scratch: vec![0.0; 16384],
        }
    }

    /// Processes one captured block (interleaved stereo) whose first frame was captured at `capture_ns`.
    pub fn process(&mut self, samples: &[f32], capture_ns: i64) {
        let mut offset = 0;
        // Chunk so the scratch buffer never needs to grow.
        while offset < samples.len() {
            let len = (samples.len() - offset).min(self.scratch.len());
            let chunk = &samples[offset..offset + len];
            let chunk_ns = capture_ns + frames_to_ns((offset / CHANNELS) as i64);
            if self.flags.calibration.load(Ordering::Relaxed) {
                let out = &mut self.scratch[..len];
                for frame in out.as_chunks_mut::<CHANNELS>().0.iter_mut() {
                    let s = click_sample(self.click_phase);
                    frame.fill(s);
                    self.click_phase = (self.click_phase + 1) % CLICK_PERIOD_FRAMES;
                }
                let out = &self.scratch[..len];
                self.writer.write(out, chunk_ns);
                push_all(&mut self.analysis, out, &self.flags);
            } else {
                self.click_phase = 0;
                self.writer.write(chunk, chunk_ns);
                push_all(&mut self.analysis, chunk, &self.flags);
            }
            offset += len;
        }
    }
}

fn push_all(ring: &mut rtrb::Producer<f32>, samples: &[f32], flags: &CaptureFlags) {
    let n = ring.slots().min(samples.len());
    let n = n - n % CHANNELS;
    if let Ok(mut chunk) = ring.write_chunk_uninit(n) {
        let (a, b) = chunk.as_mut_slices();
        let split = a.len();
        for (dst, src) in a.iter_mut().zip(&samples[..split]) {
            dst.write(*src);
        }
        for (dst, src) in b.iter_mut().zip(&samples[split..n]) {
            dst.write(*src);
        }
        // SAFETY: all n slots were initialised above.
        unsafe { chunk.commit_all() };
    }
    if n < samples.len() {
        flags
            .analysis_overruns
            .fetch_add(((samples.len() - n) / CHANNELS) as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_replaces_input_with_clicks() {
        let (writer, mut reader, control) = delay_line(100);
        let (prod, mut cons) = rtrb::RingBuffer::new(SAMPLE_RATE as usize * 4);
        let flags = Arc::new(CaptureFlags::default());
        flags.calibration.store(true, Ordering::Relaxed);
        let mut cap = CaptureProcessor::new(writer, prod, flags);
        let music = vec![0.25f32; SAMPLE_RATE as usize * 2]; // 1 s of constant "music"
        cap.process(&music, 0);
        let mut got = vec![0.0f32; music.len()];
        cons.read_chunk(music.len())
            .unwrap()
            .into_iter()
            .zip(got.iter_mut())
            .for_each(|(s, d)| *d = s);
        // Clicks at 0 and 750 ms, silence elsewhere: the music is muted.
        let frame = |ms: usize| got[ms * 48 * 2];
        assert!(got[..960].iter().any(|s| s.abs() > 0.5));
        assert_eq!(frame(100), 0.0);
        assert!(got[750 * 96..750 * 96 + 960].iter().any(|s| s.abs() > 0.5));
        // The delay line gets the same clicks.
        let mut out = vec![0.0f32; 960];
        let _ = control;
        reader.read(&mut out, frames_to_ns(4800)); // play time 100 ms → captured at 0 ms
        assert!(out.iter().any(|s| s.abs() > 0.5));
    }
}
