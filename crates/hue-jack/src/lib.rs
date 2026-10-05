//! hue-jack: music → Philips Hue appliance.
//!
//! Audio arrives on a PipeWire sink, is analysed (bands, onsets, tempo), turned into light frames by an
//! [`effects::Effect`], streamed to a Hue Entertainment area, and played back delayed by the light latency.

pub mod cli;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
