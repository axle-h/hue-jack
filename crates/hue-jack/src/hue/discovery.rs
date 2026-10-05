//! Bridge discovery over mDNS (`_hue._tcp`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;

use super::normalise_bridge_id;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DiscoveredBridge {
    pub ip: String,
    pub bridge_id: String,
}

/// Browses `_hue._tcp.local.` for `wait` and returns every bridge that resolved. Blocking.
pub fn discover(wait: Duration) -> Result<Vec<DiscoveredBridge>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse("_hue._tcp.local.")?;
    let deadline = Instant::now() + wait;
    let mut found = BTreeMap::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let Some(ip) = info.get_addresses_v4().into_iter().min() else {
                    continue;
                };
                let bridge_id = info
                    .get_property_val_str("bridgeid")
                    .map(normalise_bridge_id)
                    .unwrap_or_default();
                found.insert(ip.to_string(), bridge_id);
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    Ok(found
        .into_iter()
        .map(|(ip, bridge_id)| DiscoveredBridge { ip, bridge_id })
        .collect())
}
