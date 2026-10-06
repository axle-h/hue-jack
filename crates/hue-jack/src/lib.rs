//! hue-jack: music → Philips Hue appliance.
//!
//! Audio arrives on a PipeWire sink, is analysed (bands, onsets, tempo), turned into light frames by an
//! effect, streamed to a Hue Entertainment area, and played back delayed by the light latency.

pub mod analysis;
pub mod audio;
pub mod cli;
pub mod effects;
pub mod engine;
pub mod hue;
pub mod serve;
pub mod simulate;
pub mod sources;
pub mod state;
pub mod web;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
