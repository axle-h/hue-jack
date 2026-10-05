//! Entertainment streaming: DTLS 1.2 PSK client and the 50 Hz stream loop.
//!
//! [`StreamSession`] owns the lifecycle: `PUT {"action":"start"}`, DTLS handshake, send the latest
//! [`LightFrame`] at 50 Hz (resending during quiet passages so the bridge's ~10 s timeout never trips),
//! and on any error `stop` → back off 1/2/5 s → `start` + re-handshake.

use std::io::{self, Read, Write};
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use foreign_types::ForeignTypeRef;
use openssl::ssl::{
    ErrorCode, Ssl, SslContext, SslContextBuilder, SslMethod, SslOptions, SslStream, SslVerifyMode,
    SslVersion,
};
use serde::Serialize;

use super::clip::HueClient;
use super::huestream;
use crate::engine::LightFrame;

/// Stream rate towards the bridge.
pub const STREAM_HZ: f64 = 50.0;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
/// `DTLS_CTRL_HANDLE_TIMEOUT` from `ssl.h` (`DTLSv1_handle_timeout` is a macro, not exported by openssl-sys).
const DTLS_CTRL_HANDLE_TIMEOUT: libc::c_int = 74;

/// Where and how to open the DTLS stream.
#[derive(Clone, Debug)]
pub struct DtlsTarget {
    pub host: String,
    pub port: u16,
    /// PSK identity: the `hue-application-id`.
    pub identity: String,
    /// PSK: the hex-decoded `clientkey`.
    pub psk: Vec<u8>,
}

impl DtlsTarget {
    pub fn new(host: &str, port: u16, app_id: &str, client_key_hex: &str) -> Result<Self> {
        let psk = hex::decode(client_key_hex).context("clientkey is not hex")?;
        Ok(Self {
            host: host.to_string(),
            port,
            identity: app_id.to_string(),
            psk,
        })
    }
}

/// A connected UDP socket as a byte stream for OpenSSL: one write is one datagram, one read is one datagram.
#[derive(Debug)]
pub struct UdpIo(pub UdpSocket);

impl Read for UdpIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.recv(buf)
    }
}

impl Write for UdpIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.send(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn client_context(identity: &str, psk: &[u8]) -> Result<SslContext> {
    let mut ctx = SslContextBuilder::new(SslMethod::dtls_client())?;
    // Fedora's crypto policy doesn't list PSK suites by default; set them and the level explicitly.
    ctx.set_security_level(1);
    ctx.set_cipher_list("PSK-AES128-GCM-SHA256")?;
    ctx.set_min_proto_version(Some(SslVersion::DTLS1_2))?;
    ctx.set_max_proto_version(Some(SslVersion::DTLS1_2))?;
    ctx.set_verify(SslVerifyMode::NONE);
    ctx.set_options(SslOptions::NO_QUERY_MTU);
    let identity = identity.as_bytes().to_vec();
    let psk = psk.to_vec();
    ctx.set_psk_client_callback(move |_ssl, _hint, identity_out, psk_out| {
        if identity.len() + 1 > identity_out.len() || psk.len() > psk_out.len() {
            return Err(openssl::error::ErrorStack::get());
        }
        identity_out[..identity.len()].copy_from_slice(&identity);
        identity_out[identity.len()] = 0;
        psk_out[..psk.len()].copy_from_slice(&psk);
        Ok(psk.len())
    });
    Ok(ctx.build())
}

/// Opens a DTLS 1.2 PSK session to the bridge, retransmitting handshake flights on timeout.
pub fn connect_dtls(target: &DtlsTarget, timeout: Duration) -> Result<SslStream<UdpIo>> {
    let addr = (target.host.as_str(), target.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("cannot resolve {}", target.host))?;
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let sock = UdpSocket::bind(bind)?;
    sock.connect(addr)?;
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let ctx = client_context(&target.identity, &target.psk)?;
    let mut ssl = Ssl::new(&ctx)?;
    ssl.set_mtu(1400)?;
    ssl.set_connect_state();
    let mut stream = SslStream::new(ssl, UdpIo(sock))?;
    let deadline = Instant::now() + timeout;
    loop {
        match stream.do_handshake() {
            Ok(()) => {
                stream.get_ref().0.set_read_timeout(None)?;
                return Ok(stream);
            }
            Err(e) if e.code() == ErrorCode::WANT_READ || e.code() == ErrorCode::WANT_WRITE => {
                if Instant::now() >= deadline {
                    bail!("DTLS handshake with {addr} timed out");
                }
                // SAFETY: a valid SSL owned by `stream`; the ctrl only retransmits a flight if its timer expired.
                unsafe {
                    openssl_sys::SSL_ctrl(
                        stream.ssl().as_ptr(),
                        DTLS_CTRL_HANDLE_TIMEOUT,
                        0,
                        std::ptr::null_mut(),
                    );
                }
            }
            Err(e) => bail!("DTLS handshake with {addr} failed: {e}"),
        }
    }
}

/// Streaming state, shared with the web API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StreamState {
    #[default]
    Idle,
    Starting,
    Streaming,
    Stopping,
    Error,
    Virtual,
}

/// Counters for `/api/status`.
#[derive(Debug, Default)]
pub struct StreamStats {
    pub packets_sent: AtomicU64,
    pub errors: AtomicU64,
    state: Mutex<(StreamState, Option<String>)>,
    rate: Mutex<RateMeter>,
}

#[derive(Debug, Default)]
struct RateMeter {
    window_start: Option<Instant>,
    count: u64,
    per_sec: f64,
}

impl StreamStats {
    pub fn state(&self) -> (StreamState, Option<String>) {
        self.state.lock().unwrap().clone()
    }
    pub fn set_state(&self, state: StreamState, error: Option<String>) {
        *self.state.lock().unwrap() = (state, error);
    }
    /// Packets per second over the last full second (0 when not streaming).
    pub fn packets_per_sec(&self) -> f64 {
        let rate = self.rate.lock().unwrap();
        match rate.window_start {
            Some(start) if start.elapsed() < Duration::from_secs(2) => rate.per_sec,
            _ => 0.0,
        }
    }
    fn count_packet(&self) {
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        let mut rate = self.rate.lock().unwrap();
        let now = Instant::now();
        let start = *rate.window_start.get_or_insert(now);
        rate.count += 1;
        let elapsed = now - start;
        if elapsed >= Duration::from_secs(1) {
            rate.per_sec = rate.count as f64 / elapsed.as_secs_f64();
            rate.count = 0;
            rate.window_start = Some(now);
        }
    }
}

/// The frame the stream loop sends next; written by whoever renders light frames.
pub type FrameSlot = Arc<Mutex<LightFrame>>;

/// Sends the latest frame at [`STREAM_HZ`] until `cancel` is set (Ok) or a write fails (Err).
pub fn run_stream(
    target: &DtlsTarget,
    area_id: &str,
    frames: &FrameSlot,
    stats: &StreamStats,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut stream = connect_dtls(target, HANDSHAKE_TIMEOUT)?;
    stats.set_state(StreamState::Streaming, None);
    tracing::info!(area = area_id, "DTLS stream open");
    let period = Duration::from_secs_f64(1.0 / STREAM_HZ);
    let mut next = Instant::now();
    let mut seq: u8 = 0;
    let mut packet = Vec::with_capacity(huestream::HEADER_LEN + 20 * huestream::CHANNEL_LEN);
    let mut channels = Vec::with_capacity(20);
    while !cancel.load(Ordering::Relaxed) {
        channels.clear();
        channels.extend_from_slice(&frames.lock().unwrap().channels);
        channels.truncate(huestream::MAX_CHANNELS);
        huestream::encode_into(&mut packet, seq, area_id, &channels)?;
        match stream.ssl_write(&packet) {
            Ok(_) => stats.count_packet(),
            Err(e) if e.code() == ErrorCode::WANT_WRITE => {}
            Err(e) => return Err(anyhow!("DTLS write failed: {e}")),
        }
        seq = seq.wrapping_add(1);
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now; // fell behind; don't burst
        }
    }
    let _ = stream.shutdown();
    Ok(())
}

/// A running stream to one entertainment area, with start/stop and automatic recovery.
pub struct StreamSession {
    cancel: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
    area_id: String,
}

impl StreamSession {
    /// Starts streaming `frames` to `area_id`. Must be called inside a tokio runtime.
    pub fn start(
        client: HueClient,
        target: DtlsTarget,
        area_id: String,
        frames: FrameSlot,
        stats: Arc<StreamStats>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(supervise(
            client,
            target,
            area_id.clone(),
            frames,
            stats,
            cancel.clone(),
        ));
        Self {
            cancel,
            task,
            area_id,
        }
    }

    pub fn area_id(&self) -> &str {
        &self.area_id
    }

    /// Stops sending, sends `{"action":"stop"}` and waits for the session to finish.
    pub async fn stop(self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.task.await;
    }
}

async fn supervise(
    client: HueClient,
    target: DtlsTarget,
    area_id: String,
    frames: FrameSlot,
    stats: Arc<StreamStats>,
    cancel: Arc<AtomicBool>,
) {
    let mut failures = 0usize;
    while !cancel.load(Ordering::Relaxed) {
        stats.set_state(StreamState::Starting, None);
        let result = match client.set_streaming(&area_id, true).await {
            Ok(()) => {
                let (target, area, frames, stats, cancel) = (
                    target.clone(),
                    area_id.clone(),
                    frames.clone(),
                    stats.clone(),
                    cancel.clone(),
                );
                let started = Instant::now();
                let r = tokio::task::spawn_blocking(move || {
                    run_stream(&target, &area, &frames, &stats, &cancel)
                })
                .await
                .unwrap_or_else(|e| Err(anyhow!("stream thread panicked: {e}")));
                if started.elapsed() > Duration::from_secs(30) {
                    failures = 0;
                }
                r
            }
            Err(e) => Err(e.context("starting entertainment area")),
        };
        match result {
            Ok(()) => break,
            Err(e) => {
                stats.errors.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error = %format!("{e:#}"), "stream failed; restarting");
                stats.set_state(StreamState::Error, Some(format!("{e:#}")));
                let _ = client.set_streaming(&area_id, false).await;
                let delay = BACKOFF[failures.min(BACKOFF.len() - 1)];
                failures += 1;
                let until = Instant::now() + delay;
                while Instant::now() < until && !cancel.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
    stats.set_state(StreamState::Stopping, None);
    if let Err(e) = client.set_streaming(&area_id, false).await {
        tracing::warn!(error = %format!("{e:#}"), "stopping entertainment area failed");
    }
    stats.set_state(StreamState::Idle, None);
    tracing::info!(area = area_id, "stream stopped");
}
