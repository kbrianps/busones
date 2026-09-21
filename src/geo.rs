//! Local planar projection for Rio de Janeiro plus the small amount of
//! computational geometry the matcher needs.
//!
//! Everything downstream works in metres on an equirectangular plane anchored
//! at the middle of the municipality. Over Rio's 0.4 degree latitude span the
//! scale error stays under 0.1%, i.e. centimetres at the 30 m thresholds the
//! matcher uses, and a single linear map keeps distances consistent between
//! shapes, stops and vehicles.

pub const LAT0: f64 = -22.9;
pub const LON0: f64 = -43.4;
const EARTH_R: f64 = 6_371_000.0;

#[inline]
fn m_per_deg_lat() -> f64 {
    EARTH_R * std::f64::consts::PI / 180.0
}

#[inline]
fn m_per_deg_lon() -> f64 {
    EARTH_R * std::f64::consts::PI / 180.0 * (LAT0 * std::f64::consts::PI / 180.0).cos()
}

/// Latitude/longitude in degrees to metres east/north of the anchor.
#[inline]
pub fn project(lat: f64, lon: f64) -> (f32, f32) {
    (
        ((lon - LON0) * m_per_deg_lon()) as f32,
        ((lat - LAT0) * m_per_deg_lat()) as f32,
    )
}

/// Inverse of [`project`]. Used by the export path and by tests.
#[allow(dead_code)]
#[inline]
pub fn unproject(x: f32, y: f32) -> (f64, f64) {
    (
        LAT0 + y as f64 / m_per_deg_lat(),
        LON0 + x as f64 / m_per_deg_lon(),
    )
}

#[inline]
pub fn dist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt()
}

/// Projection of `p` onto segment `a..b`: returns `(t, distance)` where `t` is
/// the clamped position along the segment in `0..=1`.
#[inline]
pub fn project_on_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> (f32, f32) {
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let qx = ax + t * dx;
    let qy = ay + t * dy;
    (t, dist(px, py, qx, qy))
}

/// Compass bearing in degrees (0 = north, clockwise) of the vector `a -> b`.
#[inline]
pub fn bearing_deg(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let b = (bx - ax).atan2(by - ay).to_degrees();
    if b < 0.0 {
        b + 360.0
    } else {
        b
    }
}

/// Smallest absolute difference between two bearings, in degrees (0..=180).
#[inline]
pub fn bearing_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 {
        360.0 - d
    } else {
        d
    }
}

/// Bounding box of the municipality plus a margin, used to drop impossible fixes.
pub const RIO_MIN_LAT: f64 = -23.15;
pub const RIO_MAX_LAT: f64 = -22.65;
pub const RIO_MIN_LON: f64 = -43.90;
pub const RIO_MAX_LON: f64 = -43.00;

#[inline]
pub fn in_rio(lat: f64, lon: f64) -> bool {
    (RIO_MIN_LAT..=RIO_MAX_LAT).contains(&lat) && (RIO_MIN_LON..=RIO_MAX_LON).contains(&lon)
}

/// Web Mercator tile of a coordinate, used for the per-area snapshots.
pub fn tile_of(lat: f64, lon: f64, z: u8) -> (u32, u32) {
    let n = (1u64 << z) as f64;
    let x = ((lon + 180.0) / 360.0 * n).floor().clamp(0.0, n - 1.0);
    let lat_rad = lat.to_radians();
    let y = ((1.0 - (lat_rad.tan() + 1.0 / lat_rad.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor()
        .clamp(0.0, n - 1.0);
    (x as u32, y as u32)
}

/// Every tile at zoom `z` that intersects the Rio bounding box.
pub fn rio_tiles(z: u8) -> Vec<(u32, u32)> {
    let (x0, y0) = tile_of(RIO_MAX_LAT, RIO_MIN_LON, z);
    let (x1, y1) = tile_of(RIO_MIN_LAT, RIO_MAX_LON, z);
    let mut out = Vec::new();
    for x in x0.min(x1)..=x0.max(x1) {
        for y in y0.min(y1)..=y0.max(y1) {
            out.push((x, y));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_roundtrips() {
        for (lat, lon) in [(-22.9083, -43.1964), (-23.05, -43.6), (-22.75, -43.15)] {
            let (x, y) = project(lat, lon);
            let (lat2, lon2) = unproject(x, y);
            assert!((lat - lat2).abs() < 1e-6, "{lat} vs {lat2}");
            assert!((lon - lon2).abs() < 1e-6, "{lon} vs {lon2}");
        }
    }

    #[test]
    fn projection_scale_is_metres() {
        // One degree of latitude is about 111 km.
        let (_, y0) = project(-22.9, -43.4);
        let (_, y1) = project(-21.9, -43.4);
        assert!((y1 - y0 - 111_195.0).abs() < 200.0, "{}", y1 - y0);
        // Copacabana to Maracana is roughly 6 km.
        let (ax, ay) = project(-22.9711, -43.1822);
        let (bx, by) = project(-22.9121, -43.2302);
        let d = dist(ax, ay, bx, by);
        assert!((7500.0..8800.0).contains(&d), "{d}");
    }

    #[test]
    fn segment_projection() {
        let (t, d) = project_on_segment(5.0, 3.0, 0.0, 0.0, 10.0, 0.0);
        assert!((t - 0.5).abs() < 1e-6);
        assert!((d - 3.0).abs() < 1e-6);
        // Clamps past the end of the segment.
        let (t, d) = project_on_segment(20.0, 0.0, 0.0, 0.0, 10.0, 0.0);
        assert!((t - 1.0).abs() < 1e-6);
        assert!((d - 10.0).abs() < 1e-6);
    }

    #[test]
    fn bearings() {
        assert!((bearing_deg(0.0, 0.0, 0.0, 1.0) - 0.0).abs() < 1e-3);
        assert!((bearing_deg(0.0, 0.0, 1.0, 0.0) - 90.0).abs() < 1e-3);
        assert!((bearing_deg(0.0, 0.0, 0.0, -1.0) - 180.0).abs() < 1e-3);
        assert!((bearing_diff(10.0, 350.0) - 20.0).abs() < 1e-3);
        assert!((bearing_diff(350.0, 10.0) - 20.0).abs() < 1e-3);
    }

    #[test]
    fn tiles_cover_rio() {
        let t = rio_tiles(13);
        assert!(t.len() > 100 && t.len() < 900, "{} tiles", t.len());
        let centro = tile_of(-22.9068, -43.1729, 13);
        assert!(t.contains(&centro));
        let campo_grande = tile_of(-22.9, -43.56, 13);
        assert!(t.contains(&campo_grande));
    }
}
