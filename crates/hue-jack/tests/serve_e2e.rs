//! M5 acceptance: `serve` with the file audio backend and the fake bridge. Drums, a gap, drums:
//! the bridge sees `start`, ≥ 45 packets/s during the drums, `stop` after the idle period, then
//! `start` again; SIGTERM stops the stream cleanly.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fake_bridge::{AREA_ID, BRIDGE_ID, Event, FakeBridge};
use hue_jack::hue::BridgeCredentials;
use hue_jack::state::State;

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drums_gap_drums() {
    let bridge = FakeBridge::start().await.unwrap();
    let (app_key, client_key, app_id) = bridge.pre_pair();
    let dir = std::env::temp_dir().join(format!("hue-jack-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = State {
        bridge: Some(BridgeCredentials {
            ip: bridge.https_addr().to_string(),
            bridge_id: BRIDGE_ID.into(),
            app_key,
            client_key,
            app_id,
        }),
        area_id: Some(AREA_ID.into()),
        idle_stop_secs: 3,
        ..State::default()
    };
    state.save(&dir).unwrap();
    // 6 s drums, 8 s gap (8.25 s of silence: the second segment starts 0.25 s in), 4 s drums.
    let (audio, _) = hue_jack::audio::testgen::silence_gap(6.0, 8.0, 4.0);
    let stereo: Vec<f32> = audio.iter().flat_map(|&s| [s, s]).collect();
    let wav = dir.join("gap.wav");
    hue_jack::audio::wav::write_wav(&wav, 2, &stereo).unwrap();

    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_hue-jack"))
        .args([
            "--state-dir",
            dir.to_str().unwrap(),
            "--dtls-port",
            &bridge.dtls_addr().port().to_string(),
        ])
        .args([
            "serve",
            "--output",
            "null",
            "--listen",
            &format!("127.0.0.1:{port}"),
        ])
        .args(["--input-file", wav.to_str().unwrap()])
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let child = Kill(child);
    let t0 = Instant::now();

    // Mid-drums, the API reports streaming.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let status: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{port}/api/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["stream"]["state"], "streaming",
        "{}",
        status["stream"]
    );
    assert_eq!(status["audio"]["active"], true);
    assert!(
        status["stream"]["packets_per_sec"].as_f64().unwrap() >= 45.0,
        "{}",
        status["stream"]
    );

    tokio::time::sleep(Duration::from_secs(21) - t0.elapsed()).await;
    unsafe { libc::kill(child.0.id() as i32, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut child = child;
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "serve did not exit on SIGTERM");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(child.0.wait().unwrap().success());

    let events = bridge.timed_events();
    let at = |i: Instant| (i - t0).as_secs_f64();
    let starts: Vec<f64> = events
        .iter()
        .filter(|(_, e)| *e == Event::Start(AREA_ID.into()))
        .map(|(i, _)| at(*i))
        .collect();
    let stops: Vec<f64> = events
        .iter()
        .filter(|(_, e)| *e == Event::Stop(AREA_ID.into()))
        .map(|(i, _)| at(*i))
        .collect();
    println!("starts {starts:?}, stops {stops:?}");
    assert_eq!(starts.len(), 2, "events {events:?}");
    assert!(starts[0] < 2.0, "first start at {}", starts[0]);
    // Silence from 6 s; idle 3 s → stop ≈ 9 s. Drums again from 14.25 s → start ≈ 14.75 s.
    assert!(
        (8.5..10.5).contains(&stops[0]),
        "stop after the idle period at {}",
        stops[0]
    );
    assert!(
        (14.0..16.0).contains(&starts[1]),
        "second start at {}",
        starts[1]
    );
    assert!(stops.len() >= 2, "stopped at the end too");

    let packets = bridge.packets();
    let window = |a: f64, b: f64| {
        packets
            .iter()
            .filter(|p| (a..b).contains(&at(p.at)))
            .count() as f64
            / (b - a)
    };
    let rate = window(starts[0] + 1.0, 5.5);
    assert!(rate >= 45.0, "{rate} packets/s during the first drums");
    assert!(
        window(stops[0] + 0.5, starts[1] - 0.2) < 1.0,
        "no packets while stopped"
    );
    assert!(
        window(starts[1] + 1.0, 18.0) >= 45.0,
        "packets during the second drums"
    );
    assert!(
        packets
            .iter()
            .all(|p| p.config_id == AREA_ID && p.channels.len() == 6)
    );
    assert!(
        packets
            .iter()
            .any(|p| p.channels.iter().any(|c| c.1.iter().any(|&v| v > 20_000))),
        "lights actually light up"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
