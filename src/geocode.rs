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
const MIN_INTERVAL: Duration = Duration::from_millis(1_100);
const MAX_CACHE: usize = 5_000;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
    /// "Rua Voluntários da Pátria, 190", or the place's own name.
    pub name: String,
    /// Neighbourhood, when known.
    pub area: String,
}

pub struct Geocoder {
    http: reqwest::Client,
    cache: Mutex<HashMap<String, (Instant, Vec<Place>)>>,
    last: Mutex<Instant>,
}

/// Lowercased, trimmed, single-spaced: the cache key for a search.
pub fn normalize(q: &str) -> String {
    q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn str_field(v: &serde_json::Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// One Nominatim result (jsonv2 with address details) to our shape.
pub fn to_place(r: &serde_json::Value) -> Option<Place> {
    let lat: f64 = r.get("lat")?.as_str()?.parse().ok()?;
    let lon: f64 = r.get("lon")?.as_str()?.parse().ok()?;
    let a = r.get("address").cloned().unwrap_or_default();
    let street = ["road", "pedestrian", "footway", "square"].iter().map(|k| str_field(&a, k)).find(|s| !s.is_empty()).unwrap_or_default();
    let number = str_field(&a, "house_number");
    let own_name = str_field(r, "name");
    let name = if !own_name.is_empty() && (street.is_empty() || str_field(r, "category") != "highway" && number.is_empty()) {
        own_name
    } else if !street.is_empty() {
        if number.is_empty() { street } else { format!("{street}, {number}") }
    } else {
        str_field(r, "display_name").split(',').next().unwrap_or("").trim().to_string()
    };
    let area = ["suburb", "neighbourhood", "quarter", "city_district"].iter().map(|k| str_field(&a, k)).find(|s| !s.is_empty()).unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    Some(Place { lat, lon, name, area })
}

/// OpenStreetMap stores a street as many short segments, and Nominatim
/// returns each one: "Rua Bornéo" came back three times, 350 m apart. A result
/// with the same name as one already kept and within 600 m of it is another
/// piece of the same street. A long avenue still appears once per stretch.
fn one_per_street(places: Vec<Place>) -> Vec<Place> {
    let mut kept: Vec<Place> = Vec::new();
    for p in places {
        let same = kept.iter().any(|k| {
            let dy = (k.lat - p.lat) * 111_320.0;
            let dx = (k.lon - p.lon) * 111_320.0 * k.lat.to_radians().cos();
            k.name.to_lowercase() == p.name.to_lowercase() && dx.hypot(dy) < 600.0
        });
        if !same {
            kept.push(p);
        }
    }
    kept
}

impl Geocoder {
    pub fn new(http: reqwest::Client) -> Geocoder {
        Geocoder {
            http,
            cache: Mutex::new(HashMap::new()),
            last: Mutex::new(Instant::now() - MIN_INTERVAL),
        }
    }

    async fn request(&self, url: &str) -> Result<serde_json::Value> {
        // Nominatim allows one request per second; queue behind the last one.
        let mut last = self.last.lock().await;
        let wait = MIN_INTERVAL.saturating_sub(last.elapsed());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let r = self.http.get(url).header("Accept-Language", "pt-BR").send().await;
        *last = Instant::now();
        drop(last);
        let r = r.context("Nominatim unreachable")?;
        if !r.status().is_success() {
            anyhow::bail!("Nominatim returned {}", r.status());
        }
        let body = r.bytes().await.context("reading Nominatim response")?;
        Ok(serde_json::from_slice(&body)?)
    }

    async fn cached<F, Fut>(&self, key: String, search: F) -> Result<Vec<Place>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Place>>>,
    {
        if let Some((t, v)) = self.cache.lock().await.get(&key) {
            if t.elapsed() < TTL {
                return Ok(v.clone());
            }
        }
        let v = search().await?;
        let mut c = self.cache.lock().await;
        if c.len() >= MAX_CACHE {
            c.clear();
        }
        c.insert(key, (Instant::now(), v.clone()));
        Ok(v)
    }

    pub async fn search(&self, q: &str) -> Result<Vec<Place>> {
        let q = normalize(q);
        if q.chars().count() < 3 {
            return Ok(Vec::new());
        }
        let key = format!("s:{q}");
        self.cached(key, || async {
            let url = format!(
                "{BASE}/search?format=jsonv2&addressdetails=1&limit=8&countrycodes=br&bounded=1\
                 &viewbox=-43.80,-22.74,-43.09,-23.08&q={}",
                urlencode(&format!("{q}, Rio de Janeiro"))
            );
            let v = self.request(&url).await?;
            let places: Vec<Place> = v.as_array().map(|a| a.iter().filter_map(to_place).collect()).unwrap_or_default();
            Ok(one_per_street(places))
        })
        .await
    }

    pub async fn reverse(&self, lat: f64, lon: f64) -> Result<Option<Place>> {
        // Four decimals is about 11 m: close enough to share one answer.
        let key = format!("r:{lat:.4},{lon:.4}");
        let v = self
            .cached(key, || async {
                let url = format!("{BASE}/reverse?format=jsonv2&addressdetails=1&zoom=18&lat={lat:.5}&lon={lon:.5}");
                let v = self.request(&url).await?;
                Ok(to_place(&v).map(|mut l| {
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
        assert_eq!(normalize("  Rua   Voluntários  da Pátria "), "rua voluntários da pátria");
    }

    #[test]
    fn an_address_reads_as_street_and_number() {
        let r = serde_json::json!({
            "lat": "-22.9523", "lon": "-43.1901", "category": "place", "name": "",
            "display_name": "190, Rua Voluntários da Pátria, Botafogo, Rio de Janeiro",
            "address": {"house_number": "190", "road": "Rua Voluntários da Pátria", "suburb": "Botafogo"}
        });
        let l = to_place(&r).unwrap();
        assert_eq!(l.name, "Rua Voluntários da Pátria, 190");
        assert_eq!(l.area, "Botafogo");
        assert!((l.lat + 22.9523).abs() < 1e-9);
    }

    #[test]
    fn a_named_place_keeps_its_own_name() {
        let r = serde_json::json!({
            "lat": "-22.9711", "lon": "-43.1822", "category": "amenity", "name": "Copacabana Palace",
            "address": {"road": "Avenida Atlântica", "suburb": "Copacabana"}
        });
        assert_eq!(to_place(&r).unwrap().name, "Copacabana Palace");
    }

    #[test]
    fn a_street_without_number_is_just_the_street() {
        let r = serde_json::json!({
            "lat": "-22.9", "lon": "-43.2", "category": "highway", "name": "Rua do Catete",
            "address": {"road": "Rua do Catete", "suburb": "Catete"}
        });
        assert_eq!(to_place(&r).unwrap().name, "Rua do Catete");
    }

    #[test]
    fn segments_of_one_street_come_back_once() {
        let p = |lat: f64, lon: f64, name: &str, area: &str| Place { lat, lon, name: name.into(), area: area.into() };
        let got = one_per_street(vec![
            p(-22.879024, -43.3300986, "Rua Bornéo", "Cascadura"),
            p(-22.8789683, -43.3280275, "Rua Bornéo", "Cascadura"),
            p(-22.8791045, -43.3314571, "Rua Bornéo", "Madureira"),
            p(-22.8791045, -43.3314571, "Rua Bornéo, 304", "Madureira"),
            p(-22.8200000, -43.2500000, "Avenida Brasil", "Penha"),
            p(-22.8700000, -43.4500000, "Avenida Brasil", "Realengo"),
        ]);
        let names: Vec<(&str, &str)> = got.iter().map(|p| (p.name.as_str(), p.area.as_str())).collect();
        assert_eq!(
            names,
            [("Rua Bornéo", "Cascadura"), ("Rua Bornéo, 304", "Madureira"), ("Avenida Brasil", "Penha"), ("Avenida Brasil", "Realengo")]
        );
    }

    #[test]
    fn nonsense_is_dropped() {
        assert!(to_place(&serde_json::json!({"lat": "x", "lon": "1"})).is_none());
    }
}
