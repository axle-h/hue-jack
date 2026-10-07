//! CLIP v2 REST client (plus the v1 `/api` endpoints used for pairing).
//!
//! The bridge serves a certificate whose CN is its bridge id. There's no CA to check against
//! (older bridges are self-signed), so the client accepts any certificate and instead pins the CN:
//! the expected id is given (from discovery or a previous pairing) or learnt on first contact (TOFU).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use openssl::nid::Nid;
use openssl::x509::X509;
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Area, Channel, HTTPS_PORT, normalise_bridge_id, split_host_port};

/// Unauthenticated `/api/0/config` subset.
#[derive(Clone, Debug, Deserialize)]
pub struct BridgeConfig {
    pub name: String,
    pub bridgeid: String,
    pub modelid: String,
    pub apiversion: String,
    pub swversion: String,
}

/// Outcome of a `POST /api` user creation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateUser {
    Created {
        username: String,
        client_key: String,
    },
    LinkButtonNotPressed,
}

#[derive(Clone)]
pub struct HueClient {
    http: reqwest::Client,
    base: String,
    app_key: Option<String>,
    bridge_id: Arc<Mutex<Option<String>>>,
}

impl HueClient {
    /// `addr` is `host` or `host:port`. `bridge_id` pins the certificate CN; `None` learns it on first contact.
    pub fn new(addr: &str, bridge_id: Option<&str>, app_key: Option<&str>) -> Result<Self> {
        let (host, port) = split_host_port(addr, HTTPS_PORT);
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .tls_info(true)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            base: format!("https://{host}:{port}"),
            app_key: app_key.map(str::to_string),
            bridge_id: Arc::new(Mutex::new(bridge_id.map(normalise_bridge_id))),
        })
    }

    /// The bridge id this client is pinned to, if known yet.
    pub fn bridge_id(&self) -> Option<String> {
        self.bridge_id.lock().unwrap().clone()
    }

    pub fn with_app_key(mut self, app_key: &str) -> Self {
        self.app_key = Some(app_key.to_string());
        self
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let req = self.http.request(method, format!("{}{path}", self.base));
        match &self.app_key {
            Some(key) => req.header("hue-application-key", key),
            None => req,
        }
    }

    async fn send(&self, req: RequestBuilder) -> Result<Response> {
        let resp = req.send().await.context("bridge request failed")?;
        self.check_certificate(&resp)?;
        Ok(resp)
    }

    fn check_certificate(&self, resp: &Response) -> Result<()> {
        let der = resp
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(|info| info.peer_certificate())
            .ok_or_else(|| anyhow!("bridge presented no certificate"))?;
        let cert = X509::from_der(der).context("parsing bridge certificate")?;
        let cn = cert
            .subject_name()
            .entries_by_nid(Nid::COMMONNAME)
            .next()
            .and_then(|e| e.data().to_string().ok())
            .map(|s| normalise_bridge_id(&s))
            .ok_or_else(|| anyhow!("bridge certificate has no CN"))?;
        let mut pinned = self.bridge_id.lock().unwrap();
        match pinned.as_deref() {
            Some(expected) if expected != cn => {
                bail!("bridge certificate CN {cn} does not match the paired bridge {expected}")
            }
            Some(_) => {}
            None => *pinned = Some(cn),
        }
        Ok(())
    }

    /// `GET /api/0/config` (no authentication).
    pub async fn config(&self) -> Result<BridgeConfig> {
        let resp = self
            .send(self.http.get(format!("{}/api/0/config", self.base)))
            .await?;
        Ok(resp.error_for_status()?.json().await?)
    }

    /// `POST /api` with `generateclientkey`.
    pub async fn create_user(&self, devicetype: &str) -> Result<CreateUser> {
        let body = json!({ "devicetype": devicetype, "generateclientkey": true });
        let resp = self
            .send(self.http.post(format!("{}/api", self.base)).json(&body))
            .await?;
        let value: Value = resp.error_for_status()?.json().await?;
        parse_create_user(&value)
    }

    /// `GET /auth/v1`: the `hue-application-id` response header is the DTLS PSK identity.
    pub async fn application_id(&self) -> Result<String> {
        let resp = self.send(self.request(Method::GET, "/auth/v1")).await?;
        let resp = resp.error_for_status()?;
        resp.headers()
            .get("hue-application-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("bridge did not return hue-application-id"))
    }

    /// `GET /clip/v2/resource/entertainment_configuration`.
    pub async fn areas(&self) -> Result<Vec<Area>> {
        let resp = self
            .send(self.request(Method::GET, "/clip/v2/resource/entertainment_configuration"))
            .await?;
        let resp = check_status(resp).await?;
        let value: Value = resp.json().await?;
        parse_areas(&value)
    }

    /// `PUT /clip/v2/resource/entertainment_configuration/{id}` with `{"action": "start"|"stop"}`.
    pub async fn set_streaming(&self, area_id: &str, start: bool) -> Result<()> {
        let action = if start { "start" } else { "stop" };
        let resp = self
            .send(
                self.request(
                    Method::PUT,
                    &format!("/clip/v2/resource/entertainment_configuration/{area_id}"),
                )
                .json(&json!({ "action": action })),
            )
            .await?;
        check_status(resp).await?;
        Ok(())
    }

    /// The current state of every light in the area (its `light_services`).
    pub async fn area_light_states(&self, area_id: &str) -> Result<Vec<LightState>> {
        let resp = self
            .send(self.request(
                Method::GET,
                &format!("/clip/v2/resource/entertainment_configuration/{area_id}"),
            ))
            .await?;
        let area: Value = check_status(resp).await?.json().await?;
        let ids: Vec<&str> = area["data"][0]["light_services"]
            .as_array()
            .map(|a| a.iter().filter_map(|s| s["rid"].as_str()).collect())
            .unwrap_or_default();
        let resp = self
            .send(self.request(Method::GET, "/clip/v2/resource/light"))
            .await?;
        let lights: Value = check_status(resp).await?.json().await?;
        Ok(lights["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|l| l["id"].as_str().is_some_and(|id| ids.contains(&id)))
            .filter_map(parse_light_state)
            .collect())
    }

    /// `PUT /clip/v2/resource/light/{id}` with on/off, brightness and colour in one request.
    /// The bridge accepts brightness and colour for a light that stays off.
    pub async fn set_light_state(&self, light: &LightState) -> Result<()> {
        let resp = self
            .send(
                self.request(
                    Method::PUT,
                    &format!("/clip/v2/resource/light/{}", light.id),
                )
                .json(&light_state_body(light)),
            )
            .await?;
        check_status(resp).await?;
        Ok(())
    }
}

/// A light's on/off, brightness and colour. The bridge leaves lights on the last streamed frame after
/// `stop`, so the stream snapshots these before `start` and writes them back afterwards.
#[derive(Clone, Debug, PartialEq)]
pub struct LightState {
    pub id: String,
    pub on: bool,
    pub brightness: Option<f64>,
    pub colour: Option<LightColour>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LightColour {
    Xy(f64, f64),
    Mirek(u64),
}

fn parse_light_state(v: &Value) -> Option<LightState> {
    let xy = &v["color"]["xy"];
    let colour = match v["color_temperature"]["mirek"].as_u64() {
        Some(m) if v["color_temperature"]["mirek_valid"] == true => Some(LightColour::Mirek(m)),
        _ => xy["x"]
            .as_f64()
            .zip(xy["y"].as_f64())
            .map(|(x, y)| LightColour::Xy(x, y)),
    };
    Some(LightState {
        id: v["id"].as_str()?.to_string(),
        on: v["on"]["on"].as_bool()?,
        brightness: v["dimming"]["brightness"].as_f64(),
        colour,
    })
}

fn light_state_body(light: &LightState) -> Value {
    let mut body = json!({ "on": { "on": light.on } });
    if let Some(b) = light.brightness {
        body["dimming"] = json!({ "brightness": b });
    }
    match light.colour {
        Some(LightColour::Xy(x, y)) => body["color"] = json!({ "xy": { "x": x, "y": y } }),
        Some(LightColour::Mirek(m)) => body["color_temperature"] = json!({ "mirek": m }),
        None => {}
    }
    body
}

async fn check_status(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    match status {
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED => {
            bail!("bridge rejected the application key ({status}); pair again")
        }
        _ => bail!("bridge returned {status}: {body}"),
    }
}

fn parse_create_user(value: &Value) -> Result<CreateUser> {
    let first = value
        .get(0)
        .ok_or_else(|| anyhow!("empty response from POST /api"))?;
    if let Some(success) = first.get("success") {
        let username = success["username"]
            .as_str()
            .ok_or_else(|| anyhow!("no username"))?;
        let client_key = success["clientkey"]
            .as_str()
            .ok_or_else(|| anyhow!("no clientkey"))?;
        return Ok(CreateUser::Created {
            username: username.to_string(),
            client_key: client_key.to_string(),
        });
    }
    if let Some(error) = first.get("error") {
        if error["type"].as_i64() == Some(101) {
            return Ok(CreateUser::LinkButtonNotPressed);
        }
        bail!(
            "bridge refused pairing: {}",
            error["description"].as_str().unwrap_or("unknown error")
        );
    }
    bail!("unexpected response from POST /api: {value}")
}

fn parse_areas(value: &Value) -> Result<Vec<Area>> {
    #[derive(Deserialize)]
    struct Position {
        x: f32,
        y: f32,
        z: f32,
    }
    #[derive(Deserialize)]
    struct RawChannel {
        channel_id: u8,
        position: Position,
    }
    #[derive(Deserialize, Default)]
    struct Metadata {
        #[serde(default)]
        name: String,
    }
    #[derive(Deserialize)]
    struct RawArea {
        id: String,
        #[serde(default)]
        metadata: Metadata,
        #[serde(default)]
        status: String,
        #[serde(default)]
        channels: Vec<RawChannel>,
    }
    let data = value
        .get("data")
        .ok_or_else(|| anyhow!("CLIP response has no data"))?;
    let raw: Vec<RawArea> =
        serde_json::from_value(data.clone()).context("parsing entertainment areas")?;
    Ok(raw
        .into_iter()
        .map(|a| Area {
            id: a.id,
            name: a.metadata.name,
            status: a.status,
            channels: a
                .channels
                .into_iter()
                .map(|c| Channel {
                    id: c.channel_id,
                    x: c.position.x,
                    y: c.position.y,
                    z: c.position.z,
                })
                .collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_user_responses() {
        let ok = json!([{"success": {"username": "u", "clientkey": "ABCD"}}]);
        assert_eq!(
            parse_create_user(&ok).unwrap(),
            CreateUser::Created {
                username: "u".into(),
                client_key: "ABCD".into()
            }
        );
        let wait = json!([{"error": {"type": 101, "address": "", "description": "link button not pressed"}}]);
        assert_eq!(
            parse_create_user(&wait).unwrap(),
            CreateUser::LinkButtonNotPressed
        );
        let other = json!([{"error": {"type": 7, "description": "invalid value"}}]);
        assert!(parse_create_user(&other).is_err());
    }

    #[test]
    fn light_state_round_trip() {
        let xy = json!({"id": "a", "on": {"on": false}, "dimming": {"brightness": 0.0},
            "color_temperature": {"mirek": null, "mirek_valid": false},
            "color": {"xy": {"x": 0.3127, "y": 0.3325}}});
        let s = parse_light_state(&xy).unwrap();
        assert_eq!(
            s,
            LightState {
                id: "a".into(),
                on: false,
                brightness: Some(0.0),
                colour: Some(LightColour::Xy(0.3127, 0.3325))
            }
        );
        assert_eq!(
            light_state_body(&s),
            json!({"on": {"on": false}, "dimming": {"brightness": 0.0}, "color": {"xy": {"x": 0.3127, "y": 0.3325}}})
        );

        let ct = json!({"id": "b", "on": {"on": true}, "dimming": {"brightness": 60.08},
            "color_temperature": {"mirek": 366, "mirek_valid": true},
            "color": {"xy": {"x": 0.4572, "y": 0.4099}}});
        assert_eq!(
            light_state_body(&parse_light_state(&ct).unwrap()),
            json!({"on": {"on": true}, "dimming": {"brightness": 60.08}, "color_temperature": {"mirek": 366}})
        );

        let white = json!({"id": "c", "on": {"on": true}, "dimming": {"brightness": 40.0}});
        assert_eq!(parse_light_state(&white).unwrap().colour, None);
    }

    #[test]
    fn areas_from_clip() {
        let v = json!({"errors": [], "data": [{
            "id": "1a8d99cc-967b-44f2-9202-43f976c0fa6b",
            "type": "entertainment_configuration",
            "metadata": {"name": "Living room"},
            "status": "inactive",
            "channels": [
                {"channel_id": 0, "position": {"x": -0.5, "y": 0.8, "z": 0.0}, "members": []},
                {"channel_id": 1, "position": {"x": 0.5, "y": 0.8, "z": 0.0}, "members": []}
            ]
        }]});
        let areas = parse_areas(&v).unwrap();
        assert_eq!(areas.len(), 1);
        assert_eq!(areas[0].name, "Living room");
        assert_eq!(
            areas[0].channels[1],
            Channel {
                id: 1,
                x: 0.5,
                y: 0.8,
                z: 0.0
            }
        );
    }
}
