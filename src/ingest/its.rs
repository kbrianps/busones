//! Client for the SMTR aggregator (API GPS 2.0).
//!
//! The endpoint serves exactly one UTC minute per call. The open minute fills
//! in steps of 10 to 15 s and a closed minute is only complete about 15 s after
//! it ends: asking earlier returns a first-quarter stub with HTTP 200 and
//! `providers_status: ok`, so the caller has to judge completeness itself.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::model::{Dir, Fix, Vendor};
use crate::timeutil;

pub const DEFAULT_URL: &str = "https://its.mobilidade.rio/v1/geolocalizacao/veiculos";
const PAGE_LIMIT: u32 = 5000;
const MAX_PAGES: usize = 8;

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    data: Vec<Row>,
    #[serde(default)]
    next_cursor: Option<String>,
    #[serde(default)]
    providers_status: Option<serde_json::Value>,
    #[serde(default)]
    minuto_utc: Option<String>,
}

#[derive(Deserialize)]
struct Row {
    #[serde(default)]
    id_veiculo: Option<String>,
    #[serde(default)]
    fornecedor: Option<String>,
    #[serde(default)]
    modo: Option<String>,
    #[serde(default)]
    servico: Option<String>,
    #[serde(default)]
    sentido: Option<String>,
    #[serde(default)]
    latitude: Option<f64>,
    #[serde(default)]
    longitude: Option<f64>,
    #[serde(default)]
    direcao: Option<f64>,
    #[serde(default)]
    shape_id: Option<String>,
    #[serde(default)]
    trip_id: Option<String>,
    #[serde(default)]
    datetime: Option<String>,
    #[serde(default)]
    datetime_servidor: Option<String>,
}

#[derive(Default)]
pub struct Fetch {
    pub fixes: Vec<Fix>,
    pub rows: usize,
    pub bytes: usize,
    pub pages: usize,
    pub minute: Option<String>,
    pub providers: BTreeMap<String, String>,
    /// Second-of-minute of the newest `datetime_servidor` seen, the signal that
    /// tells a consolidated minute from a first-quarter stub.
    pub max_server_second: Option<i64>,
}

pub struct Client {
    http: reqwest::Client,
    url: String,
}

impl Client {
    pub fn new(url: String, http: reqwest::Client) -> Self {
        Client { http, url }
    }

    /// Fetches one UTC minute, following the cursor until the page list ends.
    /// `minute` is `None` for the minute in progress.
    pub async fn fetch(&self, minute: Option<&str>) -> Result<Fetch> {
        let mut out = Fetch::default();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut url = format!("{}?limit={PAGE_LIMIT}", self.url);
            if let Some(m) = minute {
                url.push_str("&minuto_utc=");
                url.push_str(m);
            }
            if let Some(c) = &cursor {
                url.push_str("&cursor=");
                url.push_str(&urlencode(c));
            }
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .with_context(|| format!("requesting {url}"))?;
            let status = resp.status();
            let body = resp.bytes().await.context("reading ITS body")?;
            if !status.is_success() {
                anyhow::bail!(
                    "ITS returned {status}: {}",
                    String::from_utf8_lossy(&body[..body.len().min(200)])
                );
            }
            out.bytes += body.len();
            out.pages += 1;
            let env: Envelope = serde_json::from_slice(&body).context("parsing ITS page")?;
            if out.minute.is_none() {
                out.minute = env.minuto_utc.clone();
            }
            if let Some(v) = env.providers_status.as_ref().and_then(|v| v.as_object()) {
                for (k, val) in v {
                    out.providers.insert(
                        k.clone(),
                        val.as_str().map(str::to_string).unwrap_or_else(|| val.to_string()),
                    );
                }
            }
            out.rows += env.data.len();
            for row in env.data {
                if let Some(t) = row.datetime.as_deref().and_then(timeutil::parse_iso8601) {
                    if let Some(ts) = row
                        .datetime_servidor
                        .as_deref()
                        .and_then(timeutil::parse_iso8601)
                    {
                        let sec = ts.rem_euclid(60);
                        out.max_server_second =
                            Some(out.max_server_second.map_or(sec, |m: i64| m.max(sec)));
                    }
                    if let Some(f) = to_fix(row, t) {
                        out.fixes.push(f);
                    }
                }
            }
            match env.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => break,
            }
        }
        Ok(out)
    }
}

fn to_fix(row: Row, t: i64) -> Option<Fix> {
    let id = row.id_veiculo?;
    if id.trim().is_empty() {
        return None;
    }
    let (lat, lon) = (row.latitude?, row.longitude?);
    let vendor = Vendor::from_feed(row.fornecedor.as_deref().unwrap_or(""));
    let mut f = Fix::new(id, vendor, t, lat, lon);
    f.t_server = row
        .datetime_servidor
        .as_deref()
        .and_then(timeutil::parse_iso8601);
    f.heading = row
        .direcao
        .map(|d| d as f32)
        .filter(|d| d.is_finite() && *d > 0.0 && *d <= 360.0);
    f.service = row.servico.filter(|s| !s.trim().is_empty());
    f.dir = row.sentido.as_deref().and_then(Dir::from_feed);
    f.shape_id = row.shape_id.filter(|s| !s.trim().is_empty());
    f.trip_id = row.trip_id.filter(|s| !s.trim().is_empty());
    f.brt = row
        .modo
        .as_deref()
        .is_some_and(|m| m.eq_ignore_ascii_case("brt"))
        || vendor == Vendor::Sonda;
    Some(f)
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_aggregator_page() {
        let body = r#"{
          "minuto_utc": "2026-09-20T11:35:00Z",
          "data": [
            {"id_veiculo":"B58021","fornecedor":"zirix","sistema":"sppo","modo":"onibus",
             "servico":"474","sentido":"ida","latitude":-22.81592,"longitude":-43.28157,
             "velocidade":0.0,"direcao":203.0,"route_id":null,"trip_id":null,"shape_id":"",
             "datetime":"2026-09-20T11:35:07Z","datetime_envio":"2026-09-20T11:35:07Z",
             "datetime_servidor":"2026-09-20T11:35:41Z"},
            {"id_veiculo":"901008","fornecedor":"sonda","sistema":null,"modo":"brt",
             "servico":"22","sentido":"volta","latitude":-23.0008,"longitude":-43.3643,
             "velocidade":14.6,"direcao":null,"shape_id":"ivlc","trip_id":"abc",
             "datetime":"2026-09-20T11:35:13Z","datetime_servidor":null},
            {"id_veiculo":"","fornecedor":"zirix","latitude":-22.9,"longitude":-43.2,
             "datetime":"2026-09-20T11:35:13Z"},
            {"id_veiculo":"NOPOS","fornecedor":"zirix","datetime":"2026-09-20T11:35:13Z"}
          ],
          "next_cursor": null,
          "providers_status": {"zirix":"ok","conecta":"pending"}
        }"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        assert_eq!(env.data.len(), 4);
        let fixes: Vec<Fix> = env
            .data
            .into_iter()
            .filter_map(|r| {
                let t = r.datetime.as_deref().and_then(timeutil::parse_iso8601)?;
                to_fix(r, t)
            })
            .collect();
        // The blank id and the row without coordinates are dropped.
        assert_eq!(fixes.len(), 2);

        let z = &fixes[0];
        assert_eq!(z.vendor, Vendor::Zirix);
        assert_eq!(z.service.as_deref(), Some("474"));
        assert_eq!(z.dir, Some(Dir::Ida));
        assert_eq!(z.heading, Some(203.0));
        assert!(z.shape_id.is_none(), "empty shape_id must not be kept");
        assert!(!z.brt);
        assert_eq!(z.t_server, timeutil::parse_iso8601("2026-09-20T11:35:41Z"));

        let s = &fixes[1];
        assert_eq!(s.vendor, Vendor::Sonda);
        assert!(s.brt);
        assert_eq!(s.dir, Some(Dir::Volta));
        assert_eq!(s.heading, None, "null direcao stays unknown");
        assert_eq!(s.t_server, None);
    }

    #[test]
    fn cursors_are_escaped() {
        assert_eq!(urlencode("eyJvIjoxfQ=="), "eyJvIjoxfQ%3D%3D");
        assert_eq!(urlencode("plain-1_2.3~"), "plain-1_2.3~");
    }
}
