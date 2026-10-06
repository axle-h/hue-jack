//! `hue-jack simulate`: runs the real pipeline (analysis → engine) on a WAV file and writes a
//! self-contained HTML preview: the audio (≤ 30 s clip) and an animated light layout synced to it.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{Result, bail};
use base64::Engine as _;
use serde::Serialize;

use crate::analysis::Analyzer;
use crate::audio::wav::read_wav;
use crate::audio::{CHANNELS, SAMPLE_RATE};
use crate::effects::{self, fake6};
use crate::engine::{Engine, EngineSettings, LIGHT_FPS};
use crate::hue::Channel;

pub const MAX_CLIP_SECS: f64 = 30.0;

#[derive(Debug, Clone, clap::Args)]
pub struct SimulateArgs {
    pub wav: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, default_value = "pulse")]
    pub effect: String,
    #[arg(long, default_value = "sunset")]
    pub palette: String,
    /// Light layout: `fake6`, or a JSON file with `[{"id":0,"x":-0.5,"y":0.8,"z":0}, …]`
    /// (e.g. channels copied from `hue-jack areas`).
    #[arg(long, default_value = "fake6")]
    pub layout: String,
    #[arg(long, default_value_t = 0.8)]
    pub intensity: f32,
    #[arg(long, default_value_t = 1.0)]
    pub brightness: f32,
    /// Start of the clip in seconds.
    #[arg(long, default_value_t = 0.0)]
    pub start: f64,
    /// Clip length in seconds (at most 30).
    #[arg(long, default_value_t = MAX_CLIP_SECS)]
    pub secs: f64,
}

#[derive(Serialize)]
pub struct Preview {
    pub effect: String,
    pub palette: String,
    pub fps: f64,
    pub duration: f64,
    pub channels: Vec<Channel>,
    /// Per frame: one `[r, g, b]` (0..255) per channel.
    pub frames: Vec<Vec<[u8; 3]>>,
    /// Per frame: band levels 0..100 (sub, bass, mid, high, air).
    pub bands: Vec<[u8; 5]>,
    /// Per frame: 1 when an onset landed in it.
    pub onsets: Vec<u8>,
    pub bpm: Option<f32>,
}

fn layout(spec: &str) -> Result<Vec<Channel>> {
    if spec == "fake6" {
        return Ok(fake6());
    }
    let channels: Vec<Channel> = serde_json::from_slice(&std::fs::read(spec)?)?;
    if channels.is_empty() || channels.len() > 20 {
        bail!("layout needs 1–20 channels");
    }
    Ok(channels)
}

/// Runs the pipeline over `stereo` (interleaved, 48 kHz) and returns one preview frame per 20 ms.
pub fn render(
    stereo: &[f32],
    channels: Vec<Channel>,
    settings: &EngineSettings,
) -> Result<Preview> {
    let duration = stereo.len() as f64 / CHANNELS as f64 / SAMPLE_RATE as f64;
    let mut analyzer = Analyzer::new();
    let mut engine = Engine::new(channels.clone(), settings)?;
    let mut features = Vec::new();
    analyzer.push(stereo, &mut features);
    analyzer.flush(&mut features);
    let rendered: Vec<_> = features.iter().filter_map(|f| engine.push(f)).collect();
    // Resample onto an exact 50 Hz grid from t = 0 (the analysis frame times are window centres).
    let n = (duration * LIGHT_FPS).round() as usize;
    let mut frames = Vec::with_capacity(n);
    let mut bands = Vec::with_capacity(n);
    let mut onsets = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let t = i as f64 / LIGHT_FPS;
        while j + 1 < rendered.len() && (rendered[j + 1].t - t).abs() <= (rendered[j].t - t).abs() {
            j += 1;
        }
        let r = &rendered[j];
        frames.push(r.preview.clone());
        bands.push(r.features.bands.map(|b| (b * 100.0).round() as u8));
        onsets.push((r.features.onset > 0.0 || r.features.bass_onset > 0.0) as u8);
    }
    let bpm = rendered.iter().rev().find_map(|r| r.features.bpm);
    Ok(Preview {
        effect: settings.effect.clone(),
        palette: settings.palette.clone(),
        fps: LIGHT_FPS,
        duration,
        channels,
        frames,
        bands,
        onsets,
        bpm,
    })
}

fn wav_base64(stereo: &[f32]) -> Result<String> {
    // 16-bit stereo keeps the page small enough (≈ 7.7 MB of base64 for 30 s).
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec)?;
        for &s in stereo {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        w.finalize()?;
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(cursor.into_inner()))
}

pub fn run(args: SimulateArgs) -> Result<()> {
    if effects::create(&args.effect).is_none() {
        bail!(
            "unknown effect {:?} (one of {:?})",
            args.effect,
            effects::EFFECTS
        );
    }
    let audio = read_wav(&args.wav)?.to_stereo();
    let rate = SAMPLE_RATE as f64;
    let start = ((args.start.max(0.0) * rate) as usize * CHANNELS).min(audio.len());
    let len = (args.secs.clamp(0.1, MAX_CLIP_SECS) * rate) as usize * CHANNELS;
    let clip = &audio[start..(start + len).min(audio.len())];
    let settings = EngineSettings {
        effect: args.effect.clone(),
        palette: args.palette.clone(),
        intensity: args.intensity,
        brightness_max: args.brightness,
    };
    let preview = render(clip, layout(&args.layout)?, &settings)?;
    let html = page(
        &preview,
        &wav_base64(clip)?,
        &args.wav.file_name().unwrap_or_default().to_string_lossy(),
    )?;
    std::fs::write(&args.out, html)?;
    println!(
        "wrote {} ({} frames, {:.1} s, {}, {})",
        args.out.display(),
        preview.frames.len(),
        preview.duration,
        preview.effect,
        preview.palette
    );
    Ok(())
}

/// The JSON embedded in a preview page (for tests and tooling).
pub fn extract_json(html: &str) -> Option<&str> {
    let start = html.find(r#"<script type="application/json" id="preview">"#)?;
    let body = &html[start..];
    let open = body.find('>')? + 1;
    let close = body.find("</script>")?;
    Some(&body[open..close])
}

fn page(preview: &Preview, wav_b64: &str, title: &str) -> Result<String> {
    let json = serde_json::to_string(preview)?.replace("</", "<\\/");
    let title = title.replace(['<', '>', '&'], "");
    Ok(format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>hue-jack preview: {title}</title>
<style>
  :root {{ color-scheme: dark; --bg: #0d0f14; --fg: #e8e8ec; --muted: #8a8f9c; }}
  body {{ margin: 0; background: var(--bg); color: var(--fg); font: 15px/1.4 system-ui, sans-serif; }}
  main {{ max-width: 760px; margin: 0 auto; padding: 16px; }}
  h1 {{ font-size: 18px; margin: 0 0 4px; }}
  p {{ margin: 0 0 12px; color: var(--muted); }}
  canvas {{ width: 100%; aspect-ratio: 4 / 3; display: block; background: #05060a; border-radius: 12px; }}
  audio {{ width: 100%; margin-top: 12px; }}
</style>
</head>
<body>
<main>
  <h1>{title}</h1>
  <p id="meta"></p>
  <canvas id="room" width="1200" height="900"></canvas>
  <audio id="audio" controls src="data:audio/wav;base64,{wav_b64}"></audio>
</main>
<script type="application/json" id="preview">{json}</script>
<script>
const P = JSON.parse(document.getElementById('preview').textContent);
const audio = document.getElementById('audio');
const canvas = document.getElementById('room');
const ctx = canvas.getContext('2d');
document.getElementById('meta').textContent =
  `effect ${{P.effect}} · palette ${{P.palette}} · ${{P.channels.length}} lights · ${{P.duration.toFixed(1)}} s` +
  (P.bpm ? ` · ${{P.bpm.toFixed(1)}} BPM` : '') + ' · top-down view, front of the room at the bottom';
function draw() {{
  const W = canvas.width, H = canvas.height;
  const i = Math.min(P.frames.length - 1, Math.max(0, Math.floor(audio.currentTime * P.fps)));
  ctx.fillStyle = '#05060a';
  ctx.fillRect(0, 0, W, H);
  ctx.globalCompositeOperation = 'lighter';
  const frame = P.frames[i] || [];
  P.channels.forEach((c, k) => {{
    const [r, g, b] = frame[k] || [0, 0, 0];
    const x = (c.x + 1.25) / 2.5 * W, y = (c.y + 1.25) / 2.5 * H;
    const glow = ctx.createRadialGradient(x, y, 0, x, y, 260);
    glow.addColorStop(0, `rgba(${{r}},${{g}},${{b}},0.9)`);
    glow.addColorStop(1, 'rgba(0,0,0,0)');
    ctx.fillStyle = glow;
    ctx.fillRect(x - 260, y - 260, 520, 520);
  }});
  ctx.globalCompositeOperation = 'source-over';
  P.channels.forEach((c, k) => {{
    const [r, g, b] = frame[k] || [0, 0, 0];
    const x = (c.x + 1.25) / 2.5 * W, y = (c.y + 1.25) / 2.5 * H;
    ctx.beginPath(); ctx.arc(x, y, 34, 0, Math.PI * 2);
    ctx.fillStyle = `rgb(${{r}},${{g}},${{b}})`; ctx.fill();
    ctx.lineWidth = 3; ctx.strokeStyle = 'rgba(255,255,255,0.35)'; ctx.stroke();
    ctx.fillStyle = '#aab'; ctx.font = '22px system-ui'; ctx.textAlign = 'center';
    ctx.fillText(String(c.id), x, y + 70);
  }});
  const bands = P.bands[i] || [0, 0, 0, 0, 0];
  ['sub', 'bass', 'mid', 'high', 'air'].forEach((name, k) => {{
    const h = bands[k] / 100 * 120;
    ctx.fillStyle = 'rgba(255,255,255,0.75)';
    ctx.fillRect(24 + k * 34, H - 40 - h, 24, h);
    ctx.fillStyle = '#889'; ctx.font = '16px system-ui'; ctx.textAlign = 'center';
    ctx.fillText(name, 36 + k * 34, H - 16);
  }});
  if (P.onsets[i]) {{ ctx.fillStyle = '#fff'; ctx.beginPath(); ctx.arc(W - 40, 40, 14, 0, Math.PI * 2); ctx.fill(); }}
  requestAnimationFrame(draw);
}}
requestAnimationFrame(draw);
</script>
</body>
</html>
"##
    ))
}
