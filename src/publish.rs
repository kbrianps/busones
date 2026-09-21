//! Snapshot publishing.
//!
//! Files land in a tmpfs directory that Caddy serves. Two rules matter for the
//! edge cache: every known line is written on every cycle, so a missing file
//! always means "the server has not published yet" and never "this line has no
//! buses", and a file is only rewritten when its content changed, which keeps
//! ETags stable and lets the browser and Cloudflare answer with 304.
//!
//! Vehicle records carry the fix timestamp `t` rather than an age in seconds,
//! so a snapshot that did not change is byte-identical to the previous one.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::geo;
use crate::gtfs::Gtfs;

pub struct Out {
    pub id: String,
    pub line: String,
    pub t: i64,
    pub lat: f64,
    pub lon: f64,
    pub bearing: Option<f32>,
    pub phase: &'static str,
    pub dir: Option<&'static str>,
    pub shape: Option<String>,
    pub along: Option<f32>,
    pub speed_kmh: f32,
    pub next_stop: Option<String>,
    pub conf: f32,
    pub vendor: &'static str,
    pub brt: bool,
}

/// One bus, on its way to one stop.
pub struct Arrival {
    pub stop: u32,
    pub line: String,
    pub to: String,
    pub dir: &'static str,
    pub vehicle: String,
    pub t: i64,
    pub stops_away: u16,
    pub metres: f32,
    /// Seconds from the fix time `t`, low and high end of the range.
    pub eta_lo: f32,
    pub eta_hi: f32,
}

#[derive(Default)]
pub struct Stats {
    pub files_written: usize,
    pub files_total: usize,
    pub bytes: usize,
}

pub struct Publisher {
    root: PathBuf,
    zoom: u8,
    cells: Vec<(u32, u32)>,
    lines: Vec<String>,
    hashes: HashMap<PathBuf, u64>,
}

impl Publisher {
    pub fn new(root: PathBuf, zoom: u8, gtfs: &Gtfs) -> Result<Self> {
        let mut lines: Vec<String> = gtfs
            .routes
            .iter()
            .map(|r| crate::gtfs::sanitize(&r.short_name))
            .collect();
        lines.sort();
        lines.dedup();
        let cells = geo::rio_tiles(zoom);
        for d in [
            root.join("api/v1/lines"),
            root.join(format!("api/v1/cells/{zoom}")),
        ] {
            std::fs::create_dir_all(&d)
                .with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(Publisher {
            root,
            zoom,
            cells,
            lines,
            hashes: HashMap::new(),
        })
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn publish(
        &mut self,
        outs: &[Out],
        arrivals: &[Arrival],
        gtfs: &Gtfs,
        index: serde_json::Value,
    ) -> Result<Stats> {
        let mut by_line: HashMap<&str, Vec<&Out>> = HashMap::new();
        let mut by_cell: HashMap<(u32, u32), Vec<&Out>> = HashMap::new();
        for o in outs {
            by_line.entry(o.line.as_str()).or_default().push(o);
            by_cell
                .entry(geo::tile_of(o.lat, o.lon, self.zoom))
                .or_default()
                .push(o);
        }

        let mut st = Stats::default();
        let empty: Vec<&Out> = Vec::new();

        for line in self.lines.clone() {
            let v = by_line.get(line.as_str()).unwrap_or(&empty);
            let body = render(v, false);
            let path = self.root.join(format!("api/v1/lines/{line}.json"));
            self.write(&path, &body, &mut st)?;
        }
        // Services the live feed reports that the published GTFS does not know.
        for (line, v) in by_line.iter() {
            let name = crate::gtfs::sanitize(line);
            if self.lines.binary_search(&name).is_err() {
                let body = render(v, false);
                let path = self.root.join(format!("api/v1/lines/{name}.json"));
                self.write(&path, &body, &mut st)?;
            }
        }

        // Arrivals, grouped by the cell the stop sits in and capped per line so
        // one busy corridor cannot dominate a stop's card.
        let mut by_stop: HashMap<u32, Vec<&Arrival>> = HashMap::new();
        for a in arrivals {
            by_stop.entry(a.stop).or_default().push(a);
        }
        let mut arr_cell: HashMap<(u32, u32), Vec<&Arrival>> = HashMap::new();
        for (stop, mut list) in by_stop {
            let st = &gtfs.stops[stop as usize];
            list.sort_by(|a, b| a.metres.partial_cmp(&b.metres).unwrap_or(std::cmp::Ordering::Equal));
            let mut per_line: HashMap<(&str, &str), usize> = HashMap::new();
            let kept: Vec<&Arrival> = list
                .into_iter()
                .filter(|a| {
                    let c = per_line.entry((a.line.as_str(), a.dir)).or_insert(0);
                    *c += 1;
                    *c <= 3
                })
                .collect();
            arr_cell
                .entry(geo::tile_of(st.lat, st.lon, self.zoom))
                .or_default()
                .extend(kept);
        }

        let no_arrivals: Vec<&Arrival> = Vec::new();
        for &(x, y) in &self.cells.clone() {
            let dir = self
                .root
                .join(format!("api/v1/cells/{}/{x}/{y}", self.zoom));
            if !dir.exists() {
                std::fs::create_dir_all(&dir)?;
            }
            let v = by_cell.get(&(x, y)).unwrap_or(&empty);
            let body = render(v, true);
            self.write(&dir.join("vehicles.json"), &body, &mut st)?;

            let a = arr_cell.get(&(x, y)).unwrap_or(&no_arrivals);
            let body = render_arrivals(a, gtfs);
            self.write(&dir.join("arrivals.json"), &body, &mut st)?;
        }

        // One file with the whole fleet, for the city-wide overview. The normal
        // client never fetches this: a rider watching one line must not pay for
        // every bus in Rio. An operations view covering the entire municipality
        // would otherwise need dozens of tile files per refresh.
        let all: Vec<&Out> = outs.iter().collect();
        let body = render(&all, true);
        let path = self.root.join("api/v1/fleet.json");
        self.write(&path, &body, &mut st)?;

        let body = serde_json::to_vec(&index)?;
        let path = self.root.join("api/v1/index.json");
        self.write(&path, &body, &mut st)?;
        Ok(st)
    }

    fn write(&mut self, path: &Path, body: &[u8], st: &mut Stats) -> Result<()> {
        st.files_total += 1;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut h);
        let digest = h.finish();
        if self.hashes.get(path) == Some(&digest) && path.exists() {
            return Ok(());
        }
        write_atomic(path, body)?;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        gz.write_all(body)?;
        write_atomic(&path.with_extension("json.gz"), &gz.finish()?)?;
        self.hashes.insert(path.to_path_buf(), digest);
        st.files_written += 1;
        st.bytes += body.len();
        Ok(())
    }
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(body)?;
        f.flush()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `[stop_id, line, headsign, dir, stops_away, metres, fix_time, vehicle_id,
///   eta_lo_s, eta_hi_s]`, the travel-time range counted from `fix_time`.
fn render_arrivals(list: &[&Arrival], gtfs: &Gtfs) -> Vec<u8> {
    let mut s = String::with_capacity(list.len() * 60 + 2);
    s.push('[');
    for (i, a) in list.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('[');
        push_json_string(&mut s, &gtfs.stops[a.stop as usize].id);
        s.push(',');
        push_json_string(&mut s, &a.line);
        s.push(',');
        push_json_string(&mut s, &a.to);
        s.push_str(",\"");
        s.push_str(a.dir);
        s.push_str("\",");
        s.push_str(&a.stops_away.to_string());
        s.push(',');
        s.push_str(&(a.metres.round() as i64).to_string());
        s.push(',');
        s.push_str(&a.t.to_string());
        s.push(',');
        push_json_string(&mut s, &a.vehicle);
        s.push(',');
        s.push_str(&(a.eta_lo.round() as i64).to_string());
        s.push(',');
        s.push_str(&(a.eta_hi.round() as i64).to_string());
        s.push(']');
    }
    s.push(']');
    s.into_bytes()
}

fn render(v: &[&Out], with_line: bool) -> Vec<u8> {
    let mut s = String::with_capacity(v.len() * 140 + 16);
    s.push('[');
    for (i, o) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("{\"id\":");
        push_json_string(&mut s, &o.id);
        if with_line {
            s.push_str(",\"line\":");
            push_json_string(&mut s, &o.line);
        }
        s.push_str(",\"t\":");
        s.push_str(&o.t.to_string());
        s.push_str(",\"lat\":");
        s.push_str(&format!("{:.5}", o.lat));
        s.push_str(",\"lon\":");
        s.push_str(&format!("{:.5}", o.lon));
        s.push_str(",\"ph\":\"");
        s.push_str(o.phase);
        s.push('"');
        match o.bearing {
            Some(b) => {
                s.push_str(",\"brg\":");
                s.push_str(&(b.round() as i32).to_string());
            }
            None => s.push_str(",\"brg\":null"),
        }
        if let Some(d) = o.dir {
            s.push_str(",\"dir\":\"");
            s.push_str(d);
            s.push('"');
        }
        if let Some(sh) = &o.shape {
            s.push_str(",\"shp\":");
            push_json_string(&mut s, sh);
        }
        if let Some(a) = o.along {
            s.push_str(",\"alo\":");
            s.push_str(&(a.round() as i64).to_string());
        }
        if let Some(ns) = &o.next_stop {
            s.push_str(",\"ns\":");
            push_json_string(&mut s, ns);
        }
        s.push_str(",\"spd\":");
        s.push_str(&(o.speed_kmh.round() as i32).to_string());
        s.push_str(",\"cnf\":");
        s.push_str(&format!("{:.2}", o.conf));
        s.push_str(",\"ven\":\"");
        s.push_str(o.vendor);
        s.push('"');
        if o.brt {
            s.push_str(",\"brt\":true");
        }
        s.push('}');
    }
    s.push(']');
    s.into_bytes()
}

fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(id: &str) -> Out {
        Out {
            id: id.into(),
            line: "474".into(),
            t: 1_789_902_038,
            lat: -22.90831,
            lon: -43.19645,
            bearing: Some(137.4),
            phase: "live",
            dir: Some("ida"),
            shape: Some("9ptt".into()),
            along: Some(1234.6),
            speed_kmh: 18.4,
            next_stop: Some("fcs4".into()),
            conf: 0.9,
            vendor: "zirix",
            brt: false,
        }
    }

    #[test]
    fn renders_valid_compact_json() {
        let a = out("B58021");
        let body = render(&[&a], true);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let o = &v[0];
        assert_eq!(o["id"], "B58021");
        assert_eq!(o["line"], "474");
        assert_eq!(o["t"], 1_789_902_038i64);
        assert_eq!(o["brg"], 137);
        assert_eq!(o["alo"], 1235);
        assert_eq!(o["ph"], "live");
        assert_eq!(o["dir"], "ida");
        assert!(o.get("brt").is_none());
        // Roughly 140 bytes per vehicle keeps a busy line well inside the budget.
        assert!(body.len() < 200, "{} bytes", body.len());
    }

    #[test]
    fn escapes_ids_and_renders_empty_arrays() {
        let mut a = out("we\"ird\\");
        a.dir = None;
        a.shape = None;
        a.along = None;
        a.next_stop = None;
        a.bearing = None;
        let body = render(&[&a], false);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v[0]["id"], "we\"ird\\");
        assert!(v[0]["brg"].is_null());
        assert!(v[0].get("dir").is_none());
        assert_eq!(render(&[], false), b"[]");
    }

    #[test]
    fn unchanged_content_is_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("busones-pub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut g = Gtfs::default();
        g.routes.push(crate::gtfs::Route {
            id: "R".into(),
            short_name: "474".into(),
            long_name: String::new(),
            route_type: 700,
            shapes: vec![],
        });
        g.finish();
        let mut p = Publisher::new(dir.clone(), 13, &g).unwrap();
        let a = out("B1");
        let idx = serde_json::json!({"generated_at": 1});
        let g2 = Gtfs::default();
        let first = p.publish(&[a], &[], &g2, idx.clone()).unwrap();
        assert_eq!(first.files_written, first.files_total);
        let a = out("B1");
        let second = p.publish(&[a], &[], &g2, idx).unwrap();
        assert_eq!(second.files_written, 0, "nothing changed, nothing rewritten");
        assert!(dir.join("api/v1/lines/474.json").exists());
        assert!(dir.join("api/v1/lines/474.json.gz").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
impl Out {
    /// Test helper: the published phase string.
    pub fn ph_line(&self) -> &'static str {
        self.phase
    }
}
