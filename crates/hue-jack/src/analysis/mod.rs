//! Audio analysis (PLAN §9): 100 feature frames per second from 48 kHz stereo.
//!
//! Mono mix → 2048-sample Hann window every 480 samples → FFT → band energies (AGC'd), spectral
//! flux onsets (overall and bass) with ±40 ms look-ahead peak picking, tempo, RMS and silence.
//! Frames come out [`LOOKAHEAD`] frames late, once their look-ahead is complete.

pub mod agc;
pub mod bands;
pub mod onset;
pub mod tempo;

use std::collections::VecDeque;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde::Serialize;

use crate::audio::{CHANNELS, SAMPLE_RATE};
use agc::PeakFollower;
use bands::Bands;
use onset::OnsetPicker;
use tempo::Tempo;

pub const WINDOW: usize = 2048;
pub const HOP: usize = 480;
pub const FPS: usize = SAMPLE_RATE as usize / HOP;
/// Peak-picking look-ahead in frames (±40 ms).
pub const LOOKAHEAD: usize = 4;
/// Silence: RMS below this for [`SILENCE_SECS`].
pub const SILENCE_DB: f32 = -55.0;
pub const SILENCE_SECS: f32 = 1.0;
/// Highest analysed frequency (bins above are ignored for flux).
const MAX_HZ: f32 = 16_000.0;
const BASS_FLUX_HZ: (f32, f32) = (20.0, 250.0);
/// Log compression for flux: ln(1 + γ·|X|).
const GAMMA: f32 = 1000.0;
/// Where in the window an onset's energy rises fastest, relative to the window centre, in
/// samples. Calibrated against synthetic clicks and kicks (see the analysis tests).
const ONSET_TIME_OFFSET: f64 = 400.0;

/// One analysis frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FeatureFrame {
    /// Time in seconds since the start of the stream (centre of the analysis window).
    pub t: f64,
    /// AGC-normalised band levels 0..1: sub, bass, mid, high, air.
    pub bands: [f32; 5],
    /// Raw band energies in dB (before AGC).
    pub band_db: [f32; 5],
    /// Onset strength 0..1 (0 = no onset in this frame).
    pub onset: f32,
    /// Bass onset strength 0..1.
    pub bass_onset: f32,
    /// Onset time in seconds (sub-frame precision), when `onset > 0`.
    pub onset_t: Option<f64>,
    /// Bass onset time in seconds, when `bass_onset > 0`.
    pub bass_onset_t: Option<f64>,
    pub bpm: Option<f32>,
    /// RMS of the last hop (raw level, before AGC), linear and dBFS.
    pub rms: f32,
    pub rms_db: f32,
    pub silent: bool,
}

struct Pending {
    frame: FeatureFrame,
}

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window_fn: Vec<f32>,
    ring: Vec<f32>,
    ring_pos: usize,
    hop_fill: usize,
    hop_energy: f64,
    samples: u64,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    prev_log: Vec<f32>,
    power: Vec<f32>,
    bands: Bands,
    level: PeakFollower,
    onsets: OnsetPicker,
    bass_onsets: OnsetPicker,
    tempo: Tempo,
    pending: VecDeque<Pending>,
    silent_hops: usize,
    max_bin: usize,
    bass_bins: (usize, usize),
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(WINDOW);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        let window_fn = (0..WINDOW)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos())
            .collect();
        let bin = |hz: f32| (hz * WINDOW as f32 / SAMPLE_RATE as f32).round() as usize;
        Self {
            fft,
            window_fn,
            ring: vec![0.0; WINDOW],
            ring_pos: 0,
            hop_fill: 0,
            hop_energy: 0.0,
            samples: 0,
            buf: vec![Complex::default(); WINDOW],
            scratch,
            prev_log: vec![0.0; WINDOW / 2 + 1],
            power: vec![0.0; WINDOW / 2 + 1],
            bands: Bands::new(),
            level: PeakFollower::new(10.0, FPS as f32, -70.0),
            onsets: OnsetPicker::new(onset::REL_FLOOR),
            bass_onsets: OnsetPicker::new(onset::BASS_REL_FLOOR),
            tempo: Tempo::new(),
            pending: VecDeque::with_capacity(LOOKAHEAD + 2),
            silent_hops: 0,
            max_bin: bin(MAX_HZ).min(WINDOW / 2),
            bass_bins: (bin(BASS_FLUX_HZ.0).max(1), bin(BASS_FLUX_HZ.1)),
        }
    }

    /// Delay between a sample arriving and its frame being emitted, in seconds.
    pub fn latency_secs() -> f64 {
        (WINDOW / 2 + LOOKAHEAD * HOP) as f64 / SAMPLE_RATE as f64
    }

    /// Feeds interleaved stereo samples; appends completed frames to `out`.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<FeatureFrame>) {
        for frame in samples.as_chunks::<CHANNELS>().0 {
            let mono = (frame[0] + frame[1]) * 0.5;
            self.push_mono(mono, out);
        }
    }

    /// Feeds mono samples.
    pub fn push_mono_slice(&mut self, samples: &[f32], out: &mut Vec<FeatureFrame>) {
        for &s in samples {
            self.push_mono(s, out);
        }
    }

    /// Flushes the look-ahead at the end of a file (feeds silence).
    pub fn flush(&mut self, out: &mut Vec<FeatureFrame>) {
        for _ in 0..(LOOKAHEAD + 1) * HOP {
            self.push_mono(0.0, out);
        }
    }

    fn push_mono(&mut self, s: f32, out: &mut Vec<FeatureFrame>) {
        self.ring[self.ring_pos] = s;
        self.ring_pos = (self.ring_pos + 1) % WINDOW;
        self.hop_energy += (s as f64) * (s as f64);
        self.hop_fill += 1;
        self.samples += 1;
        if self.hop_fill == HOP {
            self.frame(out);
            self.hop_fill = 0;
            self.hop_energy = 0.0;
        }
    }

    fn frame(&mut self, out: &mut Vec<FeatureFrame>) {
        let rms = (self.hop_energy / HOP as f64).sqrt() as f32;
        let rms_db = 20.0 * (rms + 1e-9).log10();
        let t = (self.samples as f64 - (WINDOW / 2) as f64) / SAMPLE_RATE as f64;

        // Slow AGC on the analysis copy: scale so the level's peak sits at -20 dBFS.
        let peak_db = self.level.update(rms_db);
        let gain = 10f32.powf((-20.0 - peak_db) / 20.0);

        for i in 0..WINDOW {
            let s = self.ring[(self.ring_pos + i) % WINDOW];
            self.buf[i] = Complex::new(s * self.window_fn[i], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        let norm = 4.0 / WINDOW as f32;
        let (mut flux, mut bass_flux) = (0.0f32, 0.0f32);
        for k in 0..=WINDOW / 2 {
            let m = self.buf[k].norm() * norm;
            self.power[k] = m * m;
            if k == 0 || k > self.max_bin {
                continue;
            }
            let l = (1.0 + GAMMA * m * gain).ln();
            let d = (l - self.prev_log[k]).max(0.0);
            self.prev_log[k] = l;
            flux += d;
            if (self.bass_bins.0..self.bass_bins.1).contains(&k) {
                bass_flux += d;
            }
        }
        flux /= self.max_bin as f32;
        bass_flux /= (self.bass_bins.1 - self.bass_bins.0) as f32;
        let (band_db, bands) = self.bands.process(&self.power);

        // Silence: raw RMS below -55 dBFS for 1 s; cleared by the first louder hop.
        if rms_db < SILENCE_DB {
            self.silent_hops += 1;
        } else {
            self.silent_hops = 0;
        }
        let silent = self.silent_hops as f32 >= SILENCE_SECS * FPS as f32;
        if silent && self.silent_hops == (SILENCE_SECS * FPS as f32) as usize {
            self.tempo.reset();
        }

        self.pending.push_back(Pending {
            frame: FeatureFrame {
                t,
                bands: if silent { [0.0; 5] } else { bands },
                band_db,
                rms,
                rms_db,
                silent,
                ..Default::default()
            },
        });
        let pick = self.onsets.push(flux);
        let bass_pick = self.bass_onsets.push(bass_flux);
        let bpm = self.tempo.push(flux);
        if self.pending.len() > LOOKAHEAD
            && let Some(mut p) = self.pending.pop_front()
        {
            let hop_secs = HOP as f64 / SAMPLE_RATE as f64;
            let at = |offset: f32| {
                p.frame.t + ONSET_TIME_OFFSET / SAMPLE_RATE as f64 + offset as f64 * hop_secs
            };
            if let Some(pick) = pick.filter(|p| p.strength > 0.0)
                && !p.frame.silent
            {
                p.frame.onset = pick.strength;
                p.frame.onset_t = Some(at(pick.offset));
            }
            if let Some(pick) = bass_pick.filter(|p| p.strength > 0.0)
                && !p.frame.silent
            {
                p.frame.bass_onset = pick.strength;
                p.frame.bass_onset_t = Some(at(pick.offset));
            }
            p.frame.bpm = bpm;
            out.push(p.frame);
        }
    }
}

/// Analyses a whole WAV file.
pub fn analyze_file(path: &Path) -> Result<Vec<FeatureFrame>> {
    let audio = crate::audio::wav::read_wav(path)?;
    Ok(analyze_stereo(&audio.to_stereo()))
}

/// Analyses interleaved stereo samples (flushing the look-ahead at the end).
pub fn analyze_stereo(samples: &[f32]) -> Vec<FeatureFrame> {
    let mut a = Analyzer::new();
    let mut out = Vec::with_capacity(samples.len() / CHANNELS / HOP + 8);
    a.push(samples, &mut out);
    a.flush(&mut out);
    out
}

/// Analyses mono samples.
pub fn analyze_mono(samples: &[f32]) -> Vec<FeatureFrame> {
    let mut a = Analyzer::new();
    let mut out = Vec::with_capacity(samples.len() / HOP + 8);
    a.push_mono_slice(samples, &mut out);
    a.flush(&mut out);
    out
}

/// `hue-jack analyze`.
pub fn analyze_cmd(wav: &Path, csv: Option<&Path>) -> Result<()> {
    let started = std::time::Instant::now();
    let frames = analyze_file(wav)?;
    let elapsed = started.elapsed();
    if let Some(csv) = csv {
        let mut f = std::io::BufWriter::new(std::fs::File::create(csv)?);
        writeln!(
            f,
            "t,rms_db,silent,sub,bass,mid,high,air,onset,bass_onset,bpm"
        )?;
        for fr in &frames {
            writeln!(
                f,
                "{:.3},{:.1},{},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{:.3},{}",
                fr.t,
                fr.rms_db,
                fr.silent as u8,
                fr.bands[0],
                fr.bands[1],
                fr.bands[2],
                fr.bands[3],
                fr.bands[4],
                fr.onset,
                fr.bass_onset,
                fr.bpm.map(|b| format!("{b:.1}")).unwrap_or_default()
            )?;
        }
        println!("wrote {} frames to {}", frames.len(), csv.display());
    }
    let duration = frames.last().map(|f| f.t).unwrap_or(0.0);
    let onsets = frames.iter().filter(|f| f.onset > 0.0).count();
    let bass = frames.iter().filter(|f| f.bass_onset > 0.0).count();
    let silent = frames.iter().filter(|f| f.silent).count();
    // The median over the track: the last estimate is often from a fade-out.
    let mut bpms: Vec<f32> = frames.iter().filter_map(|f| f.bpm).collect();
    bpms.sort_by(f32::total_cmp);
    let bpm = bpms.get(bpms.len() / 2).copied();
    println!(
        "{}: {:.1} s, {} frames analysed in {:.0} ms",
        wav.display(),
        duration,
        frames.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    println!(
        "onsets {onsets} (bass {bass}), silent {:.1} s, tempo {}",
        silent as f64 / FPS as f64,
        bpm.map(|b| format!("{b:.1} BPM")).unwrap_or("-".into())
    );
    Ok(())
}
