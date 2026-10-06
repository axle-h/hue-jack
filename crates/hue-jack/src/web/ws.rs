//! `/ws`: pushes live levels, bands, onsets and virtual light colours at ~20 Hz.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;

use crate::serve::App;

#[derive(Clone, Debug, Serialize)]
pub struct Levels {
    pub rms_db: f32,
    pub silent: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct WsChannel {
    pub id: u8,
    pub x: f32,
    pub y: f32,
    pub rgb: [u8; 3],
}

/// One `/ws` message.
#[derive(Clone, Debug, Serialize)]
pub struct LiveFrame {
    pub levels: Levels,
    pub bands: [f32; 5],
    pub onset: f32,
    pub bpm: Option<f32>,
    pub channels: Vec<WsChannel>,
}

pub async fn upgrade(State(app): State<Arc<App>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| send_frames(socket, app))
}

async fn send_frames(mut socket: WebSocket, app: Arc<App>) {
    let mut rx = app.ws.subscribe();
    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Ok(json) => {
                    if socket.send(Message::Text(json.as_ref().into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}
