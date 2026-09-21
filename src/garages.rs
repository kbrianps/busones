//! Garage areas, from `config/garages.json` (generated and explained by
//! `scripts/garage-evidence.py`).
//!
//! A bus standing inside one is not carrying passengers, whatever line it
//! last showed, so it leaves the map and the arrivals. At night, buses
//! returning to the garage with their last line still set were the ghost
//! buses riders complain about.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

use crate::geo;

#[derive(Deserialize)]
struct Area {
    lat0: f64,
    lon0: f64,
    lat1: f64,
    lon1: f64,
}

#[derive(Deserialize, Default)]
struct GarageFile {
    #[serde(default)]
    garages: Vec<Area>,
}

/// Boxes in the projected metres the engine works in, `[x0, y0, x1, y1]`.
/// A missing file means no garages; a malformed one is an error.
pub fn load(path: &Path) -> Result<Vec<[f32; 4]>> {
    let file: GarageFile = match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parsing {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => GarageFile::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(file
        .garages
        .iter()
        .map(|a| {
            let (xa, ya) = geo::project(a.lat0, a.lon0);
            let (xb, yb) = geo::project(a.lat1, a.lon1);
            [xa.min(xb), ya.min(yb), xa.max(xb), ya.max(yb)]
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn areas_load_as_projected_boxes() {
        let dir = std::env::temp_dir().join(format!("busones-garages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("garages.json");
        std::fs::write(&p, r#"{"garages":[{"buses":303,"lat0":-22.9541,"lon0":-43.3516,"lat1":-22.9492,"lon1":-43.3469}]}"#).unwrap();
        let boxes = load(&p).unwrap();
        assert_eq!(boxes.len(), 1);
        let (x, y) = geo::project(-22.9517, -43.3493);
        let b = boxes[0];
        assert!(b[0] < x && x < b[2] && b[1] < y && y < b[3], "the centre is inside");
        assert!(load(&dir.join("missing.json")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
