//! Command line interface.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};

use crate::effects::patterns::{self, Pattern};
use crate::effects::to_u16;
use crate::engine::LightFrame;
use crate::hue::clip::HueClient;
use crate::hue::stream::{DtlsTarget, FrameSlot, StreamSession, StreamStats};
use crate::hue::{self, Area, BridgeCredentials, DTLS_PORT, split_host_port};
use crate::state::{State, default_state_dir};

#[derive(Debug, Parser)]
#[command(name = "hue-jack", version, about = "Music to Philips Hue appliance")]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Args)]
pub struct GlobalArgs {
    /// State directory (default: $HUEJACK_STATE_DIR, $XDG_STATE_HOME/hue-jack or ~/.local/state/hue-jack).
    #[arg(long, global = true)]
    pub state_dir: Option<PathBuf>,
    /// Bridge address `host[:port]`, overriding discovery and the paired address.
    #[arg(long, global = true)]
    pub bridge: Option<String>,
    /// DTLS port of the bridge (tests only; the real bridge uses 2100).
    #[arg(long, global = true)]
    pub dtls_port: Option<u16>,
}

impl GlobalArgs {
    pub fn state_dir(&self) -> PathBuf {
        self.state_dir.clone().unwrap_or_else(default_state_dir)
    }

    /// A CLIP client for the paired bridge (honouring `--bridge`).
    pub fn client(&self, creds: &BridgeCredentials) -> Result<HueClient> {
        let addr = self.bridge.as_deref().unwrap_or(&creds.ip);
        HueClient::new(addr, Some(&creds.bridge_id), Some(&creds.app_key))
    }

    /// The DTLS target for the paired bridge (honouring `--bridge` and `--dtls-port`).
    pub fn dtls_target(&self, creds: &BridgeCredentials) -> Result<DtlsTarget> {
        let addr = self.bridge.as_deref().unwrap_or(&creds.ip);
        let (host, _) = split_host_port(addr, hue::HTTPS_PORT);
        DtlsTarget::new(
            &host,
            self.dtls_port.unwrap_or(DTLS_PORT),
            &creds.app_id,
            &creds.client_key,
        )
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon.
    Serve(crate::serve::ServeArgs),
    /// Print bridges found via mDNS.
    Discover {
        /// Seconds to browse.
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Pair with the bridge (press its link button when prompted) and save the credentials.
    Pair {
        /// Seconds to wait for the link button.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },
    /// List entertainment areas.
    Areas,
    /// Stream a test pattern to an entertainment area.
    TestPattern {
        /// Area id (default: the selected area, or the only one).
        #[arg(long)]
        area: Option<String>,
        #[arg(long, default_value = "chase")]
        pattern: Pattern,
        #[arg(long, default_value_t = 20)]
        secs: u64,
    },
    /// Write synthetic test WAVs (clicks, drums, sweep, silence gap).
    GenTestAudio { dir: PathBuf },
    /// Print or write feature frames for a WAV file.
    Analyze {
        wav: PathBuf,
        /// Write all frames as CSV here instead of printing a summary.
        #[arg(long)]
        csv: Option<PathBuf>,
    },
    /// Render a self-contained HTML preview of the light show for a WAV file.
    Simulate(crate::simulate::SimulateArgs),
    /// Measure the delay between two WAV recordings by cross-correlation (dev tool).
    #[command(hide = true)]
    MeasureDelay {
        reference: PathBuf,
        delayed: PathBuf,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    let g = cli.global;
    match cli.command {
        Command::Serve(args) => crate::serve::run(&g, args),
        Command::Discover { wait } => {
            let bridges = hue::discovery::discover(Duration::from_secs(wait))?;
            if bridges.is_empty() {
                println!("no bridges found");
            }
            for b in bridges {
                println!("{}\t{}", b.ip, b.bridge_id);
            }
            Ok(())
        }
        Command::Pair { timeout } => runtime()?.block_on(pair(&g, Duration::from_secs(timeout))),
        Command::Areas => runtime()?.block_on(areas(&g)),
        Command::TestPattern {
            area,
            pattern,
            secs,
        } => runtime()?.block_on(test_pattern(&g, area, pattern, Duration::from_secs(secs))),
        Command::GenTestAudio { dir } => crate::audio::testgen::write_all(&dir),
        Command::Analyze { wav, csv } => crate::analysis::analyze_cmd(&wav, csv.as_deref()),
        Command::Simulate(args) => crate::simulate::run(args),
        Command::MeasureDelay { reference, delayed } => {
            let ms = crate::audio::wav::measure_delay_files(&reference, &delayed)?;
            println!("{ms:.2}");
            Ok(())
        }
    }
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?)
}

async fn pair(g: &GlobalArgs, timeout: Duration) -> Result<()> {
    let (addr, expected_id) = match &g.bridge {
        Some(addr) => (addr.clone(), None),
        None => {
            let found =
                tokio::task::spawn_blocking(|| hue::discovery::discover(Duration::from_secs(3)))
                    .await??;
            let b = found
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no bridge found; pass --bridge <ip>"))?;
            (b.ip, Some(b.bridge_id).filter(|id| !id.is_empty()))
        }
    };
    let client = HueClient::new(&addr, expected_id.as_deref(), None)?;
    let config = client
        .config()
        .await
        .with_context(|| format!("contacting bridge at {addr}"))?;
    println!("bridge {} ({}) at {addr}", config.bridgeid, config.modelid);
    println!(
        "press the round link button on the bridge now ({} s)…",
        timeout.as_secs()
    );
    let mut last = u64::MAX;
    let creds = hue::pairing::pair(
        &client,
        &addr,
        &hue::pairing::devicetype(),
        timeout,
        Duration::from_secs(1),
        |hue::pairing::PairProgress::WaitingForButton { remaining }| {
            let secs = remaining.as_secs();
            if secs / 5 != last / 5 {
                println!("  waiting… {secs} s");
                last = secs;
            }
        },
    )
    .await?;
    let dir = g.state_dir();
    let mut state = State::load(&dir)?;
    state.bridge = Some(creds);
    state.save(&dir)?;
    println!(
        "paired; credentials saved to {}",
        dir.join(crate::state::STATE_FILE).display()
    );
    Ok(())
}

fn paired(g: &GlobalArgs) -> Result<(State, BridgeCredentials)> {
    let state = State::load(&g.state_dir())?;
    let creds = state
        .bridge
        .clone()
        .ok_or_else(|| anyhow!("not paired; run `hue-jack pair` first"))?;
    Ok((state, creds))
}

async fn areas(g: &GlobalArgs) -> Result<()> {
    let (state, creds) = paired(g)?;
    let areas = g.client(&creds)?.areas().await?;
    if areas.is_empty() {
        println!(
            "no entertainment areas; create one in the Hue app (Settings → Entertainment areas)"
        );
    }
    for a in areas {
        let selected = if state.area_id.as_deref() == Some(&a.id) {
            " (selected)"
        } else {
            ""
        };
        println!(
            "{}  {}  [{}] {} channels{selected}",
            a.id,
            a.name,
            a.status,
            a.channels.len()
        );
        for c in &a.channels {
            println!(
                "    channel {:>2}  x {:+.2}  y {:+.2}  z {:+.2}",
                c.id, c.x, c.y, c.z
            );
        }
    }
    Ok(())
}

/// Picks `wanted`, else the selected area, else the only area.
pub fn choose_area(areas: Vec<Area>, wanted: Option<&str>) -> Result<Area> {
    if let Some(id) = wanted {
        return areas
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| anyhow!("no entertainment area {id}"));
    }
    match areas.len() {
        0 => bail!("no entertainment areas; create one in the Hue app"),
        1 => Ok(areas.into_iter().next().unwrap()),
        _ => bail!("several entertainment areas; pass --area <id> (see `hue-jack areas`)"),
    }
}

async fn test_pattern(
    g: &GlobalArgs,
    area: Option<String>,
    pattern: Pattern,
    secs: Duration,
) -> Result<()> {
    let (state, creds) = paired(g)?;
    let client = g.client(&creds)?;
    let wanted = area.or(state.area_id.clone());
    let area = choose_area(client.areas().await?, wanted.as_deref())?;
    println!(
        "streaming {} to {} ({}) for {} s",
        pattern.name(),
        area.name,
        area.id,
        secs.as_secs()
    );
    let frames: FrameSlot = Arc::new(Mutex::new(LightFrame::default()));
    let stats = Arc::new(StreamStats::default());
    let session = StreamSession::start(
        client,
        g.dtls_target(&creds)?,
        area.id.clone(),
        frames.clone(),
        stats.clone(),
    );
    let start = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    let mut last_identified = None;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    while start.elapsed() < secs {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = &mut ctrl_c => break,
        }
        let t = start.elapsed().as_secs_f32();
        let colours = patterns::render(pattern, &area.channels, t);
        let frame = LightFrame {
            channels: area
                .channels
                .iter()
                .zip(colours)
                .map(|(c, rgb)| (c.id, to_u16(rgb, state.brightness_max)))
                .collect(),
        };
        *frames.lock().unwrap() = frame;
        if pattern == Pattern::Identify {
            let i = patterns::identify_index(&area.channels, t);
            if i != last_identified {
                if let Some(c) = i.map(|i| &area.channels[i]) {
                    println!(
                        "channel {:>2}  x {:+.2}  y {:+.2}  z {:+.2}",
                        c.id, c.x, c.y, c.z
                    );
                }
                last_identified = i;
            }
        }
    }
    session.stop().await;
    let (st, err) = stats.state();
    println!(
        "sent {} packets ({} errors){}",
        stats
            .packets_sent
            .load(std::sync::atomic::Ordering::Relaxed),
        stats.errors.load(std::sync::atomic::Ordering::Relaxed),
        err.map(|e| format!("; last state {st:?}: {e}"))
            .unwrap_or_default()
    );
    Ok(())
}
