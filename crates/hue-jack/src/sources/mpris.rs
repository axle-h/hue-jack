//! MPRIS watcher on the session bus: `mpris-proxy` exports Bluetooth AVRCP players, shairport-sync
//! exports AirPlay. Polls once a second (players come and go with connections).

use std::collections::HashMap;

use anyhow::Result;
use zbus::zvariant::OwnedValue;

use super::{PlayState, SourceInfo, SourceKind};

pub const PREFIX: &str = "org.mpris.MediaPlayer2.";
pub const ID_PREFIX: &str = "mpris:";

/// The bits of a player we read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerProps {
    pub identity: Option<String>,
    pub status: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// Maps a bus name and its properties to a source.
pub fn to_source(bus_name: &str, p: &PlayerProps) -> SourceInfo {
    let short = bus_name.strip_prefix(PREFIX).unwrap_or(bus_name);
    let kind = if short.to_ascii_lowercase().starts_with("shairportsync") {
        SourceKind::Airplay
    } else {
        SourceKind::Bluetooth
    };
    let state = match p.status.as_deref() {
        Some("Playing") => PlayState::Playing,
        Some("Paused") => PlayState::Paused,
        _ => PlayState::Stopped,
    };
    let name = p
        .identity
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| match kind {
            SourceKind::Airplay => "AirPlay".into(),
            _ => short.replace('_', " "),
        });
    SourceInfo {
        id: format!("{ID_PREFIX}{bus_name}"),
        kind,
        name,
        state,
        title: p.title.clone().filter(|s| !s.is_empty()),
        artist: p.artist.clone().filter(|s| !s.is_empty()),
        album: p.album.clone().filter(|s| !s.is_empty()),
    }
}

/// Reads `xesam:*` fields from MPRIS metadata.
pub fn parse_metadata(meta: &HashMap<String, OwnedValue>, p: &mut PlayerProps) {
    let string = |k: &str| {
        meta.get(k)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    };
    p.title = string("xesam:title");
    p.album = string("xesam:album");
    p.artist = meta
        .get("xesam:artist")
        .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
        .map(|a| a.join(", "))
        .or_else(|| string("xesam:artist"));
}

#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait MediaPlayer2 {
    #[zbus(property)]
    fn identity(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.mpris.MediaPlayer2.Player",
    default_path = "/org/mpris/MediaPlayer2"
)]
trait Player {
    fn pause(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;
}

/// A connection to the session bus.
pub struct Mpris {
    conn: zbus::Connection,
}

impl Mpris {
    pub async fn connect() -> Result<Self> {
        Ok(Self {
            conn: zbus::Connection::session().await?,
        })
    }

    /// Lists MPRIS players and reads their properties.
    pub async fn players(&self) -> Result<Vec<(String, PlayerProps)>> {
        let dbus = zbus::fdo::DBusProxy::new(&self.conn).await?;
        let mut out = Vec::new();
        for name in dbus.list_names().await? {
            let name = name.to_string();
            if !name.starts_with(PREFIX) {
                continue;
            }
            let mut p = PlayerProps::default();
            if let Ok(mp) = MediaPlayer2Proxy::builder(&self.conn)
                .destination(name.as_str())?
                .build()
                .await
            {
                p.identity = mp.identity().await.ok();
            }
            if let Ok(player) = PlayerProxy::builder(&self.conn)
                .destination(name.as_str())?
                .build()
                .await
            {
                p.status = player.playback_status().await.ok();
                if let Ok(meta) = player.metadata().await {
                    parse_metadata(&meta, &mut p);
                }
            }
            out.push((name, p));
        }
        Ok(out)
    }

    pub async fn pause(&self, bus_name: &str) -> Result<()> {
        PlayerProxy::builder(&self.conn)
            .destination(bus_name)?
            .build()
            .await?
            .pause()
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn maps_players_to_sources() {
        let p = PlayerProps {
            identity: Some("Alex's Pixel".into()),
            status: Some("Playing".into()),
            title: Some("Song".into()),
            ..Default::default()
        };
        let s = to_source("org.mpris.MediaPlayer2.bluez_proxy_hci0_dev_AA", &p);
        assert_eq!(
            (s.kind, s.state, s.name.as_str()),
            (SourceKind::Bluetooth, PlayState::Playing, "Alex's Pixel")
        );
        assert_eq!(s.id, "mpris:org.mpris.MediaPlayer2.bluez_proxy_hci0_dev_AA");
        let a = to_source(
            "org.mpris.MediaPlayer2.ShairportSync",
            &PlayerProps {
                status: Some("Paused".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            (a.kind, a.state, a.name.as_str()),
            (SourceKind::Airplay, PlayState::Paused, "AirPlay")
        );
        let none = to_source("org.mpris.MediaPlayer2.x", &PlayerProps::default());
        assert_eq!(none.state, PlayState::Stopped);
    }

    #[test]
    fn reads_metadata() {
        let mut meta = HashMap::new();
        meta.insert(
            "xesam:title".to_string(),
            OwnedValue::try_from(Value::from("Title")).unwrap(),
        );
        meta.insert(
            "xesam:artist".to_string(),
            OwnedValue::try_from(Value::from(vec!["A", "B"])).unwrap(),
        );
        meta.insert(
            "xesam:album".to_string(),
            OwnedValue::try_from(Value::from("Album")).unwrap(),
        );
        let mut p = PlayerProps::default();
        parse_metadata(&meta, &mut p);
        assert_eq!(
            (p.title.as_deref(), p.artist.as_deref(), p.album.as_deref()),
            (Some("Title"), Some("A, B"), Some("Album"))
        );
    }
}
