//! Address search and reverse geocoding, through Nominatim, with a cache.
//!
//! onibus-rj proxied Nominatim from a Cloudflare Worker; here the backend does
//! it. Nominatim's usage policy is the design constraint: at most one request
//! per second, an identifying User-Agent, results cached, and no searching on
//! every keystroke. The client therefore shows neighbourhoods and stops from
//! our own data while you type, and asks for a full address only when you
//! confirm the search.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const BASE: &str = "https://nominatim.openstreetmap.org";
const TTL: Duration = Duration::from_secs(7 * 24 * 3600);
const INTERVALO: Duration = Duration::from_millis(1_100);
const MAX_CACHE: usize = 5_000;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Lugar {
    pub lat: f64,
    pub lon: f64,
    /// "Rua Voluntários da Pátria, 190", or the place's own name.
    pub nome: String,
    /// Neighbourhood, when known.
    pub area: String,
}

pub struct Geocoder {
    http: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, Vec<Lugar>)>>,
    ultimo: Mutex<Instant>,
}

/// Lowercased, trimmed, single-spaced: the cache key for a search.
pub fn normaliza(q: &str) -> String {
    q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn texto(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// One Nominatim result (jsonv2 with address details) to our shape.
pub fn para_lugar(r: &serde_json::Value) -> Option<Lugar> {
    let lat: f64 = r.get("lat")?.as_str()?.parse().ok()?;
    let lon: f64 = r.get("lon")?.as_str()?.parse().ok()?;
    let a = r.get("address").cloned().unwrap_or_default();
    let rua = ["road", "pedestrian", "footway", "square"].iter().map(|k| texto(&a, k)).find(|s| !s.is_empty()).unwrap_or_default();
    let numero = texto(&a, "house_number");
    let proprio = texto(r, "name");
    let nome = if !proprio.is_empty() && (rua.is_empty() || texto(r, "category") != "highway" && numero.is_empty()) {
        proprio
    } else if !rua.is_empty() {
        if numero.is_empty() { rua } else { format!("{rua}, {numero}") }
    } else {
        texto(r, "display_name").split(',').next().unwrap_or("").trim().to_string()
    };
    let area = ["suburb", "neighbourhood", "quarter", "city_district"].iter().map(|k| texto(&a, k)).find(|s| !s.is_empty()).unwrap_or_default();
    if nome.is_empty() {
        return None;
    }
    Some(Lugar { lat, lon, nome, area })
}

impl Geocoder {
    pub fn new(http: reqwest::Client) -> Geocoder {
        Geocoder {
            http,
            cache: Mutex::new(HashMap::new()),
            ultimo: Mutex::new(Instant::now() - INTERVALO),
        }
    }

    async fn pede(&self, url: &str) -> Result<serde_json::Value> {
        // Nominatim allows one request per second; queue behind the last one.
        let mut ultimo = self.ultimo.lock().await;
        let espera = INTERVALO.saturating_sub(ultimo.elapsed());
        if !espera.is_zero() {
            tokio::time::sleep(espera).await;
        }
        let r = self.http.get(url).header("Accept-Language", "pt-BR").send().await;
        *ultimo = Instant::now();
        drop(ultimo);
        let r = r.context("Nominatim unreachable")?;
        if !r.status().is_success() {
            anyhow::bail!("Nominatim returned {}", r.status());
        }
        let corpo = r.bytes().await.context("reading Nominatim response")?;
        Ok(serde_json::from_slice(&corpo)?)
    }

    async fn em_cache<F, Fut>(&self, chave: String, busca: F) -> Result<Vec<Lugar>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Lugar>>>,
    {
        if let Some((t, v)) = self.cache.lock().await.get(&chave) {
            if t.elapsed() < TTL {
                return Ok(v.clone());
            }
        }
        let v = busca().await?;
        let mut c = self.cache.lock().await;
        if c.len() >= MAX_CACHE {
            c.clear();
        }
        c.insert(chave, (Instant::now(), v.clone()));
        Ok(v)
    }

    pub async fn busca(&self, q: &str) -> Result<Vec<Lugar>> {
        let q = normaliza(q);
        if q.chars().count() < 3 {
            return Ok(Vec::new());
        }
        let chave = format!("s:{q}");
        self.em_cache(chave, || async {
            let url = format!(
                "{BASE}/search?format=jsonv2&addressdetails=1&limit=8&countrycodes=br&bounded=1\
                 &viewbox=-43.80,-22.74,-43.09,-23.08&q={}",
                urlencode(&format!("{q}, Rio de Janeiro"))
            );
            let v = self.pede(&url).await?;
            Ok(v.as_array().map(|a| a.iter().filter_map(para_lugar).collect()).unwrap_or_default())
        })
        .await
    }

    pub async fn reverso(&self, lat: f64, lon: f64) -> Result<Option<Lugar>> {
        // Four decimals is about 11 m: close enough to share one answer.
        let chave = format!("r:{lat:.4},{lon:.4}");
        let v = self
            .em_cache(chave, || async {
                let url = format!("{BASE}/reverse?format=jsonv2&addressdetails=1&zoom=18&lat={lat:.5}&lon={lon:.5}");
                let v = self.pede(&url).await?;
                Ok(para_lugar(&v).map(|mut l| {
                    l.lat = lat;
                    l.lon = lon;
                    l
                }).into_iter().collect())
            })
            .await?;
        Ok(v.into_iter().next())
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_normalise_for_the_cache() {
        assert_eq!(normaliza("  Rua   Voluntários  da Pátria "), "rua voluntários da pátria");
    }

    #[test]
    fn an_address_reads_as_street_and_number() {
        let r = serde_json::json!({
            "lat": "-22.9523", "lon": "-43.1901", "category": "place", "name": "",
            "display_name": "190, Rua Voluntários da Pátria, Botafogo, Rio de Janeiro",
            "address": {"house_number": "190", "road": "Rua Voluntários da Pátria", "suburb": "Botafogo"}
        });
        let l = para_lugar(&r).unwrap();
        assert_eq!(l.nome, "Rua Voluntários da Pátria, 190");
        assert_eq!(l.area, "Botafogo");
        assert!((l.lat + 22.9523).abs() < 1e-9);
    }

    #[test]
    fn a_named_place_keeps_its_own_name() {
        let r = serde_json::json!({
            "lat": "-22.9711", "lon": "-43.1822", "category": "amenity", "name": "Copacabana Palace",
            "address": {"road": "Avenida Atlântica", "suburb": "Copacabana"}
        });
        assert_eq!(para_lugar(&r).unwrap().nome, "Copacabana Palace");
    }

    #[test]
    fn a_street_without_number_is_just_the_street() {
        let r = serde_json::json!({
            "lat": "-22.9", "lon": "-43.2", "category": "highway", "name": "Rua do Catete",
            "address": {"road": "Rua do Catete", "suburb": "Catete"}
        });
        assert_eq!(para_lugar(&r).unwrap().nome, "Rua do Catete");
    }

    #[test]
    fn nonsense_is_dropped() {
        assert!(para_lugar(&serde_json::json!({"lat": "x", "lon": "1"})).is_none());
    }
}
