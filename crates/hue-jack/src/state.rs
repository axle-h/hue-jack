//! Persistent state (`state.json` in the state directory).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::hue::BridgeCredentials;

pub const STATE_FILE: &str = "state.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub bridge: Option<BridgeCredentials>,
    pub area_id: Option<String>,
    /// Audio delay `D` in milliseconds.
    pub delay_ms: u32,
    pub effect: String,
    pub palette: String,
    /// Output brightness cap, 0..1.
    pub brightness_max: f32,
    /// Effect intensity, 0..1.
    pub intensity: f32,
    /// Stop streaming after this many seconds of silence.
    pub idle_stop_secs: u32,
    /// PipeWire output node name, or `auto` (first `alsa_output.*` sink).
    pub output: String,
    /// PipeWire sink whose monitor is captured.
    pub input_sink: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            bridge: None,
            area_id: None,
            delay_ms: 150,
            effect: "pulse".into(),
            palette: "sunset".into(),
            brightness_max: 1.0,
            intensity: 1.0,
            idle_stop_secs: 20,
            output: "auto".into(),
            input_sink: "hue-jack-in".into(),
        }
    }
}

/// `$HUEJACK_STATE_DIR`, else `$XDG_STATE_HOME/hue-jack`, else `~/.local/state/hue-jack`.
pub fn default_state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HUEJACK_STATE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir).join("hue-jack");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/state/hue-jack")
}

impl State {
    /// Loads `state.json` from `dir`; a missing file gives the defaults.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(STATE_FILE);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Writes `state.json` atomically (temp file + rename), mode 0600 since it holds the bridge keys.
    pub fn save(&self, dir: &Path) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(STATE_FILE);
        let tmp = dir.join(format!(".{STATE_FILE}.{}.tmp", std::process::id()));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(self)?)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("hue-jack-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(State::load(&dir).unwrap(), State::default());
        let mut s = State {
            delay_ms: 220,
            area_id: Some("a".into()),
            ..State::default()
        };
        s.bridge = Some(BridgeCredentials {
            ip: "10.0.0.166".into(),
            bridge_id: "ecb5fafffea77674".into(),
            app_key: "k".into(),
            client_key: "00".into(),
            app_id: "id".into(),
        });
        s.save(&dir).unwrap();
        assert_eq!(State::load(&dir).unwrap(), s);
        // Partial files fill in defaults.
        fs::write(dir.join(STATE_FILE), r#"{"delay_ms": 90}"#).unwrap();
        let partial = State::load(&dir).unwrap();
        assert_eq!(partial.delay_ms, 90);
        assert_eq!(partial.effect, "pulse");
        fs::remove_dir_all(&dir).unwrap();
    }
}
