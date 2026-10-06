//! Bluetooth: the "pair new device" window, the auto-accepting agent, and bonded devices.
//!
//! The window state machine and API are backend-independent and unit-tested with a mock adapter;
//! the BlueZ backend (via `bluer`) lives in [`bluez`] behind the `bluetooth` feature.
//! Outside the window the adapter is neither discoverable nor pairable, and the agent rejects
//! every request; bonded devices can still reconnect.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use futures::future::BoxFuture;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub address: String,
    pub name: Option<String>,
    pub connected: bool,
    pub paired: bool,
    pub trusted: bool,
}

/// What hue-jack needs from an adapter.
pub trait Adapter: Send + Sync {
    /// Discoverable + pairable on or off.
    fn set_visible(&self, on: bool) -> BoxFuture<'_, Result<()>>;
    fn devices(&self) -> BoxFuture<'_, Result<Vec<DeviceInfo>>>;
    fn remove_device(&self, address: &str) -> BoxFuture<'_, Result<()>>;
    /// Marks a newly paired device trusted, so it can reconnect without the window.
    fn trust(&self, address: &str) -> BoxFuture<'_, Result<()>>;
}

/// When pairing is allowed. Shared with the agent, which asks it before accepting anything.
#[derive(Debug, Default)]
pub struct PairingWindow {
    until: Mutex<Option<Instant>>,
}

impl PairingWindow {
    pub fn open(&self, secs: u64) {
        *self.until.lock().unwrap() = Some(Instant::now() + Duration::from_secs(secs));
    }
    pub fn close(&self) {
        *self.until.lock().unwrap() = None;
    }
    pub fn is_open(&self) -> bool {
        self.until
            .lock()
            .unwrap()
            .is_some_and(|t| Instant::now() < t)
    }
    pub fn remaining_secs(&self) -> u64 {
        self.until
            .lock()
            .unwrap()
            .map(|t| t.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0)
    }
    /// The agent's decision for a "Just Works" pairing request.
    pub fn should_accept(&self) -> bool {
        self.is_open()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BluetoothStatus {
    pub available: bool,
    pub pairing: bool,
    pub pairing_remaining_secs: u64,
}

/// Longest window the API accepts.
pub const MAX_WINDOW_SECS: u64 = 300;

pub struct Bluetooth {
    adapter: Arc<dyn Adapter>,
    window: Arc<PairingWindow>,
    generation: Mutex<u64>,
}

impl Bluetooth {
    pub fn new(adapter: Arc<dyn Adapter>, window: Arc<PairingWindow>) -> Arc<Self> {
        Arc::new(Self {
            adapter,
            window,
            generation: Mutex::new(0),
        })
    }

    pub fn window(&self) -> &Arc<PairingWindow> {
        &self.window
    }

    pub fn status(&self) -> BluetoothStatus {
        BluetoothStatus {
            available: true,
            pairing: self.window.is_open(),
            pairing_remaining_secs: self.window.remaining_secs(),
        }
    }

    /// Opens the pairing window for `secs` (1..=300); it closes itself afterwards. Needs a tokio runtime.
    pub async fn open_window(self: &Arc<Self>, secs: u64) -> Result<BluetoothStatus> {
        let secs = secs.clamp(1, MAX_WINDOW_SECS);
        self.adapter.set_visible(true).await?;
        self.window.open(secs);
        let generation = {
            let mut g = self.generation.lock().unwrap();
            *g += 1;
            *g
        };
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(secs)).await;
            // Only the latest window closes things (re-opening extends it).
            if *me.generation.lock().unwrap() == generation {
                me.close_window().await;
            }
        });
        Ok(self.status())
    }

    pub async fn close_window(&self) {
        self.window.close();
        if let Err(e) = self.adapter.set_visible(false).await {
            tracing::warn!("closing the Bluetooth pairing window: {e:#}");
        }
    }

    pub async fn devices(&self) -> Result<Vec<DeviceInfo>> {
        let mut d = self.adapter.devices().await?;
        d.retain(|d| d.paired);
        Ok(d)
    }

    pub async fn remove(&self, address: &str) -> Result<()> {
        self.adapter.remove_device(address).await
    }

    /// Called by the agent after it accepted a pairing.
    pub async fn paired(&self, address: &str) {
        if let Err(e) = self.adapter.trust(address).await {
            tracing::warn!(address, "trusting new device: {e:#}");
        }
    }
}

/// A fake adapter for tests (records visibility changes).
#[derive(Default)]
pub struct MockAdapter {
    pub visible: Mutex<Vec<bool>>,
    pub devices: Mutex<Vec<DeviceInfo>>,
}

impl Adapter for MockAdapter {
    fn set_visible(&self, on: bool) -> BoxFuture<'_, Result<()>> {
        self.visible.lock().unwrap().push(on);
        Box::pin(async { Ok(()) })
    }
    fn devices(&self) -> BoxFuture<'_, Result<Vec<DeviceInfo>>> {
        let d = self.devices.lock().unwrap().clone();
        Box::pin(async move { Ok(d) })
    }
    fn remove_device(&self, address: &str) -> BoxFuture<'_, Result<()>> {
        let mut d = self.devices.lock().unwrap();
        let before = d.len();
        d.retain(|d| d.address != address);
        let removed = d.len() != before;
        let address = address.to_string();
        Box::pin(async move {
            if removed {
                Ok(())
            } else {
                Err(anyhow::anyhow!("no device {address}"))
            }
        })
    }
    fn trust(&self, address: &str) -> BoxFuture<'_, Result<()>> {
        if let Some(d) = self
            .devices
            .lock()
            .unwrap()
            .iter_mut()
            .find(|d| d.address == address)
        {
            d.trusted = true;
        }
        Box::pin(async { Ok(()) })
    }
}

#[cfg(feature = "bluetooth")]
pub mod bluez;

/// Connects to BlueZ and registers the pairing agent; `None` when Bluetooth isn't available.
pub async fn start() -> Option<Arc<Bluetooth>> {
    #[cfg(feature = "bluetooth")]
    {
        match bluez::start().await {
            Ok(bt) => return Some(bt),
            Err(e) => tracing::warn!("Bluetooth unavailable: {e:#}"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn window_opens_and_closes() {
        let adapter = Arc::new(MockAdapter::default());
        let bt = Bluetooth::new(adapter.clone(), Arc::new(PairingWindow::default()));
        assert!(
            !bt.window().should_accept(),
            "closed by default: the agent rejects"
        );
        let st = bt.open_window(120).await.unwrap();
        assert!(st.pairing && st.pairing_remaining_secs >= 119);
        assert!(bt.window().should_accept());
        tokio::time::sleep(Duration::from_secs(60)).await;
        // Re-opening extends; the first timer must not close the new window.
        bt.open_window(120).await.unwrap();
        tokio::time::sleep(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;
        assert!(
            bt.status().pairing,
            "still open after the first window's end"
        );
        tokio::time::sleep(Duration::from_secs(60)).await;
        tokio::task::yield_now().await;
        assert_eq!(*adapter.visible.lock().unwrap(), vec![true, true, false]);
    }

    #[tokio::test]
    async fn devices_and_removal() {
        let adapter = Arc::new(MockAdapter::default());
        let dev = |a: &str, paired| DeviceInfo {
            address: a.into(),
            name: Some(a.into()),
            connected: false,
            paired,
            trusted: false,
        };
        *adapter.devices.lock().unwrap() = vec![
            dev("AA:BB:CC:DD:EE:01", true),
            dev("AA:BB:CC:DD:EE:02", false),
        ];
        let bt = Bluetooth::new(adapter.clone(), Arc::new(PairingWindow::default()));
        assert_eq!(
            bt.devices().await.unwrap().len(),
            1,
            "only bonded devices are listed"
        );
        bt.paired("AA:BB:CC:DD:EE:01").await;
        assert!(bt.devices().await.unwrap()[0].trusted);
        bt.remove("AA:BB:CC:DD:EE:01").await.unwrap();
        assert!(bt.devices().await.unwrap().is_empty());
        assert!(bt.remove("AA:BB:CC:DD:EE:09").await.is_err());
    }
}
