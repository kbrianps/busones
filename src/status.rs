//! The health endpoint.
//!
//! It is served by this process, never as a file: a snapshot on disk would
//! keep answering "ok" long after the publisher died, which is exactly the
//! failure an external monitor is there to catch.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Default)]
pub struct Source {
    pub state: String,
    pub last_ok: Option<i64>,
    pub last_error: Option<String>,
    pub rows: usize,
    pub bytes: usize,
    pub fetch_ms: u64,
    pub pages: usize,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, String>,
}

#[derive(Clone, Serialize, Default)]
pub struct Status {
    pub status: String,
    pub generated_at: i64,
    pub started_at: i64,
    pub uptime_s: i64,
    pub warming: bool,
    pub newest_fix_age_s: i64,
    pub fleet_in_service: usize,
    pub vehicles: BTreeMap<String, usize>,
    pub sources: BTreeMap<String, Source>,
    pub disk: String,
    pub disk_used_pct: u32,
    pub feed_version: String,
    pub published_files: usize,
    pub publish_ms: u64,
}

pub type Shared = Arc<Mutex<Status>>;

/// Percentage of the filesystem holding `path` that is in use.
pub fn disk_used_pct(path: &std::path::Path) -> u32 {
    let c = match std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    let total = st.f_blocks as f64;
    if total <= 0.0 {
        return 0;
    }
    let avail = st.f_bavail as f64;
    (((total - avail) / total) * 100.0).round() as u32
}

#[derive(Clone)]
pub struct AppState {
    pub status: Shared,
    pub geo: std::sync::Arc<crate::geocode::Geocoder>,
}

pub async fn serve(addr: std::net::SocketAddr, state: AppState) -> anyhow::Result<()> {
    use axum::{routing::get, Router};
    let app = Router::new()
        .route("/api/v1/status.json", get(handler))
        .route("/healthz", get(handler))
        .route("/api/v1/geocode", get(geocode))
        .route("/api/v1/reverse", get(reverse))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "status and geocoding endpoints listening");
    axum::serve(listener, app).await?;
    Ok(())
}

type Q = axum::extract::Query<std::collections::HashMap<String, String>>;

fn json_publico(status: axum::http::StatusCode, body: String) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "application/json; charset=utf-8"),
            // Places do not move: let the edge and the browser keep them a day.
            (axum::http::header::CACHE_CONTROL, "public, max-age=86400"),
            (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        body,
    )
        .into_response()
}

async fn geocode(axum::extract::State(st): axum::extract::State<AppState>, q: Q) -> axum::response::Response {
    let texto = q.get("q").map(String::as_str).unwrap_or("");
    match st.geo.busca(texto).await {
        Ok(v) => json_publico(axum::http::StatusCode::OK, serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())),
        Err(e) => {
            tracing::warn!(error = %e, "geocode failed");
            json_publico(axum::http::StatusCode::BAD_GATEWAY, r#"{"error":"geocoder unavailable"}"#.into())
        }
    }
}

async fn reverse(axum::extract::State(st): axum::extract::State<AppState>, q: Q) -> axum::response::Response {
    let (Some(lat), Some(lon)) = (
        q.get("lat").and_then(|v| v.parse::<f64>().ok()),
        q.get("lon").and_then(|v| v.parse::<f64>().ok()),
    ) else {
        return json_publico(axum::http::StatusCode::BAD_REQUEST, r#"{"error":"lat and lon required"}"#.into());
    };
    match st.geo.reverso(lat, lon).await {
        Ok(v) => json_publico(axum::http::StatusCode::OK, serde_json::to_string(&v).unwrap_or_else(|_| "null".into())),
        Err(e) => {
            tracing::warn!(error = %e, "reverse geocode failed");
            json_publico(axum::http::StatusCode::BAD_GATEWAY, r#"{"error":"geocoder unavailable"}"#.into())
        }
    }
}

async fn handler(
    axum::extract::State(st): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    let shared = st.status;
    let body = {
        let s = shared.lock().unwrap();
        serde_json::to_string(&*s).unwrap_or_else(|_| "{}".into())
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
}
