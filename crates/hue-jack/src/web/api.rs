//! REST handlers under `/api`. Errors are `{"error": "..."}` with a 4xx/5xx status.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

use crate::effects::patterns::Pattern;
use crate::effects::{EFFECTS, PALETTES};
use crate::hue::clip::HueClient;
use crate::hue::discovery::{DiscoveredBridge, discover as mdns_discover};
use crate::hue::{self, Area};
use crate::serve::{App, PairState, PairStatus, SettingsView};
use crate::sources::SourcesSnapshot;
use crate::sources::bluetooth::{BluetoothStatus, DeviceInfo};

pub struct ApiError(pub StatusCode, pub String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_GATEWAY, format!("{e:#}"))
    }
}

pub fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Serialize)]
pub struct ImageView {
    pub digest: Option<String>,
    pub version: Option<String>,
    pub booted: Option<String>,
}

#[derive(Serialize)]
struct BridgeView {
    paired: bool,
    ip: Option<String>,
    bridge_id: Option<String>,
}

#[derive(Serialize)]
struct StreamView {
    state: hue::stream::StreamState,
    area_id: Option<String>,
    packets_per_sec: f64,
    packets_sent: u64,
    error: Option<String>,
}

#[derive(Serialize)]
struct AudioView {
    active: bool,
    silent: bool,
    rms_db: f32,
    bpm: Option<f32>,
    delay_ms: u32,
    fill_ms: f64,
    drift_corrections: u64,
    underruns: u64,
    input_sink: String,
    output: String,
}

#[derive(Serialize)]
struct PaletteView {
    name: &'static str,
    colors: Vec<String>,
}

#[derive(Serialize)]
pub struct StatusView {
    version: &'static str,
    image: Option<ImageView>,
    bridge: BridgeView,
    stream: StreamView,
    audio: AudioView,
    calibration: bool,
    test_pattern: Option<&'static str>,
    sources: SourcesSnapshot,
    settings: SettingsView,
    effects: Vec<&'static str>,
    palettes: Vec<PaletteView>,
    bluetooth: BluetoothStatus,
}

pub async fn status(State(app): State<Arc<App>>) -> Json<StatusView> {
    let state = app.state.lock().unwrap().clone();
    let live = app.live.lock().unwrap().clone();
    let (stream_state, error) = app.stream_stats.state();
    let delay = app.delay.stats();
    Json(StatusView {
        version: crate::VERSION,
        image: app.image.as_ref().map(|i| ImageView {
            digest: i.digest.clone(),
            version: i.version.clone(),
            booted: i.booted.clone(),
        }),
        bridge: BridgeView {
            paired: state.bridge.is_some(),
            ip: state.bridge.as_ref().map(|b| b.ip.clone()),
            bridge_id: state.bridge.as_ref().map(|b| b.bridge_id.clone()),
        },
        stream: StreamView {
            state: stream_state,
            area_id: app.streaming_area(),
            packets_per_sec: (app.stream_stats.packets_per_sec() * 10.0).round() / 10.0,
            packets_sent: app.stream_stats.packets_sent.load(Ordering::Relaxed),
            error,
        },
        audio: AudioView {
            active: live.active,
            silent: live.silent,
            rms_db: (live.rms_db * 10.0).round() / 10.0,
            bpm: live.bpm.map(|b| (b * 10.0).round() / 10.0),
            delay_ms: app.delay.delay_ms(),
            fill_ms: (app.delay.fill_ms() * 10.0).round() / 10.0,
            drift_corrections: delay.drift_corrections.load(Ordering::Relaxed),
            underruns: delay.underruns.load(Ordering::Relaxed),
            input_sink: app.input_sink.clone(),
            output: app.output.clone(),
        },
        calibration: app.capture_flags.calibration.load(Ordering::Relaxed),
        test_pattern: app.active_pattern().map(|p| p.name()),
        sources: app.sources.snapshot(),
        settings: SettingsView::from(&state),
        effects: EFFECTS.to_vec(),
        palettes: PALETTES
            .iter()
            .map(|p| PaletteView {
                name: p.name,
                colors: p.hex(),
            })
            .collect(),
        bluetooth: app
            .bluetooth
            .as_ref()
            .map(|b| b.status())
            .unwrap_or(BluetoothStatus {
                available: false,
                pairing: false,
                pairing_remaining_secs: 0,
            }),
    })
}

pub async fn discover(State(_app): State<Arc<App>>) -> ApiResult<Json<Vec<DiscoveredBridge>>> {
    let found = tokio::task::spawn_blocking(|| mdns_discover(Duration::from_secs(3)))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    Ok(Json(found))
}

#[derive(Deserialize, Default)]
pub struct PairRequest {
    ip: Option<String>,
}

pub async fn start_pair(
    State(app): State<Arc<App>>,
    body: Option<Json<PairRequest>>,
) -> ApiResult<(StatusCode, Json<PairStatus>)> {
    let ip = body
        .and_then(|Json(b)| b.ip)
        .filter(|ip| !ip.trim().is_empty());
    {
        let mut pair = app.pair.lock().unwrap();
        if pair.state == PairState::Waiting {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "pairing already in progress".into(),
            ));
        }
        *pair = PairStatus::waiting(crate::serve::PAIR_TIMEOUT, "looking for the bridge");
    }
    tokio::spawn(crate::serve::run_pairing(app.clone(), ip));
    Ok((StatusCode::ACCEPTED, Json(app.pair.lock().unwrap().view())))
}

pub async fn pair_status(State(app): State<Arc<App>>) -> Json<PairStatus> {
    Json(app.pair.lock().unwrap().view())
}

pub async fn areas(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<Area>>> {
    Ok(Json(app.fetch_areas().await?))
}

pub async fn get_settings(State(app): State<Arc<App>>) -> Json<SettingsView> {
    Json(SettingsView::from(&*app.state.lock().unwrap()))
}

fn some<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// `PUT /api/settings`: any subset of the settings. `area_id: null` deselects the area.
#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default, deserialize_with = "some")]
    pub area_id: Option<Option<String>>,
    pub effect: Option<String>,
    pub palette: Option<String>,
    pub intensity: Option<f32>,
    pub brightness_max: Option<f32>,
    pub delay_ms: Option<u32>,
    pub idle_stop_secs: Option<u32>,
}

pub async fn put_settings(
    State(app): State<Arc<App>>,
    Json(patch): Json<SettingsPatch>,
) -> ApiResult<Json<SettingsView>> {
    Ok(Json(app.apply_settings(patch).await?))
}

#[derive(Deserialize)]
pub struct CalibrationRequest {
    on: bool,
}

pub async fn calibration(
    State(app): State<Arc<App>>,
    Json(req): Json<CalibrationRequest>,
) -> Json<serde_json::Value> {
    app.set_calibration(req.on);
    Json(json!({ "on": req.on }))
}

#[derive(Deserialize)]
pub struct TestPatternRequest {
    pattern: String,
    #[serde(default = "default_pattern_secs")]
    secs: u64,
}

fn default_pattern_secs() -> u64 {
    20
}

pub async fn test_pattern(
    State(app): State<Arc<App>>,
    Json(req): Json<TestPatternRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let pattern: Pattern = req
        .pattern
        .parse()
        .map_err(|e: anyhow::Error| bad_request(e.to_string()))?;
    if !(1..=300).contains(&req.secs) {
        return Err(bad_request("secs must be 1..300"));
    }
    app.start_pattern(pattern, Duration::from_secs(req.secs));
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "pattern": pattern.name() })),
    ))
}

fn bluetooth(app: &App) -> ApiResult<&Arc<crate::sources::bluetooth::Bluetooth>> {
    app.bluetooth.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Bluetooth is not available".into(),
        )
    })
}

#[derive(Deserialize)]
pub struct PairingRequest {
    #[serde(default = "default_window")]
    seconds: u64,
}

fn default_window() -> u64 {
    120
}

pub async fn bluetooth_pairing(
    State(app): State<Arc<App>>,
    Json(req): Json<PairingRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let bt = bluetooth(&app)?;
    let st = if req.seconds == 0 {
        bt.close_window().await;
        bt.status()
    } else {
        bt.open_window(req.seconds).await?
    };
    Ok(Json(
        json!({ "pairing": st.pairing, "remaining_secs": st.pairing_remaining_secs }),
    ))
}

pub async fn bluetooth_devices(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<DeviceInfo>>> {
    Ok(Json(bluetooth(&app)?.devices().await?))
}

pub fn valid_address(a: &str) -> bool {
    let parts: Vec<&str> = a.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

pub async fn bluetooth_remove(
    State(app): State<Arc<App>>,
    Path(address): Path<String>,
) -> ApiResult<StatusCode> {
    let bt = bluetooth(&app)?;
    if !valid_address(&address) {
        return Err(bad_request(format!(
            "invalid Bluetooth address {address:?}"
        )));
    }
    bt.remove(&address.to_ascii_uppercase())
        .await
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "no such endpoint".into())
}

/// A CLIP client for the paired bridge, or 409 if not paired.
pub fn client(app: &App) -> ApiResult<HueClient> {
    let creds = app
        .state
        .lock()
        .unwrap()
        .bridge
        .clone()
        .ok_or_else(|| ApiError(StatusCode::CONFLICT, "not paired with a bridge".into()))?;
    Ok(app.global.client(&creds)?)
}
