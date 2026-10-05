//! The daemon: audio → analysis → engine → Hue streamer, plus the web UI.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::audio::{self, CaptureFlags, CaptureProcessor, SAMPLE_RATE, delay_line};
use crate::cli::GlobalArgs;
use crate::state::State;

#[derive(Debug, Clone, clap::Args)]
pub struct ServeArgs {
    /// Web UI listen address.
    #[arg(long, default_value = "0.0.0.0:8080")]
    pub listen: SocketAddr,
    /// Run without a bridge: lights only show in the web UI.
    #[arg(long = "virtual")]
    pub virtual_mode: bool,
    /// Sink whose monitor is captured (overrides state.json for this run).
    #[arg(long)]
    pub input_sink: Option<String>,
    /// Output node name, `auto` or `null` (overrides state.json for this run).
    #[arg(long)]
    pub output: Option<String>,
    /// Read audio from a WAV file in real time instead of PipeWire (tests).
    #[arg(long)]
    pub input_file: Option<PathBuf>,
    /// Delay in ms (overrides state.json for this run).
    #[arg(long)]
    pub delay_ms: Option<u32>,
    /// Seconds of silence before the stream stops (overrides state.json for this run).
    #[arg(long)]
    pub idle_stop_secs: Option<u32>,
}

/// Running audio I/O; dropping it stops everything.
pub struct AudioIo {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    #[cfg(feature = "pipewire")]
    pw: Vec<audio::pipewire::PwHandle>,
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        #[cfg(feature = "pipewire")]
        self.pw.drain(..).for_each(|h| h.stop());
        self.threads.drain(..).for_each(|t| {
            let _ = t.join();
        });
    }
}

/// Starts capture and playback around a delay line. Returns the I/O handle, the delay control,
/// the analysis ring consumer and the resolved output name.
pub fn start_audio(
    input_sink: &str,
    output: &str,
    input_file: Option<&std::path::Path>,
    delay_ms: u32,
    flags: Arc<CaptureFlags>,
) -> Result<(AudioIo, audio::DelayControl, rtrb::Consumer<f32>, String)> {
    let (writer, reader, control) = delay_line(delay_ms);
    let (producer, consumer) = rtrb::RingBuffer::new(SAMPLE_RATE as usize * 2 * audio::CHANNELS);
    let capture = CaptureProcessor::new(writer, producer, flags);
    let stop = Arc::new(AtomicBool::new(false));
    let mut io = AudioIo {
        stop: stop.clone(),
        threads: Vec::new(),
        #[cfg(feature = "pipewire")]
        pw: Vec::new(),
    };
    let mut resolved = output.to_string();
    if output == "null" {
        io.threads
            .push(audio::wav::spawn_null_output(reader, stop.clone())?);
    } else {
        #[cfg(feature = "pipewire")]
        {
            resolved = audio::pipewire::resolve_output(output, input_sink)?;
            io.pw
                .push(audio::pipewire::spawn_playback(&resolved, reader)?);
        }
        #[cfg(not(feature = "pipewire"))]
        bail!("built without PipeWire: use --output null");
    }
    match input_file {
        Some(path) => io
            .threads
            .push(audio::wav::spawn_file_input(path, capture, stop.clone())?),
        None => {
            #[cfg(feature = "pipewire")]
            io.pw
                .push(audio::pipewire::spawn_capture(input_sink, capture)?);
            #[cfg(not(feature = "pipewire"))]
            {
                let _ = (capture, input_sink);
                bail!("built without PipeWire: use --input-file");
            }
        }
    }
    Ok((io, control, consumer, resolved))
}

pub fn run(g: &GlobalArgs, args: ServeArgs) -> Result<()> {
    let state = State::load(&g.state_dir())?;
    let input_sink = args.input_sink.clone().unwrap_or(state.input_sink.clone());
    let output = args.output.clone().unwrap_or(state.output.clone());
    let delay_ms = args.delay_ms.unwrap_or(state.delay_ms);
    if output == input_sink {
        bail!("output {output:?} is the input sink: that would be a feedback loop");
    }
    let flags = Arc::new(CaptureFlags::default());
    let (_io, control, mut analysis, output) = start_audio(
        &input_sink,
        &output,
        args.input_file.as_deref(),
        delay_ms,
        flags.clone(),
    )?;
    tracing::info!(input_sink, output, delay_ms, "audio running");
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc_hook(move || stop.store(true, Ordering::Relaxed));
    }
    let mut last = std::time::Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let n = analysis.slots();
        if let Ok(chunk) = analysis.read_chunk(n) {
            chunk.commit_all();
        }
        std::thread::sleep(Duration::from_millis(20));
        if last.elapsed() > Duration::from_secs(10) {
            last = std::time::Instant::now();
            let s = control.stats();
            tracing::info!(
                fill_ms = format!("{:.1}", control.fill_ms()),
                error_frames = s.error_frames.load(Ordering::Relaxed),
                drift_corrections = s.drift_corrections.load(Ordering::Relaxed),
                underruns = s.underruns.load(Ordering::Relaxed),
                resyncs = s.resyncs.load(Ordering::Relaxed),
                "delay line"
            );
        }
    }
    Ok(())
}

/// Calls `f` on SIGINT or SIGTERM.
fn ctrlc_hook(f: impl Fn() + Send + 'static) -> Result<()> {
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .unwrap();
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            });
            f();
        })?;
    Ok(())
}
