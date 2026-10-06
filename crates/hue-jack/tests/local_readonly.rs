//! Read-only checks against the real buses of the machine running them (ignored by default; the
//! CI container has neither). They never pause players, register agents or change the adapter.
//!
//!   cargo test -p hue-jack --test local_readonly -- --ignored --nocapture

#[tokio::test]
#[ignore]
async fn mpris_players_on_the_session_bus() {
    let bus = hue_jack::sources::mpris::Mpris::connect()
        .await
        .expect("session bus");
    let players = bus.players().await.expect("list players");
    for (name, props) in &players {
        let s = hue_jack::sources::mpris::to_source(name, props);
        println!("{} → {:?} {:?} {:?}", name, s.kind, s.state, s.title);
    }
    println!("{} MPRIS players", players.len());
}

#[cfg(feature = "bluetooth")]
#[tokio::test]
#[ignore]
async fn bluetooth_adapter_names() {
    let session = bluer::Session::new().await.expect("BlueZ session");
    let names = session.adapter_names().await.expect("adapter names");
    println!("adapters: {names:?}");
}
