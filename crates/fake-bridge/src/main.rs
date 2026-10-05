//! `fake-bridge`: run the fake Hue bridge standalone (for manual testing of hue-jack).

use std::net::SocketAddr;

use clap::Parser;
use fake_bridge::FakeBridge;

#[derive(Parser)]
#[command(about = "Fake Philips Hue bridge: CLIP v2 over HTTPS + DTLS HueStream receiver")]
struct Args {
    /// HTTPS listen address.
    #[arg(long, default_value = "127.0.0.1:8443")]
    https: SocketAddr,
    /// DTLS listen address.
    #[arg(long, default_value = "127.0.0.1:2100")]
    dtls: SocketAddr,
    /// Keep the link button pressed (re-pressed every 25 s).
    #[arg(long)]
    link_button: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let bridge = FakeBridge::start_on(args.https, args.dtls).await?;
    println!(
        "fake bridge: https://{} dtls {} area {}",
        bridge.https_addr(),
        bridge.dtls_addr(),
        fake_bridge::AREA_ID
    );
    let mut last_packets = 0;
    loop {
        if args.link_button {
            bridge.press_link_button();
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let n = bridge.packets().len();
        if n != last_packets {
            println!("packets: {n} (+{})", n - last_packets);
            last_packets = n;
        }
    }
}
