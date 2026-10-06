//! M5 acceptance: every REST endpoint, the WebSocket and the embedded UI, in-process.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fake_bridge::{AREA_ID, BRIDGE_ID, FakeBridge};
use futures::StreamExt;
use hue_jack::audio::{CaptureFlags, delay_line};
use hue_jack::cli::GlobalArgs;
use hue_jack::serve::{App, AppParts, controller, engine_thread};
use hue_jack::sources::bluetooth::{Bluetooth, DeviceInfo, MockAdapter, PairingWindow};
use hue_jack::state::State;
use serde_json::{Value, json};
use tower::ServiceExt;

struct Harness {
    app: Arc<App>,
    dir: std::path::PathBuf,
    adapter: Arc<MockAdapter>,
}

fn harness(state: State, bluetooth: bool, dtls_port: Option<u16>) -> Harness {
    let dir = std::env::temp_dir().join(format!(
        "hue-jack-api-{}-{}",
        std::process::id(),
        rand_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (_w, _r, delay) = delay_line(state.delay_ms);
    let adapter = Arc::new(MockAdapter::default());
    *adapter.devices.lock().unwrap() = vec![DeviceInfo {
        address: "AA:BB:CC:DD:EE:01".into(),
        name: Some("Phone".into()),
        connected: true,
        paired: true,
        trusted: true,
    }];
    let bt = bluetooth.then(|| Bluetooth::new(adapter.clone(), Arc::new(PairingWindow::default())));
    let global = GlobalArgs {
        state_dir: Some(dir.clone()),
        bridge: None,
        dtls_port,
    };
    let (app, rx) = App::new(AppParts {
        global,
        state,
        virtual_mode: false,
        input_sink: "hue-jack-in".into(),
        output: "null".into(),
        delay,
        capture_flags: Arc::new(CaptureFlags::default()),
        bluetooth: bt,
        image: None,
    });
    tokio::spawn(controller(app.clone(), rx));
    Harness { app, dir, adapter }
}

fn rand_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

async fn call(
    app: &Arc<App>,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    let req = req
        .body(
            body.map(|b| Body::from(b.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    let resp = hue_jack::web::router(app.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 10 << 20)
        .await
        .unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    (status, value)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_settings_and_validation() {
    let h = harness(State::default(), false, None);
    let (st, v) = call(&h.app, Method::GET, "/api/status", None).await;
    assert_eq!(st, StatusCode::OK);
    for key in [
        "version",
        "image",
        "bridge",
        "stream",
        "audio",
        "calibration",
        "test_pattern",
        "sources",
        "settings",
        "effects",
        "palettes",
        "bluetooth",
    ] {
        assert!(v.get(key).is_some(), "status has {key}");
    }
    assert_eq!(v["bridge"]["paired"], false);
    assert_eq!(v["stream"]["state"], "idle");
    assert_eq!(v["effects"], json!(["pulse", "spectrum", "chase"]));
    assert_eq!(v["bluetooth"]["available"], false);
    assert!(
        v["palettes"][0]["colors"][0]
            .as_str()
            .unwrap()
            .starts_with('#')
    );

    let (st, v) = call(&h.app, Method::GET, "/api/settings", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["delay_ms"], 150);

    let patch = json!({"effect": "chase", "palette": "ocean", "intensity": 0.5, "brightness_max": 0.7, "delay_ms": 220, "idle_stop_secs": 30});
    let (st, v) = call(&h.app, Method::PUT, "/api/settings", Some(patch)).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["effect"], "chase");
    assert_eq!(v["delay_ms"], 220);
    assert_eq!(h.app.delay.delay_ms(), 220, "delay applied live");
    let saved = State::load(&h.dir).unwrap();
    assert_eq!(
        (
            saved.effect.as_str(),
            saved.palette.as_str(),
            saved.delay_ms,
            saved.idle_stop_secs
        ),
        ("chase", "ocean", 220, 30)
    );

    for bad in [
        json!({"effect": "strobe-party"}),
        json!({"palette": "beige"}),
        json!({"intensity": 1.5}),
        json!({"brightness_max": -0.1}),
        json!({"delay_ms": 5000}),
        json!({"idle_stop_secs": 0}),
    ] {
        let (st, v) = call(&h.app, Method::PUT, "/api/settings", Some(bad.clone())).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad} → {v}");
        assert!(v["error"].is_string());
    }
    let (st, _) = call(
        &h.app,
        Method::PUT,
        "/api/settings",
        Some(json!({"nonsense": 1})),
    )
    .await;
    assert!(st.is_client_error());
    // Areas need a paired bridge.
    let (st, v) = call(
        &h.app,
        Method::PUT,
        "/api/settings",
        Some(json!({"area_id": AREA_ID})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{v}");
    let (st, _) = call(&h.app, Method::GET, "/api/areas", None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (st, v) = call(&h.app, Method::GET, "/api/nope", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(v["error"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_areas_and_streaming_controls() {
    let bridge = FakeBridge::start().await.unwrap();
    let h = harness(State::default(), false, Some(bridge.dtls_addr().port()));
    let ip = bridge.https_addr().to_string();
    let (st, v) = call(
        &h.app,
        Method::POST,
        "/api/bridge/pair",
        Some(json!({"ip": ip})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{v}");
    assert_eq!(v["state"], "waiting");
    let (st, _) = call(
        &h.app,
        Method::POST,
        "/api/bridge/pair",
        Some(json!({"ip": ip})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "one pairing at a time");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (_, v) = call(&h.app, Method::GET, "/api/bridge/pair", None).await;
    assert_eq!(v["state"], "waiting");
    assert!(v["remaining_secs"].as_u64().unwrap() > 20);
    bridge.press_link_button();
    let mut state = Value::Null;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        state = call(&h.app, Method::GET, "/api/bridge/pair", None).await.1;
        if state["state"] != "waiting" {
            break;
        }
    }
    assert_eq!(state["state"], "paired", "{state}");
    let saved = State::load(&h.dir).unwrap();
    assert_eq!(saved.bridge.as_ref().unwrap().bridge_id, BRIDGE_ID);
    assert_eq!(
        saved.area_id.as_deref(),
        Some(AREA_ID),
        "the only area is selected automatically"
    );

    let (st, v) = call(&h.app, Method::GET, "/api/areas", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v[0]["id"], AREA_ID);
    assert_eq!(v[0]["channels"].as_array().unwrap().len(), 6);
    let (st, _) = call(
        &h.app,
        Method::PUT,
        "/api/settings",
        Some(json!({"area_id": "00000000-0000-0000-0000-000000000000"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v) = call(
        &h.app,
        Method::PUT,
        "/api/settings",
        Some(json!({"area_id": AREA_ID})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["area_id"], AREA_ID);

    // A test pattern starts the stream without any audio, and stops it when it ends.
    let (st, _) = call(
        &h.app,
        Method::POST,
        "/api/test-pattern",
        Some(json!({"pattern": "disco", "secs": 3})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, v) = call(
        &h.app,
        Method::POST,
        "/api/test-pattern",
        Some(json!({"pattern": "chase", "secs": 2})),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
    assert_eq!(v["pattern"], "chase");
    let (_, v) = call(&h.app, Method::GET, "/api/status", None).await;
    assert_eq!(v["test_pattern"], "chase");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (_, v) = call(&h.app, Method::GET, "/api/status", None).await;
    assert_eq!(v["stream"]["state"], "streaming", "{}", v["stream"]);
    assert_eq!(v["stream"]["area_id"], AREA_ID);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!bridge.packets().is_empty());
    let (_, v) = call(&h.app, Method::GET, "/api/status", None).await;
    assert_eq!(v["test_pattern"], Value::Null);
    assert_eq!(v["stream"]["state"], "idle", "stopped after the pattern");
    assert!(!bridge.is_active());

    // Calibration also streams.
    let (st, v) = call(
        &h.app,
        Method::POST,
        "/api/calibration",
        Some(json!({"on": true})),
    )
    .await;
    assert_eq!((st, v), (StatusCode::OK, json!({"on": true})));
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(bridge.is_active());
    let (_, v) = call(&h.app, Method::GET, "/api/status", None).await;
    assert_eq!(v["calibration"], true);
    call(
        &h.app,
        Method::POST,
        "/api/calibration",
        Some(json!({"on": false})),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(!bridge.is_active());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bluetooth_endpoints() {
    let h = harness(State::default(), true, None);
    let (st, v) = call(
        &h.app,
        Method::POST,
        "/api/bluetooth/pairing",
        Some(json!({"seconds": 120})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["pairing"], true);
    assert!(v["remaining_secs"].as_u64().unwrap() >= 119);
    let (_, v) = call(&h.app, Method::GET, "/api/status", None).await;
    assert_eq!(
        v["bluetooth"],
        json!({"available": true, "pairing": true, "pairing_remaining_secs": v["bluetooth"]["pairing_remaining_secs"]})
    );
    let (st, v) = call(
        &h.app,
        Method::POST,
        "/api/bluetooth/pairing",
        Some(json!({"seconds": 0})),
    )
    .await;
    assert_eq!((st, &v["pairing"]), (StatusCode::OK, &json!(false)));
    assert_eq!(*h.adapter.visible.lock().unwrap(), vec![true, false]);

    let (st, v) = call(&h.app, Method::GET, "/api/bluetooth/devices", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v[0]["address"], "AA:BB:CC:DD:EE:01");
    let (st, _) = call(
        &h.app,
        Method::DELETE,
        "/api/bluetooth/devices/not-an-address",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &h.app,
        Method::DELETE,
        "/api/bluetooth/devices/AA%3ABB%3ACC%3ADD%3AEE%3A01",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(
        &h.app,
        Method::DELETE,
        "/api/bluetooth/devices/AA:BB:CC:DD:EE:01",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let none = harness(State::default(), false, None);
    let (st, v) = call(
        &none.app,
        Method::POST,
        "/api/bluetooth/pairing",
        Some(json!({"seconds": 60})),
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
    assert!(v["error"].is_string());
    let (st, _) = call(&none.app, Method::GET, "/api/bluetooth/devices", None).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_returns_a_list() {
    let h = harness(State::default(), false, None);
    let (st, v) = call(&h.app, Method::GET, "/api/bridges/discover", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(v.is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_is_served() {
    let h = harness(State::default(), false, None);
    let resp = hue_jack::web::router(h.app.clone())
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("hue-jack"));
    // Unknown UI routes get the app too; unknown files don't.
    let resp = hue_jack::web::router(h.app.clone())
        .oneshot(
            Request::builder()
                .uri("/lights")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = hue_jack::web::router(h.app.clone())
        .oneshot(
            Request::builder()
                .uri("/missing.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_streams_live_frames() {
    let h = harness(State::default(), false, None);
    // Feed drums into the engine thread through the analysis ring.
    let (mut producer, consumer) = rtrb::RingBuffer::new(48_000 * 4);
    let stop = Arc::new(AtomicBool::new(false));
    let engine = engine_thread(h.app.clone(), consumer, stop.clone()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, hue_jack::web::router(h.app.clone())).into_future());
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let feeder = tokio::task::spawn_blocking(move || {
        let (audio, _) = hue_jack::audio::testgen::drums(3.0, 128.0);
        for block in audio.chunks(480) {
            let stereo: Vec<f32> = block.iter().flat_map(|&s| [s, s]).collect();
            while producer.slots() < stereo.len() {
                std::thread::sleep(Duration::from_millis(1));
            }
            for s in stereo {
                producer.push(s).unwrap();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut frames = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while frames.len() < 30 && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(1), ws.next()).await
            && let Ok(text) = msg.into_text()
        {
            frames.push(serde_json::from_str::<Value>(&text).unwrap());
        }
    }
    feeder.await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    engine.join().unwrap();
    assert!(frames.len() >= 30, "{} frames", frames.len());
    let f = &frames[frames.len() - 1];
    assert_eq!(f["bands"].as_array().unwrap().len(), 5);
    assert!(f["levels"]["rms_db"].is_number());
    assert_eq!(
        f["channels"].as_array().unwrap().len(),
        6,
        "virtual layout without an area"
    );
    assert!(f["channels"][0]["rgb"].as_array().unwrap().len() == 3);
    assert!(
        frames.iter().any(|f| f["onset"].as_f64().unwrap() > 0.0),
        "onsets reach the UI"
    );
    assert!(
        frames.iter().any(
            |f| f["channels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["rgb"][0].as_u64().unwrap() > 100
                    || c["rgb"][1].as_u64().unwrap() > 100
                    || c["rgb"][2].as_u64().unwrap() > 100)
        )
    );
}
