//! Client for the YouTube cast sidecar's control API (HTTP over a unix socket).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{PlayState, SourceInfo, SourceKind};

pub const ID: &str = "youtube";

/// `GET /status` from the sidecar.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct YtStatus {
    pub state: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub thumbnail: Option<String>,
    pub itag: Option<u32>,
    pub bitrate: Option<f64>,
}

/// `$HUEJACK_YTCR_SOCKET`, else `$XDG_RUNTIME_DIR/hue-jack/ytcr.sock`.
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("HUEJACK_YTCR_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    runtime.join("hue-jack/ytcr.sock")
}

/// One HTTP/1.0 request over a unix socket (HTTP/1.0: no chunking, the server closes).
pub async fn request(socket: &Path, method: &str, path: &str) -> Result<(u16, Vec<u8>)> {
    let fut = async {
        let mut s = tokio::net::UnixStream::connect(socket)
            .await
            .with_context(|| format!("connecting to {}", socket.display()))?;
        s.write_all(
            format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
                .as_bytes(),
        )
        .await?;
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await?;
        parse_response(&buf)
    };
    tokio::time::timeout(Duration::from_secs(3), fut)
        .await
        .context("sidecar timed out")?
}

pub fn parse_response(buf: &[u8]) -> Result<(u16, Vec<u8>)> {
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("malformed HTTP response")?;
    let head = std::str::from_utf8(&buf[..split])?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .context("no status")?
        .parse()?;
    Ok((status, buf[split + 4..].to_vec()))
}

pub async fn status(socket: &Path) -> Result<YtStatus> {
    let (code, body) = request(socket, "GET", "/status").await?;
    if code != 200 {
        bail!("sidecar returned {code}");
    }
    Ok(serde_json::from_slice(&body)?)
}

pub async fn pause(socket: &Path) -> Result<()> {
    let (code, _) = request(socket, "POST", "/pause").await?;
    if !(200..300).contains(&code) {
        bail!("sidecar returned {code}");
    }
    Ok(())
}

pub fn to_source(s: &YtStatus) -> SourceInfo {
    let state = match s.state.as_str() {
        "playing" => PlayState::Playing,
        "paused" => PlayState::Paused,
        _ => PlayState::Stopped,
    };
    SourceInfo {
        id: ID.into(),
        kind: SourceKind::Youtube,
        name: "YouTube Music".into(),
        state,
        title: s.title.clone(),
        artist: s.artist.clone(),
        album: s.album.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_status() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nConnection: close\r\n\r\n{\"state\":\"playing\",\"title\":\"T\",\"artist\":\"A\",\"album\":null,\"thumbnail\":null,\"itag\":251,\"bitrate\":160}";
        let (code, body) = parse_response(raw).unwrap();
        assert_eq!(code, 200);
        let s: YtStatus = serde_json::from_slice(&body).unwrap();
        assert_eq!(s.itag, Some(251));
        let src = to_source(&s);
        assert_eq!(
            (src.state, src.kind, src.title.as_deref()),
            (PlayState::Playing, SourceKind::Youtube, Some("T"))
        );
        assert_eq!(
            to_source(&YtStatus {
                state: "loading".into(),
                ..Default::default()
            })
            .state,
            PlayState::Stopped
        );
    }

    #[tokio::test]
    async fn talks_to_a_unix_socket_server() {
        let dir = std::env::temp_dir().join(format!("hue-jack-ytcr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ytcr.sock");
        let _ = std::fs::remove_file(&path);
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 1024];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let resp = if req.starts_with("GET /status") {
                    "HTTP/1.0 200 OK\r\n\r\n{\"state\":\"paused\",\"title\":null,\"artist\":null,\"album\":null,\"thumbnail\":null,\"itag\":null,\"bitrate\":null}".to_string()
                } else {
                    "HTTP/1.0 204 No Content\r\n\r\n".to_string()
                };
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        assert_eq!(status(&path).await.unwrap().state, "paused");
        pause(&path).await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
