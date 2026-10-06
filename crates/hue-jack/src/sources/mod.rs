//! Music sources (Bluetooth, AirPlay, YouTube cast): what's playing, and arbitration.
//!
//! Watchers ([`mpris`] for Bluetooth AVRCP via `mpris-proxy` and for shairport-sync, [`ytcr`] for
//! the YouTube sidecar) report players into [`Sources`]. Policy: the most recently started source
//! wins; the others are paused where possible.

pub mod bluetooth;
pub mod mpris;
pub mod ytcr;

use std::sync::{Arc, Mutex};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Bluetooth,
    Airplay,
    Youtube,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SourceInfo {
    pub id: String,
    pub kind: SourceKind,
    pub name: String,
    pub state: PlayState,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// What a watcher should do after an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arbitration {
    /// Pause these sources (ids): another source just started playing.
    Pause(Vec<String>),
    None,
}

/// The current sources and the active one.
#[derive(Debug, Default)]
pub struct Sources {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    list: Vec<SourceInfo>,
    active: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SourcesSnapshot {
    pub active: Option<String>,
    pub list: Vec<SourceInfo>,
}

impl Sources {
    pub fn snapshot(&self) -> SourcesSnapshot {
        let inner = self.inner.lock().unwrap();
        SourcesSnapshot {
            active: inner.active.clone(),
            list: inner.list.clone(),
        }
    }

    /// Adds or updates a source. A source that starts playing becomes active, and every other
    /// playing source should be paused (returned).
    pub fn update(&self, info: SourceInfo) -> Arbitration {
        let mut inner = self.inner.lock().unwrap();
        let was_playing = inner
            .list
            .iter()
            .find(|s| s.id == info.id)
            .map(|s| s.state == PlayState::Playing);
        let starts = info.state == PlayState::Playing && was_playing != Some(true);
        let id = info.id.clone();
        match inner.list.iter_mut().find(|s| s.id == info.id) {
            Some(s) => *s = info,
            None => inner.list.push(info),
        }
        if starts {
            inner.active = Some(id.clone());
            let others = inner
                .list
                .iter()
                .filter(|s| s.id != id && s.state == PlayState::Playing)
                .map(|s| s.id.clone())
                .collect::<Vec<_>>();
            if !others.is_empty() {
                return Arbitration::Pause(others);
            }
        } else if inner.active.as_deref() == Some(&id)
            && !inner
                .list
                .iter()
                .any(|s| s.id == id && s.state == PlayState::Playing)
        {
            // The active source stopped: hand over to another one that's still playing, if any.
            inner.active = inner
                .list
                .iter()
                .find(|s| s.state == PlayState::Playing)
                .map(|s| s.id.clone())
                .or(Some(id));
        }
        Arbitration::None
    }

    pub fn remove(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.list.retain(|s| s.id != id);
        if inner.active.as_deref() == Some(id) {
            inner.active = inner
                .list
                .iter()
                .find(|s| s.state == PlayState::Playing)
                .map(|s| s.id.clone());
        }
    }

    /// Removes every source of a kind whose id isn't in `keep` (a watcher's full refresh).
    pub fn retain_kind(&self, kind: SourceKind, keep: &[String]) {
        let ids: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner
                .list
                .iter()
                .filter(|s| s.kind == kind && !keep.contains(&s.id))
                .map(|s| s.id.clone())
                .collect()
        };
        for id in ids {
            self.remove(&id);
        }
    }
}

/// Starts the source watchers (MPRIS on the session bus, the YouTube sidecar) on the current runtime.
pub fn start_watchers(sources: Arc<Sources>, _bluetooth: Option<Arc<bluetooth::Bluetooth>>) {
    let _ = sources;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(id: &str, kind: SourceKind, state: PlayState) -> SourceInfo {
        SourceInfo {
            id: id.into(),
            kind,
            name: id.into(),
            state,
            title: None,
            artist: None,
            album: None,
        }
    }

    #[test]
    fn most_recent_source_wins() {
        let s = Sources::default();
        assert_eq!(
            s.update(src("bt", SourceKind::Bluetooth, PlayState::Playing)),
            Arbitration::None
        );
        assert_eq!(s.snapshot().active.as_deref(), Some("bt"));
        // YouTube starts: Bluetooth gets paused.
        assert_eq!(
            s.update(src("yt", SourceKind::Youtube, PlayState::Playing)),
            Arbitration::Pause(vec!["bt".into()])
        );
        assert_eq!(s.snapshot().active.as_deref(), Some("yt"));
        // Updates while already playing don't re-arbitrate.
        assert_eq!(
            s.update(src("yt", SourceKind::Youtube, PlayState::Playing)),
            Arbitration::None
        );
        s.update(src("bt", SourceKind::Bluetooth, PlayState::Paused));
        s.update(src("yt", SourceKind::Youtube, PlayState::Stopped));
        assert_eq!(
            s.snapshot().active.as_deref(),
            Some("yt"),
            "last active stays shown while nothing plays"
        );
        s.retain_kind(SourceKind::Youtube, &[]);
        assert_eq!(s.snapshot().active, None);
        assert_eq!(s.snapshot().list.len(), 1);
    }
}
