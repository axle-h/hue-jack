//! Philips Hue: discovery, pairing, CLIP v2 and Entertainment streaming.

pub mod clip;
pub mod discovery;
pub mod huestream;
pub mod pairing;
pub mod stream;

use serde::{Deserialize, Serialize};

/// Default HTTPS port of the bridge.
pub const HTTPS_PORT: u16 = 443;
/// Default DTLS port of the Entertainment API.
pub const DTLS_PORT: u16 = 2100;

/// Everything needed to talk to a paired bridge. Stored in `state.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BridgeCredentials {
    /// `host` or `host:port` of the bridge's HTTPS API.
    pub ip: String,
    /// Bridge id, pinned at pairing (lowercase hex, matches the certificate CN).
    pub bridge_id: String,
    /// CLIP application key (`username` from pairing).
    pub app_key: String,
    /// Hex-encoded PSK for DTLS (`clientkey` from pairing).
    pub client_key: String,
    /// `hue-application-id`, the DTLS PSK identity.
    pub app_id: String,
}

/// One entertainment channel with its position (each axis in -1..1; x left→right, y back→front, z down→up).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// An entertainment area (`entertainment_configuration`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub id: String,
    pub name: String,
    pub status: String,
    pub channels: Vec<Channel>,
}

/// Splits `host[:port]` into host and port, defaulting the port.
pub fn split_host_port(addr: &str, default_port: u16) -> (String, u16) {
    if let Some((host, port)) = addr.rsplit_once(':')
        && !host.contains(':')
        && let Ok(port) = port.parse()
    {
        return (host.to_string(), port);
    }
    (addr.to_string(), default_port)
}

/// Normalises a bridge id for comparison (the certificate CN is lowercase; `/api/0/config` is uppercase).
pub fn normalise_bridge_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port() {
        assert_eq!(
            split_host_port("10.0.0.166", 443),
            ("10.0.0.166".into(), 443)
        );
        assert_eq!(
            split_host_port("127.0.0.1:8443", 443),
            ("127.0.0.1".into(), 8443)
        );
        assert_eq!(
            split_host_port("localhost:x", 443),
            ("localhost:x".into(), 443)
        );
    }
}
