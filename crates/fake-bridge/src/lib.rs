//! A fake Hue bridge for tests: CLIP v2 over HTTPS and a DTLS-PSK HueStream receiver.
//!
//! It implements just enough of the bridge for hue-jack: `/api/0/config`, `POST /api` (pairing, gated
//! by [`FakeBridge::press_link_button`]), `/auth/v1`, `entertainment_configuration` GET/PUT and `light`
//! GET/PUT. The DTLS receiver only accepts a handshake while the area is started, like the real bridge,
//! and records every HueStream packet it decodes. Like the real bridge, `stop` leaves the lights on the
//! last streamed frame (here: off, brightness 0).

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_server::tls_openssl::{OpenSSLAcceptor, OpenSSLConfig};
use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::PKey;
use openssl::ssl::{
    ErrorCode, Ssl, SslContextBuilder, SslMethod, SslOptions, SslStream, SslVerifyMode,
};
use openssl::x509::{X509, X509NameBuilder};
use parking_lot::Mutex;
use serde_json::{Value, json};

/// The fixed id of the fake entertainment area.
pub const AREA_ID: &str = "8a3c6f4e-6f2b-4c1d-9b7a-2f1e5d4c3b2a";
/// The fake bridge id (certificate CN, lowercase).
pub const BRIDGE_ID: &str = "001788fffe123456";
/// How long the link button stays "pressed".
pub const LINK_WINDOW: Duration = Duration::from_secs(30);
/// The bridge drops a stream after this long without packets.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// One decoded HueStream packet.
#[derive(Clone, Debug)]
pub struct Packet {
    pub at: Instant,
    pub seq: u8,
    pub config_id: String,
    pub channels: Vec<(u8, [u16; 3])>,
}

/// Something that happened on the CLIP side, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    PairRefused,
    Paired,
    Start(String),
    Stop(String),
    DtlsConnected,
    DtlsClosed,
}

#[derive(Default)]
struct Inner {
    link_pressed_at: Option<Instant>,
    app_key: Option<String>,
    client_key: Option<String>,
    app_id: Option<String>,
    active: bool,
    events: Vec<(Instant, Event)>,
    packets: Vec<Packet>,
    lights: Vec<Value>,
}

#[derive(Clone)]
struct Shared(Arc<Mutex<Inner>>);

impl Shared {
    fn event(&self, e: Event) {
        self.0.lock().events.push((Instant::now(), e));
    }
}

pub struct FakeBridge {
    shared: Shared,
    https_addr: SocketAddr,
    dtls_addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: axum_server::Handle<SocketAddr>,
}

impl FakeBridge {
    /// Starts the fake bridge on 127.0.0.1 with ephemeral ports. Needs a tokio runtime.
    pub async fn start() -> Result<Self> {
        Self::start_on("127.0.0.1:0".parse()?, "127.0.0.1:0".parse()?).await
    }

    /// Starts the fake bridge on the given HTTPS and DTLS addresses (port 0 = ephemeral).
    pub async fn start_on(https: SocketAddr, dtls: SocketAddr) -> Result<Self> {
        let shared = Shared(Arc::new(Mutex::new(Inner {
            lights: initial_lights(),
            ..Inner::default()
        })));
        let (cert_pem, key_pem) = self_signed(BRIDGE_ID)?;
        let config = OpenSSLConfig::from_pem(&cert_pem, &key_pem).context("TLS config")?;
        let listener = TcpListener::bind(https)?;
        listener.set_nonblocking(true)?;
        let https_addr = listener.local_addr()?;
        let app = router(shared.clone());
        let handle = axum_server::Handle::new();
        let server = axum_server::from_tcp(listener)?
            .acceptor(OpenSSLAcceptor::new(config))
            .handle(handle.clone());
        tokio::spawn(async move {
            if let Err(e) = server.serve(app.into_make_service()).await {
                tracing::error!("fake bridge HTTPS server failed: {e}");
            }
        });

        let udp = UdpSocket::bind(dtls)?;
        let dtls_addr = udp.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        {
            let shared = shared.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("fake-bridge-dtls".into())
                .spawn(move || dtls_server(udp, shared, stop))?;
        }
        Ok(Self {
            shared,
            https_addr,
            dtls_addr,
            stop,
            handle,
        })
    }

    pub fn https_addr(&self) -> SocketAddr {
        self.https_addr
    }
    pub fn dtls_addr(&self) -> SocketAddr {
        self.dtls_addr
    }
    /// Press the link button: pairing succeeds for the next [`LINK_WINDOW`].
    pub fn press_link_button(&self) {
        self.shared.0.lock().link_pressed_at = Some(Instant::now());
    }
    pub fn packets(&self) -> Vec<Packet> {
        self.shared.0.lock().packets.clone()
    }
    pub fn events(&self) -> Vec<Event> {
        self.shared
            .0
            .lock()
            .events
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }
    pub fn timed_events(&self) -> Vec<(Instant, Event)> {
        self.shared.0.lock().events.clone()
    }
    pub fn is_active(&self) -> bool {
        self.shared.0.lock().active
    }
    /// The credentials handed out at pairing: `(app_key, client_key, app_id)`.
    pub fn credentials(&self) -> Option<(String, String, String)> {
        let inner = self.shared.0.lock();
        Some((
            inner.app_key.clone()?,
            inner.client_key.clone()?,
            inner.app_id.clone()?,
        ))
    }
    /// The CLIP `light` resources of the area's lights, in channel order.
    pub fn lights(&self) -> Vec<Value> {
        self.shared.0.lock().lights.clone()
    }
    /// Pre-pairs without the button, returning `(app_key, client_key, app_id)`.
    pub fn pre_pair(&self) -> (String, String, String) {
        let mut inner = self.shared.0.lock();
        issue_credentials(&mut inner);
        (
            inner.app_key.clone().unwrap(),
            inner.client_key.clone().unwrap(),
            inner.app_id.clone().unwrap(),
        )
    }
}

impl Drop for FakeBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.shutdown();
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    openssl::rand::rand_bytes(&mut buf).expect("rand");
    hex::encode(buf)
}

fn issue_credentials(inner: &mut Inner) {
    if inner.app_key.is_none() {
        inner.app_key = Some(random_hex(20));
        inner.client_key = Some(random_hex(16).to_uppercase());
        let id = random_hex(16);
        inner.app_id = Some(format!(
            "{}-{}-{}-{}-{}",
            &id[..8],
            &id[8..12],
            &id[12..16],
            &id[16..20],
            &id[20..]
        ));
    }
}

fn self_signed(cn: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
    let key = PKey::from_ec_key(EcKey::generate(&group)?)?;
    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_nid(Nid::COUNTRYNAME, "NL")?;
    name.append_entry_by_nid(Nid::ORGANIZATIONNAME, "Philips Hue")?;
    name.append_entry_by_nid(Nid::COMMONNAME, cn)?;
    let name = name.build();
    let mut b = X509::builder()?;
    b.set_version(2)?;
    let mut serial = BigNum::new()?;
    serial.rand(64, MsbOption::MAYBE_ZERO, false)?;
    let serial = serial.to_asn1_integer()?;
    b.set_serial_number(&serial)?;
    b.set_subject_name(&name)?;
    b.set_issuer_name(&name)?;
    b.set_pubkey(&key)?;
    let (not_before, not_after) = (Asn1Time::days_from_now(0)?, Asn1Time::days_from_now(3650)?);
    b.set_not_before(&not_before)?;
    b.set_not_after(&not_after)?;
    b.sign(&key, MessageDigest::sha256())?;
    Ok((b.build().to_pem()?, key.private_key_to_pem_pkcs8()?))
}

fn light_id(i: usize) -> String {
    format!("11111111-0000-0000-0000-00000000000{i}")
}

/// Six colour lights in a mix of states: on in colour temperature, on in xy, and off.
fn initial_lights() -> Vec<Value> {
    (0..6)
        .map(|i| {
            let (on, ct) = (i % 3 != 2, i % 2 == 0);
            json!({
                "id": light_id(i),
                "type": "light",
                "metadata": {"name": format!("Fake light {i}")},
                "on": {"on": on},
                "dimming": {"brightness": 20.0 + 10.0 * i as f64},
                "color_temperature": {"mirek": if ct { json!(250 + 20 * i) } else { Value::Null }, "mirek_valid": ct},
                "color": {"xy": {"x": 0.3 + 0.01 * i as f64, "y": 0.3}},
            })
        })
        .collect()
}

/// What the real bridge leaves behind after a stream: the last frame (here, all black).
fn blank_lights(lights: &mut [Value]) {
    for l in lights {
        l["on"] = json!({"on": false});
        l["dimming"]["brightness"] = json!(0.0);
        l["color"]["xy"] = json!({"x": 0.1532, "y": 0.0475});
        l["color_temperature"] = json!({"mirek": null, "mirek_valid": false});
    }
}

fn area_json(active: bool) -> Value {
    // Six channels around a living room: a row of three at the front and three behind.
    let positions = [
        (-0.8, 0.8, 0.0),
        (0.0, 1.0, 0.4),
        (0.8, 0.8, 0.0),
        (-0.8, -0.6, 0.0),
        (0.0, -0.8, 0.4),
        (0.8, -0.6, 0.0),
    ];
    let channels: Vec<Value> = positions
        .iter()
        .enumerate()
        .map(|(i, (x, y, z))| {
            json!({
                "channel_id": i,
                "position": {"x": x, "y": y, "z": z},
                "members": [{"service": {"rid": format!("00000000-0000-0000-0000-00000000000{i}"), "rtype": "entertainment"}, "index": 0}]
            })
        })
        .collect();
    json!({
        "id": AREA_ID,
        "type": "entertainment_configuration",
        "metadata": {"name": "Fake living room"},
        "configuration_type": "screen",
        "status": if active { "active" } else { "inactive" },
        "channels": channels,
        "light_services": (0..6).map(|i| json!({"rid": light_id(i), "rtype": "light"})).collect::<Vec<_>>(),
    })
}

fn router(shared: Shared) -> Router {
    Router::new()
        .route("/api/0/config", get(config))
        .route("/api", post(create_user))
        .route("/auth/v1", get(auth))
        .route(
            "/clip/v2/resource/entertainment_configuration",
            get(list_areas),
        )
        .route(
            "/clip/v2/resource/entertainment_configuration/{id}",
            get(get_area).put(put_area),
        )
        .route("/clip/v2/resource/light", get(list_lights))
        .route(
            "/clip/v2/resource/light/{id}",
            axum::routing::put(put_light),
        )
        .with_state(shared)
}

async fn list_lights(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if !authorised(&s, &headers) {
        return forbidden();
    }
    let lights = s.0.lock().lights.clone();
    Json(json!({"errors": [], "data": lights})).into_response()
}

async fn put_light(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if !authorised(&s, &headers) {
        return forbidden();
    }
    let mut inner = s.0.lock();
    let Some(light) = inner.lights.iter_mut().find(|l| l["id"] == id.as_str()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"errors": [{"description": "Not found"}], "data": []})),
        )
            .into_response();
    };
    if let Some(on) = body.get("on") {
        light["on"] = on.clone();
    }
    if let Some(b) = body.pointer("/dimming/brightness") {
        light["dimming"]["brightness"] = b.clone();
    }
    if let Some(xy) = body.pointer("/color/xy") {
        light["color"]["xy"] = xy.clone();
        light["color_temperature"] = json!({"mirek": null, "mirek_valid": false});
    }
    if let Some(m) = body.pointer("/color_temperature/mirek") {
        light["color_temperature"] = json!({"mirek": m, "mirek_valid": true});
    }
    Json(json!({"errors": [], "data": [{"rid": id, "rtype": "light"}]})).into_response()
}

async fn config() -> Json<Value> {
    Json(json!({
        "name": "Fake Bridge", "datastoreversion": "197", "swversion": "1978293000", "apiversion": "1.78.0",
        "mac": "00:17:88:12:34:56", "bridgeid": BRIDGE_ID.to_uppercase(), "factorynew": false,
        "replacesbridgeid": null, "modelid": "BSB002", "starterkitid": ""
    }))
}

async fn create_user(State(s): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
    if body.get("devicetype").and_then(Value::as_str).is_none() {
        return Json(
            json!([{"error": {"type": 5, "address": "/", "description": "invalid/missing parameters in body"}}]),
        );
    }
    let mut inner = s.0.lock();
    let pressed = inner
        .link_pressed_at
        .is_some_and(|t| t.elapsed() < LINK_WINDOW);
    if !pressed {
        inner.events.push((Instant::now(), Event::PairRefused));
        return Json(
            json!([{"error": {"type": 101, "address": "", "description": "link button not pressed"}}]),
        );
    }
    issue_credentials(&mut inner);
    inner.events.push((Instant::now(), Event::Paired));
    Json(json!([{"success": {"username": inner.app_key, "clientkey": inner.client_key}}]))
}

fn authorised(s: &Shared, headers: &HeaderMap) -> bool {
    let key = headers
        .get("hue-application-key")
        .and_then(|v| v.to_str().ok());
    key.is_some() && s.0.lock().app_key.as_deref() == key
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"errors": [{"description": "Forbidden"}], "data": []})),
    )
        .into_response()
}

async fn auth(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if !authorised(&s, &headers) {
        return forbidden();
    }
    let app_id = s.0.lock().app_id.clone().unwrap_or_default();
    ([("hue-application-id", app_id)], StatusCode::OK).into_response()
}

async fn list_areas(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if !authorised(&s, &headers) {
        return forbidden();
    }
    let active = s.0.lock().active;
    Json(json!({"errors": [], "data": [area_json(active)]})).into_response()
}

async fn get_area(State(s): State<Shared>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if !authorised(&s, &headers) {
        return forbidden();
    }
    if id != AREA_ID {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"errors": [{"description": "Not found"}], "data": []})),
        )
            .into_response();
    }
    let active = s.0.lock().active;
    Json(json!({"errors": [], "data": [area_json(active)]})).into_response()
}

async fn put_area(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if !authorised(&s, &headers) {
        return forbidden();
    }
    if id != AREA_ID {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"errors": [{"description": "Not found"}], "data": []})),
        )
            .into_response();
    }
    let mut inner = s.0.lock();
    match body.get("action").and_then(Value::as_str) {
        Some("start") => {
            inner.active = true;
            inner
                .events
                .push((Instant::now(), Event::Start(id.clone())));
        }
        Some("stop") => {
            inner.active = false;
            blank_lights(&mut inner.lights);
            inner.events.push((Instant::now(), Event::Stop(id.clone())));
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"errors": [{"description": "invalid action"}], "data": []})),
            )
                .into_response();
        }
    }
    Json(json!({"errors": [], "data": [{"rid": id, "rtype": "entertainment_configuration"}]}))
        .into_response()
}

/// Per-channel 16-bit RGB values.
pub type Channels = Vec<(u8, [u16; 3])>;

/// Decodes a HueStream v2 RGB packet. Independent of hue-jack's encoder on purpose.
pub fn decode(packet: &[u8]) -> Result<(u8, String, Channels)> {
    if packet.len() < 52 || &packet[..9] != b"HueStream" {
        bail!("not a HueStream packet");
    }
    if packet[9] != 2 || packet[10] != 0 {
        bail!("unsupported version {}.{}", packet[9], packet[10]);
    }
    if packet[14] != 0 {
        bail!("unsupported colour space {}", packet[14]);
    }
    let seq = packet[11];
    let config_id = std::str::from_utf8(&packet[16..52])?.to_string();
    let body = &packet[52..];
    if !body.len().is_multiple_of(7) || body.len() / 7 > 20 {
        bail!("bad channel data length {}", body.len());
    }
    let channels = body
        .as_chunks::<7>()
        .0
        .iter()
        .map(|c| {
            (
                c[0],
                [
                    u16::from_be_bytes([c[1], c[2]]),
                    u16::from_be_bytes([c[3], c[4]]),
                    u16::from_be_bytes([c[5], c[6]]),
                ],
            )
        })
        .collect();
    Ok((seq, config_id, channels))
}

/// An unconnected UDP socket bound to one peer: reads drop datagrams from anyone else.
struct PeerIo {
    sock: UdpSocket,
    peer: SocketAddr,
    pending: Option<Vec<u8>>,
}

impl Read for PeerIo {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(first) = self.pending.take() {
            let n = first.len().min(buf.len());
            buf[..n].copy_from_slice(&first[..n]);
            return Ok(n);
        }
        loop {
            let (n, from) = self.sock.recv_from(buf)?;
            if from == self.peer {
                return Ok(n);
            }
        }
    }
}

impl Write for PeerIo {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sock.send_to(buf, self.peer)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn dtls_server(udp: UdpSocket, shared: Shared, stop: Arc<AtomicBool>) {
    udp.set_read_timeout(Some(Duration::from_millis(100)))
        .expect("udp timeout");
    let mut buf = vec![0u8; 2048];
    while !stop.load(Ordering::Relaxed) {
        let (n, peer) = match udp.recv_from(&mut buf) {
            Ok(r) => r,
            Err(_) => continue,
        };
        // Like the real bridge: no handshake unless the area has been started.
        let creds = {
            let inner = shared.0.lock();
            match (inner.active, &inner.app_id, &inner.client_key) {
                (true, Some(id), Some(key)) => Some((id.clone(), key.clone())),
                _ => None,
            }
        };
        let Some((app_id, client_key)) = creds else {
            continue;
        };
        let io = PeerIo {
            sock: udp.try_clone().expect("clone udp"),
            peer,
            pending: Some(buf[..n].to_vec()),
        };
        if let Err(e) = dtls_session(io, &app_id, &client_key, &shared, &stop) {
            tracing::debug!("fake bridge DTLS session ended: {e:#}");
        }
        shared.event(Event::DtlsClosed);
    }
}

fn dtls_session(
    io: PeerIo,
    app_id: &str,
    client_key: &str,
    shared: &Shared,
    stop: &AtomicBool,
) -> Result<()> {
    let mut ctx = SslContextBuilder::new(SslMethod::dtls_server())?;
    ctx.set_security_level(1);
    ctx.set_cipher_list("PSK-AES128-GCM-SHA256")?;
    ctx.set_verify(SslVerifyMode::NONE);
    ctx.set_options(SslOptions::NO_QUERY_MTU);
    let expected_identity = app_id.as_bytes().to_vec();
    let psk = hex::decode(client_key)?;
    ctx.set_psk_server_callback(move |_ssl, identity, psk_out| {
        if identity != Some(expected_identity.as_slice()) {
            return Err(openssl::error::ErrorStack::get());
        }
        psk_out[..psk.len()].copy_from_slice(&psk);
        Ok(psk.len())
    });
    let ctx = ctx.build();
    let mut ssl = Ssl::new(&ctx)?;
    ssl.set_mtu(1400)?;
    ssl.set_accept_state();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = SslStream::new(ssl, io)?;
    loop {
        match stream.do_handshake() {
            Ok(()) => break,
            Err(e) if e.code() == ErrorCode::WANT_READ || e.code() == ErrorCode::WANT_WRITE => {
                if Instant::now() > deadline || stop.load(Ordering::Relaxed) {
                    bail!("handshake timed out");
                }
            }
            Err(e) => bail!("handshake failed: {e}"),
        }
    }
    shared.event(Event::DtlsConnected);
    let mut last = Instant::now();
    let mut buf = vec![0u8; 2048];
    loop {
        if stop.load(Ordering::Relaxed) || !shared.0.lock().active {
            return Ok(());
        }
        if last.elapsed() > STREAM_IDLE_TIMEOUT {
            bail!("stream idle timeout");
        }
        match stream.ssl_read(&mut buf) {
            Ok(n) => {
                last = Instant::now();
                match decode(&buf[..n]) {
                    Ok((seq, config_id, channels)) => {
                        shared.0.lock().packets.push(Packet {
                            at: last,
                            seq,
                            config_id,
                            channels,
                        });
                    }
                    Err(e) => tracing::warn!("fake bridge: bad packet: {e}"),
                }
            }
            Err(e) if e.code() == ErrorCode::WANT_READ => {}
            Err(e) if e.code() == ErrorCode::ZERO_RETURN => return Ok(()),
            Err(e) => bail!("read failed: {e}"),
        }
    }
}
