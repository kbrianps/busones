//! Runtime configuration, read from the environment so the systemd unit is the
//! single place deployment differs from a laptop.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// The service table shipped with the repository.
pub const DEFAULT_ALIASES: &str = "config/service-aliases.json";

pub struct Config {
    pub its_url: String,
    pub brt_url: String,
    pub gtfs_path: PathBuf,
    pub aliases_path: PathBuf,
    pub runtime_dir: PathBuf,
    /// Where the history store will live once stop events are computed.
    #[allow(dead_code)]
    pub state_dir: PathBuf,
    pub status_addr: SocketAddr,
    pub cell_zoom: u8,
    pub warm_minutes: i64,
    pub publish_interval: Duration,
    pub brt_interval: Duration,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        Ok(Config {
            its_url: env_or("BUSONES_ITS_URL", crate::ingest::its::DEFAULT_URL),
            brt_url: env_or(
                "BUSONES_BRT_URL",
                "https://dados.mobilidade.rio/gtfs/realtime/brt/vehicle-positions",
            ),
            gtfs_path: env_or("BUSONES_GTFS", "data/gtfs.json.gz").into(),
            aliases_path: env_or("BUSONES_ALIASES", DEFAULT_ALIASES).into(),
            runtime_dir: env_or("BUSONES_RUNTIME_DIR", "run").into(),
            state_dir: env_or("BUSONES_STATE_DIR", "state").into(),
            status_addr: env_or("BUSONES_STATUS_ADDR", "127.0.0.1:8081").parse()?,
            cell_zoom: env_or("BUSONES_CELL_ZOOM", "13").parse()?,
            warm_minutes: env_or("BUSONES_WARM_MINUTES", "10").parse()?,
            publish_interval: Duration::from_secs(env_or("BUSONES_PUBLISH_SECS", "10").parse()?),
            brt_interval: Duration::from_secs(env_or("BUSONES_BRT_SECS", "30").parse()?),
        })
    }
}
