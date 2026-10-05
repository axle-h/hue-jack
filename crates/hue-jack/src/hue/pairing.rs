//! Pairing: create an application key (needs the bridge's link button) and fetch the PSK identity.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use super::BridgeCredentials;
use super::clip::{CreateUser, HueClient};

/// Pairing progress, reported while waiting for the link button.
#[derive(Clone, Debug, PartialEq)]
pub enum PairProgress {
    WaitingForButton { remaining: Duration },
}

/// Polls `POST /api` every `poll` until the link button is pressed or `timeout` passes,
/// then fetches the application id. `addr` is what gets stored as the bridge address.
pub async fn pair(
    client: &HueClient,
    addr: &str,
    devicetype: &str,
    timeout: Duration,
    poll: Duration,
    mut progress: impl FnMut(PairProgress),
) -> Result<BridgeCredentials> {
    let deadline = Instant::now() + timeout;
    let (username, client_key) = loop {
        match client.create_user(devicetype).await? {
            CreateUser::Created {
                username,
                client_key,
            } => break (username, client_key),
            CreateUser::LinkButtonNotPressed => {
                let now = Instant::now();
                if now >= deadline {
                    bail!("link button not pressed within {} s", timeout.as_secs());
                }
                progress(PairProgress::WaitingForButton {
                    remaining: deadline - now,
                });
                tokio::time::sleep(poll.min(deadline - now)).await;
            }
        }
    };
    let app_id = client
        .clone()
        .with_app_key(&username)
        .application_id()
        .await?;
    let bridge_id = match client.bridge_id() {
        Some(id) => id,
        None => bail!("bridge id unknown after pairing"),
    };
    Ok(BridgeCredentials {
        ip: addr.to_string(),
        bridge_id,
        app_key: username,
        client_key,
        app_id,
    })
}

/// `devicetype` for this host: `hue-jack#<hostname>` (max 40 chars per the API).
pub fn devicetype() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "nuc".to_string());
    let mut dt = format!("hue-jack#{host}");
    dt.truncate(40);
    dt
}
