//! The engine: feature frames → effect → [`LightFrame`] at 50 Hz.

use serde::Serialize;

/// One frame of light output: per channel, 16-bit RGB as sent to the bridge.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LightFrame {
    pub channels: Vec<(u8, [u16; 3])>,
}
