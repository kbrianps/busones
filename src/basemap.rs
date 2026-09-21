//! Our own basemap: a few layers of OpenStreetMap, drawn in the browser.
//!
//! The standard OSM raster style puts everything on screen: in one block of
//! downtown Rio a z15 tile carries 560 buildings and 1,245 points of interest,
//! of which 524 are trees, 105 benches and 73 pedestrian crossings. At a bus
//! stop none of that helps. What does is water, green areas, streets and the
//! names of neighbourhoods and main avenues, so that is all we keep.
//!
//! Input is a Protomaps PMTiles extract of the municipality (vector tiles,
//! ODbL). Output is one small JSON file per tile, coordinates quantised to a
//! 1024 grid, plus a single file of place names.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use crate::geo;
use crate::pb::Buf;

/// Zooms we publish. The client uses the nearest one at or below its own zoom
/// and scales vectors, which stay sharp, so three levels cover 11 to 18.
pub const ZOOMS: [u8; 3] = [11, 13, 15];
const Q: f64 = 1024.0;

/* ---------- PMTiles v3 ---------- */

struct Header {
    root_off: u64,
    root_len: u64,
    leaf_off: u64,
    tile_off: u64,
    internal: u8,
    tile_comp: u8,
}

fn u64le(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

fn header(b: &[u8]) -> Result<Header> {
    if b.len() < 127 || &b[0..7] != b"PMTiles" || b[7] != 3 {
        bail!("not a PMTiles v3 archive");
    }
    Ok(Header {
        root_off: u64le(b, 8),
        root_len: u64le(b, 16),
        leaf_off: u64le(b, 40),
        tile_off: u64le(b, 56),
        internal: b[97],
        tile_comp: b[98],
    })
}

fn inflate(b: &[u8], comp: u8) -> Result<Vec<u8>> {
    match comp {
        0 | 1 => Ok(b.to_vec()),
        2 => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(b).read_to_end(&mut out)?;
            Ok(out)
        }
        c => bail!("unsupported PMTiles compression {c}"),
    }
}

struct Entry {
    tile_id: u64,
    offset: u64,
    length: u64,
    run: u64,
}

fn directory(b: &[u8]) -> Result<Vec<Entry>> {
    let mut p = Buf::new(b);
    let n = p.varint().context("directory size")? as usize;
    let mut e: Vec<Entry> = (0..n).map(|_| Entry { tile_id: 0, offset: 0, length: 0, run: 0 }).collect();
    let mut last = 0u64;
    for x in e.iter_mut() {
        last += p.varint().context("tile ids")?;
        x.tile_id = last;
    }
    for x in e.iter_mut() {
        x.run = p.varint().context("run lengths")?;
    }
    for x in e.iter_mut() {
        x.length = p.varint().context("lengths")?;
    }
    for i in 0..n {
        let v = p.varint().context("offsets")?;
        e[i].offset = if v == 0 && i > 0 {
            e[i - 1].offset + e[i - 1].length
        } else {
            v.checked_sub(1).context("bad offset")?
        };
    }
    Ok(e)
}

/// Every tile in the archive as `(tile_id, absolute offset, length)`.
fn entries(file: &[u8], h: &Header, off: u64, len: u64, out: &mut Vec<(u64, u64, u64)>) -> Result<()> {
    let raw = file.get(off as usize..(off + len) as usize).context("directory out of range")?;
    for e in directory(&inflate(raw, h.internal)?)? {
        if e.run == 0 {
            entries(file, h, h.leaf_off + e.offset, e.length, out)?;
        } else {
            // A run shares one blob across consecutive ids, e.g. open ocean.
            for k in 0..e.run {
                out.push((e.tile_id + k, h.tile_off + e.offset, e.length));
            }
        }
    }
    Ok(())
}

/// PMTiles numbers tiles along a Hilbert curve, zoom by zoom.
pub fn zxy(id: u64) -> (u8, u32, u32) {
    let mut acc = 0u64;
    let mut z = 0u8;
    loop {
        let n = 1u64 << (2 * z as u64);
        if acc + n > id {
            break;
        }
        acc += n;
        z += 1;
    }
    let n = 1u64 << z;
    let (mut x, mut y) = (0u64, 0u64);
    let mut t = id - acc;
    let mut s = 1u64;
    while s < n {
        let rx = 1 & (t / 2);
        let ry = 1 & (t ^ rx);
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - x;
                y = s - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        x += s * rx;
        y += s * ry;
        t /= 4;
        s *= 2;
    }
    (z, x as u32, y as u32)
}

/* ---------- Mapbox Vector Tiles ---------- */

pub struct Feature {
    pub geom_type: u32,
    pub props: HashMap<String, String>,
    pub parts: Vec<Vec<(i32, i32)>>,
}

pub struct Layer {
    pub name: String,
    pub extent: u32,
    pub features: Vec<Feature>,
}

fn zigzag(v: u32) -> i32 {
    ((v >> 1) as i32) ^ -((v & 1) as i32)
}

fn geometry(cmds: &[u32]) -> Vec<Vec<(i32, i32)>> {
    let mut out = Vec::new();
    let mut cur: Vec<(i32, i32)> = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    let mut i = 0;
    while i < cmds.len() {
        let c = cmds[i];
        i += 1;
        let (id, count) = (c & 7, c >> 3);
        match id {
            1 | 2 => {
                for _ in 0..count {
                    if i + 1 >= cmds.len() {
                        return out;
                    }
                    let (dx, dy) = (zigzag(cmds[i]), zigzag(cmds[i + 1]));
                    i += 2;
                    x += dx;
                    y += dy;
                    if id == 1 && !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push((x, y));
                }
            }
            7 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => break,
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn value(b: &[u8]) -> String {
    let mut p = Buf::new(b);
    while !p.done() {
        let Some((f, w)) = p.tag() else { break };
        match (f, w) {
            (1, 2) => return p.bytes().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default(),
            (2, 5) => return p.fixed32().map(|v| f32::from_bits(v).to_string()).unwrap_or_default(),
            (3, 1) => return p.fixed64().map(|v| f64::from_bits(v).to_string()).unwrap_or_default(),
            (4, 0) | (5, 0) => return p.varint().map(|v| (v as i64).to_string()).unwrap_or_default(),
            (6, 0) => return p.varint().map(|v| (((v >> 1) as i64) ^ -((v & 1) as i64)).to_string()).unwrap_or_default(),
            (7, 0) => return p.varint().map(|v| (v != 0).to_string()).unwrap_or_default(),
            _ => {
                if p.skip(w).is_none() {
                    break;
                }
            }
        }
    }
    String::new()
}

pub fn decode(tile: &[u8]) -> Vec<Layer> {
    let mut layers = Vec::new();
    let mut p = Buf::new(tile);
    while !p.done() {
        let Some((f, w)) = p.tag() else { break };
        if (f, w) != (3, 2) {
            if p.skip(w).is_none() {
                break;
            }
            continue;
        }
        let Some(lb) = p.bytes() else { break };
        let mut l = Buf::new(lb);
        let (mut name, mut extent) = (String::new(), 4096u32);
        let (mut keys, mut vals, mut raw) = (Vec::new(), Vec::new(), Vec::new());
        while !l.done() {
            let Some((lf, lw)) = l.tag() else { break };
            match (lf, lw) {
                (1, 2) => name = l.bytes().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default(),
                (2, 2) => raw.extend(l.bytes()),
                (3, 2) => keys.push(l.bytes().map(|s| String::from_utf8_lossy(s).into_owned()).unwrap_or_default()),
                (4, 2) => vals.push(l.bytes().map(value).unwrap_or_default()),
                (5, 0) => extent = l.varint().unwrap_or(4096) as u32,
                _ => {
                    if l.skip(lw).is_none() {
                        break;
                    }
                }
            }
        }
        // Features usually precede the key and value tables in the stream.
        let mut features = Vec::with_capacity(raw.len());
        for fb in raw {
            let mut fp = Buf::new(fb);
            let (mut gt, mut tags, mut cmds) = (0u32, Vec::new(), Vec::new());
            while !fp.done() {
                let Some((ff, fw)) = fp.tag() else { break };
                match (ff, fw) {
                    (2, 2) => tags = fp.bytes().map(Buf::packed).unwrap_or_default(),
                    (3, 0) => gt = fp.varint().unwrap_or(0) as u32,
                    (4, 2) => cmds = fp.bytes().map(Buf::packed).unwrap_or_default(),
                    _ => {
                        if fp.skip(fw).is_none() {
                            break;
                        }
                    }
                }
            }
            let mut props = HashMap::new();
            for kv in tags.chunks(2) {
                if let [k, v] = kv {
                    if let (Some(k), Some(v)) = (keys.get(*k as usize), vals.get(*v as usize)) {
                        props.insert(k.clone(), v.clone());
                    }
                }
            }
            features.push(Feature { geom_type: gt, props, parts: geometry(&cmds) });
        }
        layers.push(Layer { name, extent, features });
    }
    layers
}

/* ---------- geometry helpers ---------- */

fn area(r: &[(f64, f64)]) -> f64 {
    let n = r.len();
    (0..n).map(|i| { let (a, b) = (r[i], r[(i + 1) % n]); a.0 * b.1 - b.0 * a.1 }).sum::<f64>() / 2.0
}

/// Douglas-Peucker, iterative, keeping both ends.
fn simplify(p: &[(f64, f64)], tol: f64) -> Vec<(f64, f64)> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[p.len() - 1] = true;
    let mut stack = vec![(0usize, p.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (ax, ay) = p[a];
        let (bx, by) = p[b];
        let (dx, dy) = (bx - ax, by - ay);
        let len = (dx * dx + dy * dy).sqrt().max(1e-9);
        let mut best = (0.0, 0usize);
        for (i, &(x, y)) in p.iter().enumerate().take(b).skip(a + 1) {
            let d = ((x - ax) * dy - (y - ay) * dx).abs() / len;
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tol {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    p.iter().zip(keep).filter(|(_, k)| *k).map(|(v, _)| *v).collect()
}

fn flatten(v: &[(f64, f64)]) -> Vec<i32> {
    let mut out = Vec::with_capacity(v.len() * 2);
    let mut last: Option<(i32, i32)> = None;
    for &(x, y) in v {
        let q = (x.round() as i32, y.round() as i32);
        if last != Some(q) {
            out.extend([q.0, q.1]);
            last = Some(q);
        }
    }
    out
}

/* ---------- build ---------- */

const VERDE: &[&str] = &[
    "park", "wood", "forest", "scrub", "grass", "garden", "nature_reserve", "golf_course",
    "recreation_ground", "cemetery", "grassland", "meadow", "heath", "national_park",
    "protected_area", "village_green",
];
const AREIA: &[&str] = &["beach", "sand"];
const AGUA_FORA: &[&str] = &["fountain", "swimming_pool"];

#[derive(Default, serde::Serialize)]
struct Tile {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    w: Vec<Vec<Vec<i32>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    g: Vec<Vec<Vec<i32>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    s: Vec<Vec<Vec<i32>>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    r0: Vec<Vec<i32>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    r1: Vec<Vec<i32>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    r2: Vec<Vec<i32>>,
    /// Road names: `[x, y, angle_degrees, name, class]`, class 0 or 1.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    l: Vec<(i32, i32, i32, String, u8)>,
}

impl Tile {
    fn is_empty(&self) -> bool {
        self.w.is_empty() && self.g.is_empty() && self.s.is_empty()
            && self.r0.is_empty() && self.r1.is_empty() && self.r2.is_empty()
    }
}

fn polygons(f: &Feature, scale: f64, tol: f64, min_area: f64) -> Vec<Vec<i32>> {
    let mut rings = Vec::new();
    let mut exterior_ok = false;
    for part in &f.parts {
        let pts: Vec<(f64, f64)> = part.iter().map(|&(x, y)| (x as f64 * scale, y as f64 * scale)).collect();
        let a = area(&pts);
        // Exterior rings have positive area in tile coordinates; a hole is kept
        // only when its exterior survived.
        if a > 0.0 {
            exterior_ok = a >= min_area;
            if !exterior_ok {
                continue;
            }
        } else if !exterior_ok || a.abs() < min_area {
            continue;
        }
        let s = flatten(&simplify(&pts, tol));
        if s.len() >= 6 {
            rings.push(s);
        }
    }
    rings
}

pub struct Stats {
    pub tiles: [usize; 3],
    pub bytes: usize,
    pub gz_bytes: usize,
    pub places: usize,
}

pub fn build(pmtiles: &Path, out: &Path) -> Result<Stats> {
    let file = std::fs::read(pmtiles).with_context(|| format!("reading {}", pmtiles.display()))?;
    let h = header(&file)?;
    let mut all = Vec::new();
    entries(&file, &h, h.root_off, h.root_len, &mut all)?;

    let mut stats = Stats { tiles: [0; 3], bytes: 0, gz_bytes: 0, places: 0 };
    let mut places: Vec<(f64, f64, String, u8)> = Vec::new();

    for (id, off, len) in all {
        let (z, x, y) = zxy(id);
        let Some(zi) = ZOOMS.iter().position(|&zz| zz == z) else { continue };
        let (x0, y0) = geo::tile_of(geo::RIO_MAX_LAT, geo::RIO_MIN_LON, z);
        let (x1, y1) = geo::tile_of(geo::RIO_MIN_LAT, geo::RIO_MAX_LON, z);
        if x < x0 || x > x1 || y < y0 || y > y1 {
            continue;
        }
        let raw = &file[off as usize..(off + len) as usize];
        let layers = decode(&inflate(raw, h.tile_comp)?);

        let mut t = Tile::default();
        for layer in &layers {
            let scale = Q / layer.extent as f64;
            let tol_area = if z >= 15 { 2.0 } else { 1.4 };
            let min_green = if z >= 15 { 120.0 } else { 60.0 };
            for f in &layer.features {
                let kind = f.props.get("kind").map(String::as_str).unwrap_or("");
                match (layer.name.as_str(), f.geom_type) {
                    ("water", 3) if !AGUA_FORA.contains(&kind) => {
                        let p = polygons(f, scale, tol_area, 20.0);
                        if !p.is_empty() { t.w.push(p); }
                    }
                    ("landuse" | "landcover", 3) if VERDE.contains(&kind) => {
                        let p = polygons(f, scale, tol_area, min_green);
                        if !p.is_empty() { t.g.push(p); }
                    }
                    ("landuse" | "landcover", 3) if AREIA.contains(&kind) => {
                        let p = polygons(f, scale, tol_area, 40.0);
                        if !p.is_empty() { t.s.push(p); }
                    }
                    ("roads", 2) => {
                        let road_class = match kind {
                            "highway" => 0,
                            "major_road" | "medium_road" => 1,
                            "minor_road" if z >= 13 => 2,
                            _ => continue,
                        };
                        let mut best: Option<(f64, (f64, f64), f64)> = None;
                        for part in &f.parts {
                            let pts: Vec<(f64, f64)> = part.iter().map(|&(px, py)| (px as f64 * scale, py as f64 * scale)).collect();
                            let s = simplify(&pts, 1.4);
                            let flat = flatten(&s);
                            if flat.len() < 4 { continue; }
                            match road_class {
                                0 => t.r0.push(flat),
                                1 => t.r1.push(flat),
                                _ => t.r2.push(flat),
                            }
                            // The longest straight stretch is where a name reads best.
                            for w in s.windows(2) {
                                let (a, b) = (w[0], w[1]);
                                let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                                if best.is_none_or(|m| d > m.0) {
                                    let mut ang = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
                                    if ang > 90.0 { ang -= 180.0; }
                                    if ang < -90.0 { ang += 180.0; }
                                    best = Some((d, ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), ang));
                                }
                            }
                        }
                        if z == 15 && road_class < 2 {
                            if let (Some(name), Some((d, (mx, my), ang))) = (f.props.get("name"), best) {
                                if d > 90.0 && (0.0..Q).contains(&mx) && (0.0..Q).contains(&my) {
                                    t.l.push((mx.round() as i32, my.round() as i32, ang.round() as i32, name.clone(), road_class));
                                }
                            }
                        }
                    }
                    ("places", 1) if matches!(kind, "locality" | "macrohood" | "neighbourhood" | "suburb") => {
                        if let (Some(name), Some(&(px, py))) = (f.props.get("name"), f.parts.first().and_then(|p| p.first())) {
                            let n = 1u64 << z;
                            let wx = (x as f64 + px as f64 / layer.extent as f64) / n as f64;
                            let wy = (y as f64 + py as f64 / layer.extent as f64) / n as f64;
                            let lon = wx * 360.0 - 180.0;
                            let lat = (std::f64::consts::PI * (1.0 - 2.0 * wy)).sinh().atan().to_degrees();
                            let rank = match kind { "locality" => 0, "macrohood" => 1, _ => 2 };
                            if geo::in_rio(lat, lon) {
                                places.push((lat, lon, name.clone(), rank));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Road names repeat on every piece of an avenue; one per name per tile.
        t.l.sort_by(|a, b| a.3.cmp(&b.3));
        t.l.dedup_by(|a, b| a.3 == b.3);

        if t.is_empty() {
            continue;
        }
        let dir = out.join(format!("{z}/{x}"));
        std::fs::create_dir_all(&dir)?;
        let body = serde_json::to_vec(&t)?;
        let (n, g) = write_gz(&dir.join(format!("{y}.json")), &body)?;
        stats.tiles[zi] += 1;
        stats.bytes += n;
        stats.gz_bytes += g;
    }

    // Place names: the most important spelling of each name wins, and repeats
    // within a kilometre collapse into one.
    places.sort_by(|a, b| a.3.cmp(&b.3).then(a.2.cmp(&b.2)));
    let mut keep: Vec<(f64, f64, String, u8)> = Vec::new();
    for p in places {
        let dup = keep.iter().any(|k| {
            k.2 == p.2 && {
                let (ax, ay) = geo::project(k.0, k.1);
                let (bx, by) = geo::project(p.0, p.1);
                geo::dist(ax, ay, bx, by) < 1_000.0
            }
        });
        if !dup {
            keep.push(p);
        }
    }
    stats.places = keep.len();
    let json: Vec<serde_json::Value> = keep
        .iter()
        .map(|(lat, lon, n, r)| serde_json::json!([(lon * 1e5).round() / 1e5, (lat * 1e5).round() / 1e5, n, r]))
        .collect();
    std::fs::create_dir_all(out)?;
    let (n, g) = write_gz(&out.join("places.json"), &serde_json::to_vec(&json)?)?;
    stats.bytes += n;
    stats.gz_bytes += g;
    Ok(stats)
}

fn write_gz(path: &Path, body: &[u8]) -> Result<(usize, usize)> {
    std::fs::write(path, body)?;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(9));
    enc.write_all(body)?;
    let gz = enc.finish()?;
    std::fs::write(path.with_extension("json.gz"), &gz)?;
    Ok((body.len(), gz.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hilbert_ids_map_to_tiles() {
        assert_eq!(zxy(0), (0, 0, 0));
        // Zoom 1 walks the quadrants in Hilbert order.
        assert_eq!(zxy(1), (1, 0, 0));
        assert_eq!(zxy(2), (1, 0, 1));
        assert_eq!(zxy(3), (1, 1, 1));
        assert_eq!(zxy(4), (1, 1, 0));
        // First id of zoom 2 follows the 1 + 4 tiles of zooms 0 and 1.
        assert_eq!(zxy(5).0, 2);
    }

    #[test]
    fn geometry_commands_decode_to_rings() {
        // MoveTo(2,2) LineTo(2,0)(0,2) ClosePath: a triangle.
        let cmds = [9, 4, 4, 18, 4, 0, 0, 4, 15];
        let g = geometry(&cmds);
        assert_eq!(g, vec![vec![(2, 2), (4, 2), (4, 4)]]);
    }

    #[test]
    fn simplification_keeps_corners_and_drops_noise() {
        let p = vec![(0.0, 0.0), (5.0, 0.1), (10.0, 0.0), (10.0, 10.0)];
        let s = simplify(&p, 1.0);
        assert_eq!(s, vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]);
    }
}
