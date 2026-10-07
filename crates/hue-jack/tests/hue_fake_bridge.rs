//! M1 acceptance: pair (with a refused attempt first), list areas, start, stream `chase` for 3 s, stop,
//! and the lights are back in their previous state.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_bridge::{AREA_ID, BRIDGE_ID, Event, FakeBridge};
use hue_jack::effects::patterns::{self, Pattern};
use hue_jack::effects::to_u16;
use hue_jack::engine::LightFrame;
use hue_jack::hue::clip::{CreateUser, HueClient};
use hue_jack::hue::pairing::pair;
use hue_jack::hue::stream::{DtlsTarget, FrameSlot, StreamSession, StreamState, StreamStats};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pair_list_stream_stop() {
    let bridge = FakeBridge::start().await.unwrap();
    let addr = bridge.https_addr().to_string();

    // Before the button: refused.
    let client = HueClient::new(&addr, None, None).unwrap();
    assert_eq!(
        client.create_user("hue-jack#test").await.unwrap(),
        CreateUser::LinkButtonNotPressed
    );
    assert_eq!(
        client.bridge_id().as_deref(),
        Some(BRIDGE_ID),
        "CN learnt on first contact"
    );

    // Pairing polls until the button is pressed.
    let pressing = {
        let b = &bridge;
        async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            b.press_link_button();
        }
    };
    let mut waits = 0;
    let (creds, ()) = tokio::join!(
        pair(
            &client,
            &addr,
            "hue-jack#test",
            Duration::from_secs(10),
            Duration::from_millis(200),
            |_| waits += 1
        ),
        pressing
    );
    let creds = creds.unwrap();
    assert!(waits >= 2, "polled while waiting ({waits})");
    let (app_key, client_key, app_id) = bridge.credentials().unwrap();
    assert_eq!(
        (
            creds.app_key.as_str(),
            creds.client_key.as_str(),
            creds.app_id.as_str()
        ),
        (&*app_key, &*client_key, &*app_id)
    );
    assert_eq!(creds.bridge_id, BRIDGE_ID);
    assert!(bridge.events().starts_with(&[Event::PairRefused]));
    assert!(bridge.events().contains(&Event::Paired));

    // Areas.
    let client = HueClient::new(&addr, Some(&creds.bridge_id), Some(&creds.app_key)).unwrap();
    let areas = client.areas().await.unwrap();
    assert_eq!(areas.len(), 1);
    let area = areas.into_iter().next().unwrap();
    assert_eq!(area.id, AREA_ID);
    assert_eq!(area.channels.len(), 6);

    // Stream chase for 3 s.
    let lights_before = bridge.lights();
    let frames: FrameSlot = Arc::new(Mutex::new(LightFrame::default()));
    let stats = Arc::new(StreamStats::default());
    let target = DtlsTarget::new(
        "127.0.0.1",
        bridge.dtls_addr().port(),
        &creds.app_id,
        &creds.client_key,
    )
    .unwrap();
    let session = StreamSession::start(
        client.clone(),
        target,
        area.id.clone(),
        frames.clone(),
        stats.clone(),
    );
    // Wait for the handshake so the 3 s window is all streaming.
    let deadline = Instant::now() + Duration::from_secs(5);
    while stats.state().0 != StreamState::Streaming {
        assert!(
            Instant::now() < deadline,
            "stream did not start: {:?}",
            stats.state()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        let colours = patterns::render(
            Pattern::Chase,
            &area.channels,
            start.elapsed().as_secs_f32(),
        );
        *frames.lock().unwrap() = LightFrame {
            channels: area
                .channels
                .iter()
                .zip(colours)
                .map(|(c, rgb)| (c.id, to_u16(rgb, 1.0)))
                .collect(),
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    session.stop().await;

    let packets = bridge.packets();
    assert!(
        packets.len() >= 140,
        "only {} packets in 3 s",
        packets.len()
    );
    assert!(packets.iter().all(|p| p.config_id == AREA_ID));
    let ids: Vec<u8> = packets
        .last()
        .unwrap()
        .channels
        .iter()
        .map(|c| c.0)
        .collect();
    assert_eq!(ids, vec![0, 1, 2, 3, 4, 5]);
    assert!(
        packets
            .windows(2)
            .all(|w| w[1].seq == w[0].seq.wrapping_add(1)),
        "sequence increments"
    );
    assert!(
        packets
            .iter()
            .any(|p| p.channels.iter().any(|c| c.1 != [0, 0, 0])),
        "chase lights something"
    );
    let events = bridge.events();
    assert!(events.contains(&Event::Start(AREA_ID.into())));
    assert_eq!(
        events.last(),
        Some(&Event::Stop(AREA_ID.into())),
        "events: {events:?}"
    );
    assert!(!bridge.is_active());
    assert_eq!(stats.state().0, StreamState::Idle);
    assert_eq!(
        visible(&bridge.lights()),
        visible(&lights_before),
        "lights restored after stop"
    );
}

/// What a light shows: on/off, brightness, and colour temperature or xy, whichever mode it's in.
fn visible(lights: &[serde_json::Value]) -> Vec<serde_json::Value> {
    lights
        .iter()
        .map(|l| {
            let colour = if l["color_temperature"]["mirek_valid"] == true {
                l["color_temperature"]["mirek"].clone()
            } else {
                l["color"]["xy"].clone()
            };
            serde_json::json!([l["id"], l["on"], l["dimming"], colour])
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_bridge_id_is_rejected() {
    let bridge = FakeBridge::start().await.unwrap();
    let client = HueClient::new(
        &bridge.https_addr().to_string(),
        Some("ecb5fafffea77674"),
        None,
    )
    .unwrap();
    let err = client.config().await.unwrap_err();
    assert!(format!("{err:#}").contains("does not match"), "{err:#}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_psk_fails_handshake() {
    let bridge = FakeBridge::start().await.unwrap();
    let (key, _client_key, app_id) = bridge.pre_pair();
    let client = HueClient::new(&bridge.https_addr().to_string(), None, Some(&key)).unwrap();
    client.set_streaming(AREA_ID, true).await.unwrap();
    let target = DtlsTarget::new(
        "127.0.0.1",
        bridge.dtls_addr().port(),
        &app_id,
        "00112233445566778899aabbccddeeff",
    )
    .unwrap();
    let r = tokio::task::spawn_blocking(move || {
        hue_jack::hue::stream::connect_dtls(&target, Duration::from_secs(2))
    })
    .await
    .unwrap();
    assert!(r.is_err());
}
