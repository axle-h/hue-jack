//! The daemon: audio → analysis → engine → Hue streamer, plus the web UI.
//!
//! Threads: PipeWire capture and playback (real-time, lock-free), the engine thread (analysis,
//! effects, activity detection), the 50 Hz DTLS stream thread, and a tokio runtime for the web UI,
//! the stream lifecycle controller and the source watchers.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::http::StatusCode;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};

use crate::analysis::{Analyzer, FeatureFrame, SILENCE_DB};
use crate::audio::{self, CaptureFlags, CaptureProcessor, DelayControl, SAMPLE_RATE, delay_line};
use crate::cli::GlobalArgs;
use crate::effects::patterns::Pattern;
use crate::effects::{self, fake6};
use crate::engine::{Engine, EngineSettings, LightFrame};
use crate::hue::clip::HueClient;
use crate::hue::stream::{FrameSlot, StreamSession, StreamState, StreamStats};
use crate::hue::{self, Area, BridgeCredentials, Channel};
use crate::sources::Sources;
use crate::sources::bluetooth::Bluetooth;
use crate::state::State;
use crate::web::api::{ApiError, ApiResult, SettingsPatch, bad_request};
use crate::web::ws::{Levels, LiveFrame, WsChannel};

/// How long the link button can take.
pub const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
/// Audio louder than −55 dBFS for this long counts as active (starts the stream).
pub const ACTIVE_AFTER_SECS: f64 = 0.5;
const WS_PERIOD_SECS: f64 = 0.05;

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
    /// Start Bluetooth (pairing agent, adapter control) and the source watchers (MPRIS on the
    /// session bus, the YouTube sidecar). Off by default so a dev run never touches the desktop's
    /// Bluetooth adapter or media players; the appliance's unit turns it on.
    #[arg(long, env = "HUEJACK_SOURCES")]
    pub sources: bool,
}

/// Settings as the API shows them.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
pub struct SettingsView {
    pub area_id: Option<String>,
    pub effect: String,
    pub palette: String,
    pub intensity: f32,
    pub brightness_max: f32,
    pub delay_ms: u32,
    pub idle_stop_secs: u32,
}

impl From<&State> for SettingsView {
    fn from(s: &State) -> Self {
        Self {
            area_id: s.area_id.clone(),
            effect: s.effect.clone(),
            palette: s.palette.clone(),
            intensity: s.intensity,
            brightness_max: s.brightness_max,
            delay_ms: s.delay_ms,
            idle_stop_secs: s.idle_stop_secs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PairState {
    Idle,
    Waiting,
    Paired,
    Failed,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairStatus {
    pub state: PairState,
    pub message: Option<String>,
    pub remaining_secs: u64,
    #[serde(skip)]
    deadline: Option<Instant>,
}

impl PairStatus {
    pub fn idle() -> Self {
        Self {
            state: PairState::Idle,
            message: None,
            remaining_secs: 0,
            deadline: None,
        }
    }
    pub fn waiting(timeout: Duration, message: &str) -> Self {
        Self {
            state: PairState::Waiting,
            message: Some(message.into()),
            remaining_secs: timeout.as_secs(),
            deadline: Some(Instant::now() + timeout),
        }
    }
    pub fn view(&self) -> Self {
        let mut v = self.clone();
        v.remaining_secs = self
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0);
        v
    }
}

/// What the engine thread reads (it checks `version` every frame).
#[derive(Default)]
pub struct EngineControl {
    pub settings: Mutex<EngineSettings>,
    pub channels: Mutex<Vec<Channel>>,
    pub pattern: Mutex<Option<Pattern>>,
    pub calibration: AtomicBool,
    pub idle_stop_secs: Mutex<f64>,
    version: AtomicU64,
}

impl EngineControl {
    pub fn bump(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Live {
    pub active: bool,
    pub silent: bool,
    pub rms_db: f32,
    pub bpm: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct ImageInfo {
    pub digest: Option<String>,
    pub version: Option<String>,
    pub booted: Option<String>,
}

pub enum Ctl {
    AudioActive(bool),
    Reevaluate,
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

pub struct App {
    pub global: GlobalArgs,
    pub state_dir: PathBuf,
    pub state: Mutex<State>,
    pub virtual_mode: bool,
    pub input_sink: String,
    pub output: String,
    pub delay: DelayControl,
    pub capture_flags: Arc<CaptureFlags>,
    pub engine: Arc<EngineControl>,
    pub live: Mutex<Live>,
    pub ws: broadcast::Sender<Arc<str>>,
    pub stream_stats: Arc<StreamStats>,
    pub frames: FrameSlot,
    pub controller: mpsc::UnboundedSender<Ctl>,
    pub pair: Mutex<PairStatus>,
    pub areas: Mutex<Option<Vec<Area>>>,
    pub test_pattern: Mutex<Option<(Pattern, Instant)>>,
    pub sources: Arc<Sources>,
    pub bluetooth: Option<Arc<Bluetooth>>,
    pub image: Option<ImageInfo>,
    streaming_area: Mutex<Option<String>>,
}

/// Everything needed to build an [`App`] (tests build one without audio).
pub struct AppParts {
    pub global: GlobalArgs,
    pub state: State,
    pub virtual_mode: bool,
    pub input_sink: String,
    pub output: String,
    pub delay: DelayControl,
    pub capture_flags: Arc<CaptureFlags>,
    pub bluetooth: Option<Arc<Bluetooth>>,
    pub image: Option<ImageInfo>,
}

impl App {
    /// Builds the app and returns the controller's receiving end (run it with [`controller`]).
    pub fn new(parts: AppParts) -> (Arc<App>, mpsc::UnboundedReceiver<Ctl>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let (ws, _) = broadcast::channel(16);
        let s = &parts.state;
        let engine = Arc::new(EngineControl::default());
        *engine.settings.lock().unwrap() = EngineSettings {
            effect: s.effect.clone(),
            palette: s.palette.clone(),
            intensity: s.intensity,
            brightness_max: s.brightness_max,
        };
        *engine.channels.lock().unwrap() = fake6();
        *engine.idle_stop_secs.lock().unwrap() = s.idle_stop_secs as f64;
        let stream_stats = Arc::new(StreamStats::default());
        if parts.virtual_mode {
            stream_stats.set_state(StreamState::Virtual, None);
        }
        let app = Arc::new(App {
            state_dir: parts.global.state_dir(),
            global: parts.global,
            state: Mutex::new(parts.state),
            virtual_mode: parts.virtual_mode,
            input_sink: parts.input_sink,
            output: parts.output,
            delay: parts.delay,
            capture_flags: parts.capture_flags,
            engine,
            live: Mutex::new(Live {
                rms_db: -120.0,
                silent: true,
                ..Default::default()
            }),
            ws,
            stream_stats,
            frames: Arc::new(Mutex::new(LightFrame::default())),
            controller: tx,
            pair: Mutex::new(PairStatus::idle()),
            areas: Mutex::new(None),
            test_pattern: Mutex::new(None),
            sources: Arc::new(Sources::default()),
            bluetooth: parts.bluetooth,
            image: parts.image,
            streaming_area: Mutex::new(None),
        });
        (app, rx)
    }

    pub fn streaming_area(&self) -> Option<String> {
        self.streaming_area.lock().unwrap().clone()
    }

    pub fn active_pattern(&self) -> Option<Pattern> {
        self.test_pattern
            .lock()
            .unwrap()
            .filter(|(_, until)| Instant::now() < *until)
            .map(|(p, _)| p)
    }

    fn save_state(&self) -> ApiResult<()> {
        let state = self.state.lock().unwrap().clone();
        state.save(&self.state_dir).map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("saving state: {e:#}"),
            )
        })
    }

    /// Fetches the areas from the bridge (and caches them). 409 if not paired.
    pub async fn fetch_areas(&self) -> ApiResult<Vec<Area>> {
        let client = crate::web::api::client(self)?;
        let areas = client.areas().await?;
        *self.areas.lock().unwrap() = Some(areas.clone());
        Ok(areas)
    }

    /// Points the engine at the selected area's channels (or the fake layout).
    fn update_channels(&self) {
        let area_id = self.state.lock().unwrap().area_id.clone();
        let channels = area_id
            .and_then(|id| {
                self.areas
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|a| a.iter().find(|a| a.id == id).map(|a| a.channels.clone()))
            })
            .filter(|c| !c.is_empty())
            .unwrap_or_else(fake6);
        *self.engine.channels.lock().unwrap() = channels;
        self.engine.bump();
    }

    pub async fn apply_settings(&self, patch: SettingsPatch) -> ApiResult<SettingsView> {
        if let Some(e) = &patch.effect
            && effects::create(e).is_none()
        {
            return Err(bad_request(format!("unknown effect {e:?}")));
        }
        if let Some(p) = &patch.palette
            && effects::palette(p).is_none()
        {
            return Err(bad_request(format!("unknown palette {p:?}")));
        }
        for (name, v) in [
            ("intensity", patch.intensity),
            ("brightness_max", patch.brightness_max),
        ] {
            if let Some(v) = v
                && !(0.0..=1.0).contains(&v)
            {
                return Err(bad_request(format!("{name} must be 0..1")));
            }
        }
        if let Some(d) = patch.delay_ms
            && d > audio::delay::MAX_DELAY_MS
        {
            return Err(bad_request(format!(
                "delay_ms must be 0..{}",
                audio::delay::MAX_DELAY_MS
            )));
        }
        if let Some(i) = patch.idle_stop_secs
            && !(1..=3600).contains(&i)
        {
            return Err(bad_request("idle_stop_secs must be 1..3600"));
        }
        if let Some(Some(id)) = &patch.area_id {
            let areas = self.fetch_areas().await?;
            if !areas.iter().any(|a| &a.id == id) {
                return Err(bad_request(format!("no entertainment area {id}")));
            }
        }
        let view = {
            let mut s = self.state.lock().unwrap();
            if let Some(a) = patch.area_id.clone() {
                s.area_id = a;
            }
            if let Some(v) = patch.effect.clone() {
                s.effect = v;
            }
            if let Some(v) = patch.palette.clone() {
                s.palette = v;
            }
            if let Some(v) = patch.intensity {
                s.intensity = v;
            }
            if let Some(v) = patch.brightness_max {
                s.brightness_max = v;
            }
            if let Some(v) = patch.delay_ms {
                s.delay_ms = v;
            }
            if let Some(v) = patch.idle_stop_secs {
                s.idle_stop_secs = v;
            }
            *self.engine.settings.lock().unwrap() = EngineSettings {
                effect: s.effect.clone(),
                palette: s.palette.clone(),
                intensity: s.intensity,
                brightness_max: s.brightness_max,
            };
            *self.engine.idle_stop_secs.lock().unwrap() = s.idle_stop_secs as f64;
            self.delay.set_delay_ms(s.delay_ms);
            SettingsView::from(&*s)
        };
        self.engine.bump();
        if patch.area_id.is_some() {
            self.update_channels();
            let _ = self.controller.send(Ctl::Reevaluate);
        }
        self.save_state()?;
        Ok(view)
    }

    pub fn set_calibration(&self, on: bool) {
        self.capture_flags.calibration.store(on, Ordering::Relaxed);
        self.engine.calibration.store(on, Ordering::Relaxed);
        self.engine.bump();
        // Calibration needs the lights on even before the clicks count as "active" audio.
        let _ = self.controller.send(Ctl::Reevaluate);
    }

    pub fn start_pattern(&self, pattern: Pattern, secs: Duration) {
        *self.test_pattern.lock().unwrap() = Some((pattern, Instant::now() + secs));
        *self.engine.pattern.lock().unwrap() = Some(pattern);
        self.engine.bump();
        let _ = self.controller.send(Ctl::Reevaluate);
    }

    /// Clears an expired test pattern; returns whether one is running.
    fn check_pattern(&self) -> bool {
        let mut tp = self.test_pattern.lock().unwrap();
        match *tp {
            Some((_, until)) if Instant::now() >= until => {
                *tp = None;
                *self.engine.pattern.lock().unwrap() = None;
                self.engine.bump();
                false
            }
            Some(_) => true,
            None => false,
        }
    }
}

/// Pairs with the bridge in the background (`POST /api/bridge/pair`).
pub async fn run_pairing(app: Arc<App>, ip: Option<String>) {
    let result = pair_bridge(&app, ip).await;
    let mut pair = app.pair.lock().unwrap();
    *pair = match result {
        Ok(()) => PairStatus {
            state: PairState::Paired,
            message: Some("paired".into()),
            remaining_secs: 0,
            deadline: None,
        },
        Err(e) => PairStatus {
            state: PairState::Failed,
            message: Some(format!("{e:#}")),
            remaining_secs: 0,
            deadline: None,
        },
    };
}

async fn pair_bridge(app: &Arc<App>, ip: Option<String>) -> Result<()> {
    let (addr, expected) = match ip.or_else(|| app.global.bridge.clone()) {
        Some(addr) => (addr, None),
        None => {
            let found =
                tokio::task::spawn_blocking(|| hue::discovery::discover(Duration::from_secs(3)))
                    .await??;
            let b = found
                .into_iter()
                .next()
                .context("no bridge found on the network; enter its IP address")?;
            (b.ip, Some(b.bridge_id).filter(|id| !id.is_empty()))
        }
    };
    let client = HueClient::new(&addr, expected.as_deref(), None)?;
    client
        .config()
        .await
        .with_context(|| format!("contacting the bridge at {addr}"))?;
    *app.pair.lock().unwrap() =
        PairStatus::waiting(PAIR_TIMEOUT, "press the link button on the bridge");
    let creds = hue::pairing::pair(
        &client,
        &addr,
        &hue::pairing::devicetype(),
        PAIR_TIMEOUT,
        Duration::from_secs(1),
        |_| {},
    )
    .await?;
    {
        let mut s = app.state.lock().unwrap();
        s.bridge = Some(creds);
    }
    app.save_state().map_err(|e| anyhow::anyhow!(e.1))?;
    // Select the area automatically when there's exactly one.
    if let Ok(areas) = app.fetch_areas().await {
        let mut s = app.state.lock().unwrap();
        if s.area_id.is_none() && areas.len() == 1 {
            s.area_id = Some(areas[0].id.clone());
        }
    }
    let _ = app.save_state();
    app.update_channels();
    let _ = app.controller.send(Ctl::Reevaluate);
    Ok(())
}

/// The stream lifecycle: streams while audio is active (or a test pattern / calibration runs),
/// stops after `idle_stop_secs` of silence. Restarts when the area or bridge changes.
pub async fn controller(app: Arc<App>, mut rx: mpsc::UnboundedReceiver<Ctl>) {
    let mut session: Option<(StreamSession, BridgeCredentials)> = None;
    let mut audio_active = false;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let mut shutdown = None;
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(Ctl::AudioActive(a)) => audio_active = a,
                Some(Ctl::Reevaluate) => {}
                Some(Ctl::Shutdown(done)) => { shutdown = Some(done); }
                None => break,
            },
            _ = tick.tick() => {}
        }
        if shutdown.is_some() {
            break;
        }
        let pattern = app.check_pattern();
        let calibration = app.capture_flags.calibration.load(Ordering::Relaxed);
        let want = if app.virtual_mode || !(audio_active || pattern || calibration) {
            None
        } else {
            let s = app.state.lock().unwrap();
            match (&s.bridge, &s.area_id) {
                (Some(b), Some(a)) => Some((b.clone(), a.clone())),
                _ => None,
            }
        };
        let current = session
            .as_ref()
            .map(|(s, b)| (b.clone(), s.area_id().to_string()));
        if want != current {
            if let Some((s, _)) = session.take() {
                tracing::info!(area = s.area_id(), "stopping stream");
                *app.streaming_area.lock().unwrap() = None;
                s.stop().await;
            }
            if let Some((creds, area)) = want {
                match (app.global.client(&creds), app.global.dtls_target(&creds)) {
                    (Ok(client), Ok(target)) => {
                        tracing::info!(area, "starting stream");
                        *app.streaming_area.lock().unwrap() = Some(area.clone());
                        let s = StreamSession::start(
                            client,
                            target,
                            area,
                            app.frames.clone(),
                            app.stream_stats.clone(),
                        );
                        session = Some((s, creds));
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        app.stream_stats
                            .set_state(StreamState::Error, Some(format!("{e:#}")));
                    }
                }
            }
        }
    }
    if let Some((s, _)) = session.take() {
        s.stop().await;
    }
    *app.streaming_area.lock().unwrap() = None;
    if let Some(done) = shutdown {
        let _ = done.send(());
    }
}

/// Tracks whether audio is active: louder than −55 dBFS for 0.5 s starts it, quieter for
/// `idle_stop_secs` ends it.
#[derive(Debug, Default)]
pub struct Activity {
    pub active: bool,
    loud: f64,
    quiet: f64,
}

impl Activity {
    /// Feeds one analysis frame (10 ms); returns `Some(active)` on a change.
    pub fn update(&mut self, rms_db: f32, idle_stop_secs: f64) -> Option<bool> {
        let dt = 1.0 / crate::analysis::FPS as f64;
        if rms_db > SILENCE_DB {
            self.loud += dt;
            self.quiet = 0.0;
        } else {
            self.quiet += dt;
            self.loud = 0.0;
        }
        if !self.active && self.loud >= ACTIVE_AFTER_SECS - 1e-9 {
            self.active = true;
            return Some(true);
        }
        if self.active && self.quiet >= idle_stop_secs - 1e-9 {
            self.active = false;
            return Some(false);
        }
        None
    }
}

/// The engine thread: analysis ring → analyser → engine → frame slot + WebSocket.
pub fn engine_thread(
    app: Arc<App>,
    mut ring: rtrb::Consumer<f32>,
    stop: Arc<AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    Ok(std::thread::Builder::new()
        .name("engine".into())
        .spawn(move || {
            let control = app.engine.clone();
            let mut analyzer = Analyzer::new();
            let mut engine = Engine::new(fake6(), &control.settings.lock().unwrap())
                .unwrap_or_else(|_| Engine::new(fake6(), &EngineSettings::default()).unwrap());
            let mut seen = u64::MAX;
            let mut buf: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize);
            let mut features: Vec<FeatureFrame> = Vec::new();
            let mut last_data = Instant::now();
            let mut activity = Activity::default();
            let mut idle_stop = 20.0;
            let mut next_ws = f64::MIN;
            let mut onset_since_ws = 0.0f32;
            while !stop.load(Ordering::Relaxed) {
                let n = ring.slots() - ring.slots() % audio::CHANNELS;
                if n > 0 {
                    let chunk = ring.read_chunk(n).expect("slots available");
                    buf.clear();
                    let (a, b) = chunk.as_slices();
                    buf.extend_from_slice(a);
                    buf.extend_from_slice(b);
                    chunk.commit_all();
                    analyzer.push(&buf, &mut features);
                    last_data = Instant::now();
                } else if last_data.elapsed() > Duration::from_millis(100) {
                    // No capture data (nothing playing into the sink): time still passes, as silence.
                    let frames = (last_data.elapsed().as_secs_f64() * SAMPLE_RATE as f64)
                        .min(SAMPLE_RATE as f64) as usize;
                    buf.clear();
                    buf.resize(frames * audio::CHANNELS, 0.0);
                    analyzer.push(&buf, &mut features);
                    last_data = Instant::now();
                } else {
                    std::thread::sleep(Duration::from_millis(4));
                    continue;
                }
                for f in features.drain(..) {
                    let version = control.version.load(Ordering::Acquire);
                    if version != seen {
                        seen = version;
                        let _ = engine.set_settings(&control.settings.lock().unwrap());
                        engine.set_channels(control.channels.lock().unwrap().clone());
                        engine.set_calibration(control.calibration.load(Ordering::Relaxed));
                        let pattern = *control.pattern.lock().unwrap();
                        engine.set_pattern(pattern);
                        idle_stop = *control.idle_stop_secs.lock().unwrap();
                    }
                    if let Some(active) = activity.update(f.rms_db, idle_stop) {
                        tracing::info!(active, "audio activity");
                        let _ = app.controller.send(Ctl::AudioActive(active));
                    }
                    {
                        let mut live = app.live.lock().unwrap();
                        live.active = activity.active;
                        live.silent = f.silent;
                        live.rms_db = f.rms_db;
                        live.bpm = f.bpm;
                    }
                    onset_since_ws = onset_since_ws.max(f.onset.max(f.bass_onset));
                    let Some(rendered) = engine.push(&f) else {
                        continue;
                    };
                    *app.frames.lock().unwrap() = rendered.frame.clone();
                    if rendered.t >= next_ws - 1e-6 && app.ws.receiver_count() > 0 {
                        // A 50 ms schedule on the 20 ms frame grid: 20 messages/s on average.
                        next_ws = if rendered.t - next_ws > 1.0 {
                            rendered.t + WS_PERIOD_SECS
                        } else {
                            next_ws + WS_PERIOD_SECS
                        };
                        let live = LiveFrame {
                            levels: Levels {
                                rms_db: f.rms_db,
                                silent: f.silent,
                            },
                            bands: rendered.features.bands,
                            onset: onset_since_ws,
                            bpm: f.bpm,
                            channels: engine
                                .channels()
                                .iter()
                                .zip(&rendered.preview)
                                .map(|(c, rgb)| WsChannel {
                                    id: c.id,
                                    x: c.x,
                                    y: c.y,
                                    rgb: *rgb,
                                })
                                .collect(),
                        };
                        onset_since_ws = 0.0;
                        if let Ok(json) = serde_json::to_string(&live) {
                            let _ = app.ws.send(json.into());
                        }
                    }
                }
            }
        })?)
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
) -> Result<(AudioIo, DelayControl, rtrb::Consumer<f32>, String)> {
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
    #[allow(unused_mut)]
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

/// Reads the booted image from `bootc status --json` (needs root; absent elsewhere).
async fn image_info() -> Option<ImageInfo> {
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("bootc")
            .args(["status", "--json"])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let booted = &v["status"]["booted"]["image"];
    Some(ImageInfo {
        digest: booted["imageDigest"].as_str().map(str::to_string),
        version: booted["version"].as_str().map(str::to_string),
        booted: booted["image"]["image"].as_str().map(str::to_string),
    })
}

pub fn run(g: &GlobalArgs, args: ServeArgs) -> Result<()> {
    let mut state = State::load(&g.state_dir())?;
    if let Some(d) = args.delay_ms {
        state.delay_ms = d;
    }
    if let Some(i) = args.idle_stop_secs {
        state.idle_stop_secs = i;
    }
    let input_sink = args.input_sink.clone().unwrap_or(state.input_sink.clone());
    let output = args.output.clone().unwrap_or(state.output.clone());
    if output == input_sink {
        bail!("output {output:?} is the input sink: that would be a feedback loop");
    }
    let flags = Arc::new(CaptureFlags::default());
    let (io, delay, ring, output) = start_audio(
        &input_sink,
        &output,
        args.input_file.as_deref(),
        state.delay_ms,
        flags.clone(),
    )?;
    tracing::info!(
        input_sink,
        output,
        delay_ms = state.delay_ms,
        "audio running"
    );

    let rt = crate::cli::runtime()?;
    rt.block_on(async move {
        let image = image_info().await;
        let bluetooth = if args.sources {
            crate::sources::bluetooth::start().await
        } else {
            None
        };
        let (app, rx) = App::new(AppParts {
            global: g.clone(),
            state,
            virtual_mode: args.virtual_mode,
            input_sink,
            output,
            delay,
            capture_flags: flags,
            bluetooth,
            image,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let engine = engine_thread(app.clone(), ring, stop.clone())?;
        let ctl = tokio::spawn(controller(app.clone(), rx));
        if args.sources {
            crate::sources::start_watchers(app.sources.clone());
        }
        // Learn the selected area's channel layout.
        if app.state.lock().unwrap().bridge.is_some() {
            let app = app.clone();
            tokio::spawn(async move {
                for attempt in 0..30 {
                    match app.fetch_areas().await {
                        Ok(_) => {
                            app.update_channels();
                            break;
                        }
                        Err(e) => tracing::warn!(attempt, "fetching entertainment areas: {}", e.1),
                    }
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            });
        }
        let listener = tokio::net::TcpListener::bind(args.listen)
            .await
            .with_context(|| format!("binding {}", args.listen))?;
        tracing::info!("web UI on http://{}", listener.local_addr()?);
        axum::serve(listener, crate::web::router(app.clone()))
            .with_graceful_shutdown(async {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                tracing::info!("shutting down");
            })
            .await?;
        // Stop streaming cleanly so the bridge restores the lights.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let _ = app.controller.send(Ctl::Shutdown(done_tx));
        let _ = tokio::time::timeout(Duration::from_secs(5), done_rx).await;
        let _ = ctl.await;
        stop.store(true, Ordering::Relaxed);
        let _ = engine.join();
        drop(io);
        anyhow::Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_starts_after_half_a_second_and_stops_after_idle() {
        let mut a = Activity::default();
        let mut changes = Vec::new();
        for i in 0..300 {
            let db = if (100..200).contains(&i) {
                -20.0
            } else {
                -80.0
            };
            if let Some(c) = a.update(db, 0.5) {
                changes.push((i, c));
            }
        }
        assert_eq!(changes, vec![(149, true), (249, false)]);
    }
}
