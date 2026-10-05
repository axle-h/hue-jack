//! WAV files: reading/writing, a real-time paced file input, a null output, and delay measurement.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use super::{CHANNELS, CaptureProcessor, DelayReader, SAMPLE_RATE, frames_to_ns, monotonic_ns};

/// Audio as interleaved samples with a channel count, at [`SAMPLE_RATE`].
#[derive(Clone, Debug)]
pub struct Audio {
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / SAMPLE_RATE as f64
    }
    /// Converts to interleaved stereo (mono is duplicated; extra channels are dropped).
    pub fn to_stereo(&self) -> Vec<f32> {
        match self.channels {
            2 => self.samples.clone(),
            1 => self.samples.iter().flat_map(|&s| [s, s]).collect(),
            n => self
                .samples
                .chunks_exact(n)
                .flat_map(|f| [f[0], f[1]])
                .collect(),
        }
    }
    /// Mixes the given channel range to mono.
    pub fn mono(&self, channels: std::ops::Range<usize>) -> Vec<f32> {
        let k = channels.len() as f32;
        self.samples
            .chunks_exact(self.channels)
            .map(|f| f[channels.clone()].iter().sum::<f32>() / k)
            .collect()
    }
}

/// Reads a WAV file (int or float), resampling to 48 kHz if needed (linear; fine for analysis and tests).
pub fn read_wav(path: &Path) -> Result<Audio> {
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|s| s as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    if channels == 0 {
        bail!("{} has no channels", path.display());
    }
    let audio = Audio { channels, samples };
    if spec.sample_rate == SAMPLE_RATE {
        return Ok(audio);
    }
    let ratio = spec.sample_rate as f64 / SAMPLE_RATE as f64;
    let in_frames = audio.frames();
    let out_frames = (in_frames as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_frames * channels);
    for i in 0..out_frames {
        let x = i as f64 * ratio;
        let j = x as usize;
        let f = (x - j as f64) as f32;
        let k = (j + 1).min(in_frames - 1);
        for c in 0..channels {
            out.push(
                audio.samples[j * channels + c] * (1.0 - f) + audio.samples[k * channels + c] * f,
            );
        }
    }
    Ok(Audio {
        channels,
        samples: out,
    })
}

/// Writes 32-bit float WAV at 48 kHz.
pub fn write_wav(path: &Path, channels: usize, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    for &s in samples {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

/// Plays a WAV file into the capture side in real time (10 ms blocks), then silence until stopped.
pub fn spawn_file_input(
    path: &Path,
    mut capture: CaptureProcessor,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>> {
    let audio = read_wav(path)?.to_stereo();
    let block = (SAMPLE_RATE / 100) as usize * CHANNELS;
    Ok(std::thread::Builder::new()
        .name("file-input".into())
        .spawn(move || {
            let silence = vec![0.0f32; block];
            let start = monotonic_ns();
            let mut frame: i64 = 0;
            while !stop.load(Ordering::Relaxed) {
                let begin = frame as usize * CHANNELS;
                let chunk = if begin < audio.len() {
                    &audio[begin..(begin + block).min(audio.len())]
                } else {
                    &silence[..]
                };
                let due = start + frames_to_ns(frame);
                let now = monotonic_ns();
                if due > now {
                    std::thread::sleep(Duration::from_nanos((due - now) as u64));
                }
                capture.process(chunk, due);
                frame += (chunk.len() / CHANNELS) as i64;
            }
        })?)
}

/// Pulls from the delay line in real time and discards the audio (tests, `--output null`).
pub fn spawn_null_output(mut reader: DelayReader, stop: Arc<AtomicBool>) -> Result<JoinHandle<()>> {
    Ok(std::thread::Builder::new()
        .name("null-output".into())
        .spawn(move || {
            let frames = (SAMPLE_RATE / 100) as i64;
            let mut buf = vec![0.0f32; frames as usize * CHANNELS];
            let start = monotonic_ns();
            let mut n: i64 = 0;
            while !stop.load(Ordering::Relaxed) {
                let due = start + frames_to_ns(n);
                let now = monotonic_ns();
                if due > now {
                    std::thread::sleep(Duration::from_nanos((due - now) as u64));
                }
                reader.read(&mut buf, due);
                n += frames;
            }
        })?)
}

/// Lag (in ms) of `delayed` relative to `reference`, by FFT cross-correlation over 0..max_ms,
/// with parabolic interpolation of the peak.
pub fn measure_delay(reference: &[f32], delayed: &[f32], max_ms: f64) -> f64 {
    let n = reference.len().max(delayed.len());
    let size = (2 * n).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(size);
    let ifft = planner.plan_fft_inverse(size);
    let to_c = |x: &[f32]| {
        let mut v: Vec<Complex<f32>> = x.iter().map(|&s| Complex::new(s, 0.0)).collect();
        v.resize(size, Complex::new(0.0, 0.0));
        v
    };
    let (mut a, mut b) = (to_c(reference), to_c(delayed));
    fft.process(&mut a);
    fft.process(&mut b);
    // corr[k] = sum_t ref[t] * delayed[t + k]
    let mut c: Vec<Complex<f32>> = a.iter().zip(&b).map(|(x, y)| x.conj() * y).collect();
    ifft.process(&mut c);
    let max_lag = ((max_ms / 1000.0 * SAMPLE_RATE as f64) as usize).min(size - 2);
    let (best, _) = (0..=max_lag)
        .map(|k| (k, c[k].re))
        .fold((0, f32::MIN), |m, x| if x.1 > m.1 { x } else { m });
    let (y0, y1, y2) = (
        if best > 0 { c[best - 1].re } else { c[best].re },
        c[best].re,
        c[best + 1].re,
    );
    let denom = y0 - 2.0 * y1 + y2;
    let frac = if denom.abs() > f32::EPSILON {
        0.5 * (y0 - y2) / denom
    } else {
        0.0
    };
    (best as f64 + frac as f64) * 1000.0 / SAMPLE_RATE as f64
}

/// `measure-delay`: one 4-channel file (channels 0–1 reference, 2–3 delayed) or two files.
pub fn measure_delay_files(reference: &Path, delayed: &Path) -> Result<f64> {
    let a = read_wav(reference)?;
    if reference == delayed {
        if a.channels < 4 {
            bail!("a single file needs 4 channels (reference L/R, delayed L/R)");
        }
        return Ok(measure_delay(&a.mono(0..2), &a.mono(2..4), 1000.0));
    }
    let b = read_wav(delayed)?;
    Ok(measure_delay(
        &a.mono(0..a.channels),
        &b.mono(0..b.channels),
        1000.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_known_delay() {
        let n = 48_000 * 3;
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
            })
            .collect();
        let d = 7_200; // 150 ms
        let mut delayed = vec![0.0; d];
        delayed.extend_from_slice(&noise[..n - d]);
        let ms = measure_delay(&noise, &delayed, 1000.0);
        assert!((ms - 150.0).abs() < 0.05, "{ms}");
    }
}
