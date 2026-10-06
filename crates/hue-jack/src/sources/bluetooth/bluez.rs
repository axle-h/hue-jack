//! BlueZ backend via `bluer`: adapter control and the pairing agent.
//!
//! The agent auto-accepts pairing (confirmation and Just Works authorisation) only while the
//! pairing window is open, and service authorisation for bonded devices. Newly paired devices are
//! marked trusted so they reconnect without the window.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bluer::agent::{Agent, ReqError};
use bluer::{Address, Session};
use futures::future::BoxFuture;

use super::{Adapter, Bluetooth, DeviceInfo, PairingWindow};

struct BluezAdapter {
    adapter: bluer::Adapter,
}

impl Adapter for BluezAdapter {
    fn set_visible(&self, on: bool) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.adapter.set_pairable(on).await?;
            self.adapter.set_discoverable(on).await?;
            Ok(())
        })
    }

    fn devices(&self) -> BoxFuture<'_, Result<Vec<DeviceInfo>>> {
        Box::pin(async move {
            let mut out = Vec::new();
            for addr in self.adapter.device_addresses().await? {
                let d = self.adapter.device(addr)?;
                out.push(DeviceInfo {
                    address: addr.to_string(),
                    name: d.alias().await.ok().or(d.name().await.ok().flatten()),
                    connected: d.is_connected().await.unwrap_or(false),
                    paired: d.is_paired().await.unwrap_or(false),
                    trusted: d.is_trusted().await.unwrap_or(false),
                });
            }
            out.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.cmp(&b.name)));
            Ok(out)
        })
    }

    fn remove_device(&self, address: &str) -> BoxFuture<'_, Result<()>> {
        let address = address.to_string();
        Box::pin(async move {
            let addr = Address::from_str(&address).context("invalid address")?;
            self.adapter.remove_device(addr).await?;
            Ok(())
        })
    }

    fn trust(&self, address: &str) -> BoxFuture<'_, Result<()>> {
        let address = address.to_string();
        Box::pin(async move {
            let addr = Address::from_str(&address).context("invalid address")?;
            self.adapter.device(addr)?.set_trusted(true).await?;
            Ok(())
        })
    }
}

fn gate(
    window: &Arc<PairingWindow>,
    what: &'static str,
    device: Address,
) -> BoxFuture<'static, Result<(), ReqError>> {
    let accept = window.should_accept();
    tracing::info!(%device, accept, "Bluetooth {what} request");
    Box::pin(async move {
        if accept {
            Ok(())
        } else {
            Err(ReqError::Rejected)
        }
    })
}

/// Connects to BlueZ, registers the agent and closes the pairing window.
pub async fn start() -> Result<Arc<Bluetooth>> {
    let session = Session::new().await.context("connecting to BlueZ")?;
    let adapter = session
        .default_adapter()
        .await
        .context("no Bluetooth adapter")?;
    adapter
        .set_powered(true)
        .await
        .context("powering the adapter")?;
    let window = Arc::new(PairingWindow::default());
    let agent = {
        let (w1, w2) = (window.clone(), window.clone());
        let svc_adapter = adapter.clone();
        let svc_window = window.clone();
        Agent {
            request_default: true,
            request_confirmation: Some(Box::new(move |req| gate(&w1, "confirmation", req.device))),
            request_authorization: Some(Box::new(move |req| {
                gate(&w2, "pairing authorisation", req.device)
            })),
            authorize_service: Some(Box::new(move |req| {
                let adapter = svc_adapter.clone();
                let window = svc_window.clone();
                Box::pin(async move {
                    let bonded = match adapter.device(req.device) {
                        Ok(d) => d.is_paired().await.unwrap_or(false),
                        Err(_) => false,
                    };
                    if bonded || window.should_accept() {
                        Ok(())
                    } else {
                        Err(ReqError::Rejected)
                    }
                })
            })),
            ..Default::default()
        }
    };
    let agent_handle = session
        .register_agent(agent)
        .await
        .context("registering the pairing agent")?;
    let bt = Bluetooth::new(
        Arc::new(BluezAdapter {
            adapter: adapter.clone(),
        }),
        window,
    );
    bt.close_window().await;
    tracing::info!(adapter = adapter.name(), "Bluetooth ready");
    // Keep the session and agent alive, and trust devices once they've paired.
    let keeper = bt.clone();
    tokio::spawn(async move {
        let _session = session;
        let _agent = agent_handle;
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Ok(devices) = keeper.devices().await {
                for d in devices.iter().filter(|d| d.paired && !d.trusted) {
                    tracing::info!(address = d.address, name = ?d.name, "new Bluetooth device paired; trusting it");
                    keeper.paired(&d.address).await;
                }
            }
        }
    });
    Ok(bt)
}
