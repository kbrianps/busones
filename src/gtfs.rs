//! Static GTFS: preparation from the published feed and the in-memory model
//! the matcher works against.
//!
//! `busones gtfs build` turns the extracted feed into one compact artifact so
//! the server starts in milliseconds instead of parsing 85 MB of stop_times on
//! every restart. Distances along a shape come from the feed's own
//! `shape_dist_traveled` (metres, verified monotonic on all 961 shapes), which
//! is what `stop_times` is keyed on, so stops never need to be re-projected.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufReader, Write};
use std::path::Path;

use crate::csv;
use crate::geo;

#[derive(Serialize, Deserialize, Default)]
pub struct Route {
    pub id: String,
    pub short_name: String,
    pub long_name: String,
    pub route_type: u16,
    pub shapes: Vec<u32>,
    /// Other codes this line answers to, from the service table.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aka: Vec<String>,
    /// Official colour, `#RRGGBB` or empty. In Rio it is the stripe of the
    /// line's operating region on the new yellow buses (SMTR resolution 3870).
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub text_color: String,
}

/// Bumped whenever `gtfs build` starts extracting something new, so a server
/// never runs on an artifact that silently lacks it.
pub const FORMAT: u32 = 2;

/// `FF7600` or `ff7600` to `#FF7600`; anything else to empty.
fn hex_color(s: &str) -> String {
    let s = s.trim().trim_start_matches('#');
    if s.len() == 6 && s.bytes().all(|c| c.is_ascii_hexdigit()) {
        format!("#{}", s.to_ascii_uppercase())
    } else {
        String::new()
    }
}

/// Live service codes the published GTFS spells differently, and codes that
/// are not passenger lines at all. Lives in `config/service-aliases.json`,
/// with the evidence for every entry in `config/README.md`.
#[derive(Deserialize, Default)]
pub struct ServiceTable {
    /// Live code -> GTFS `route_short_name`.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// Live codes that name no line: vendor placeholders that follow no route.
    #[serde(default)]
    pub not_service: Vec<String>,
}

impl ServiceTable {
    /// A missing file is an empty table; a malformed one is an error, so a typo
    /// cannot silently hide every alias.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing service table {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
}

/// The SMTR publishes new lines under a provisional `LECD` code before they
/// get their public number.
fn is_provisional(code: &str) -> bool {
    code.starts_with("LECD")
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct ShapeStop {
    pub stop: u32,
    pub dist: f32,
}

#[derive(Serialize, Deserialize)]
pub struct Shape {
    pub id: String,
    pub route: u32,
    pub direction: u8,
    pub headsign: String,
    /// Micro-degrees, the on-disk form.
    pub lat: Vec<i32>,
    pub lon: Vec<i32>,
    /// Metres along the shape, from the feed.
    pub cum: Vec<f32>,
    pub circular: bool,
    pub stops: Vec<ShapeStop>,
    /// Metres per second the timetable assumes for this shape. The published
    /// stop times are interpolated at constant speed (2,997 of 3,000 trips
    /// sampled), so this is the only thing they actually encode: a per-route
    /// average, useful as a prior when too few buses are running to measure.
    #[serde(default)]
    pub planned_speed: f32,
    /// Scheduled headway ranges: `[service, from_hour, to_hour, minutes]`,
    /// service 0 = weekdays, 1 = Saturday, 2 = Sunday. Rio publishes most
    /// lines as frequencies rather than timetables, so "a bus every ~7 min"
    /// is the honest form of a schedule here.
    #[serde(default)]
    pub freq: Vec<[u16; 4]>,
    #[serde(skip)]
    pub x: Vec<f32>,
    #[serde(skip)]
    pub y: Vec<f32>,
    /// minx, miny, maxx, maxy in metres.
    #[serde(skip)]
    pub bbox: [f32; 4],
}

impl Shape {
    #[inline]
    pub fn length(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    #[inline]
    pub fn point_at(&self, along: f32) -> (f32, f32) {
        if self.x.is_empty() {
            return (0.0, 0.0);
        }
        let a = along.clamp(0.0, self.length());
        let i = match self.cum.binary_search_by(|c| c.partial_cmp(&a).unwrap()) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
        .min(self.x.len() - 1);
        let j = (i + 1).min(self.x.len() - 1);
        let span = self.cum[j] - self.cum[i];
        let t = if span > 0.1 { (a - self.cum[i]) / span } else { 0.0 };
        (
            self.x[i] + t * (self.x[j] - self.x[i]),
            self.y[i] + t * (self.y[j] - self.y[i]),
        )
    }

    /// Bearing of the shape at a given distance along it.
    pub fn bearing_at(&self, along: f32) -> f32 {
        let (ax, ay) = self.point_at((along - 15.0).max(0.0));
        let (bx, by) = self.point_at((along + 15.0).min(self.length()));
        geo::bearing_deg(ax, ay, bx, by)
    }

    /// Index of the first stop the vehicle has not clearly passed yet.
    ///
    /// The 5 m tolerance keeps a bus that is standing at a stop pointing at
    /// that stop instead of flickering to the next one on GPS noise.
    pub fn next_stop_idx(&self, along: f32) -> Option<usize> {
        self.stops.iter().position(|s| s.dist > along - 5.0)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Stop {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    #[serde(skip)]
    pub x: f32,
    #[serde(skip)]
    pub y: f32,
}

/// Which service runs on a date: 0 weekday, 1 Saturday, 2 Sunday, or none.
///
/// Holidays come from `calendar_dates.txt`: the SMTR adds the Sunday service
/// and removes the weekday one, so a holiday runs the Sunday frequencies.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DayTypes {
    /// For each weekday, Sunday first as in JavaScript's `getDay()`.
    pub weekdays: [Option<u8>; 7],
    /// Dates (`YYYYMMDD`) whose day type differs from their weekday's.
    pub exceptions: BTreeMap<String, Option<u8>>,
}

impl Default for DayTypes {
    fn default() -> Self {
        DayTypes {
            weekdays: [Some(2), Some(0), Some(0), Some(0), Some(0), Some(0), Some(1)],
            exceptions: BTreeMap::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Gtfs {
    /// See [`FORMAT`].
    #[serde(default)]
    pub format: u32,
    pub feed_version: String,
    #[serde(default)]
    pub days: DayTypes,
    pub routes: Vec<Route>,
    pub shapes: Vec<Shape>,
    pub stops: Vec<Stop>,
    /// Every trip of the feed, so the BRT GTFS-Realtime feed can be joined by trip_id.
    pub trip_shape: Vec<(String, u32)>,
    #[serde(skip)]
    pub by_short: HashMap<String, u32>,
    #[serde(skip)]
    pub by_shape_id: HashMap<String, u32>,
    #[serde(skip)]
    pub by_trip: HashMap<String, u32>,
    /// Live codes that are not passenger lines, from the service table.
    #[serde(skip)]
    pub not_service: HashSet<String>,
}

impl Gtfs {
    /// Rebuilds everything that is derived from the serialised fields.
    pub fn finish(&mut self) {
        for sh in &mut self.shapes {
            let n = sh.lat.len();
            sh.x = Vec::with_capacity(n);
            sh.y = Vec::with_capacity(n);
            let mut bbox = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for i in 0..n {
                let (x, y) = geo::project(sh.lat[i] as f64 / 1e6, sh.lon[i] as f64 / 1e6);
                bbox[0] = bbox[0].min(x);
                bbox[1] = bbox[1].min(y);
                bbox[2] = bbox[2].max(x);
                bbox[3] = bbox[3].max(y);
                sh.x.push(x);
                sh.y.push(y);
            }
            sh.bbox = bbox;
        }
        for st in &mut self.stops {
            let (x, y) = geo::project(st.lat, st.lon);
            st.x = x;
            st.y = y;
        }
        self.index_names();
        self.by_shape_id = self
            .shapes
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i as u32))
            .collect();
        self.by_trip = self.trip_shape.iter().cloned().collect();
    }

    /// Every name a route answers to, the public one first so it wins a clash.
    fn index_names(&mut self) {
        let mut by_short = HashMap::new();
        for (i, r) in self.routes.iter().enumerate() {
            by_short.insert(r.short_name.clone(), i as u32);
        }
        for (i, r) in self.routes.iter().enumerate() {
            for a in &r.aka {
                by_short.entry(a.clone()).or_insert(i as u32);
            }
        }
        self.by_short = by_short;
    }

    /// Applies the service table and returns what could not be applied.
    ///
    /// The GTFS names some lines by their provisional `LECD` code while buses
    /// and riders use the public number (buses on LECD140 report 685), and it
    /// goes the other way once a line gets its number (buses still report
    /// LECD156 for what the GTFS now calls 856). Either way the line is shown
    /// under its public name and the other code keeps resolving to it.
    pub fn apply_services(&mut self, t: &ServiceTable) -> Vec<String> {
        let mut problems = Vec::new();
        for (live, target) in &t.aliases {
            let Some(&i) = self.by_short.get(target.as_str()) else {
                problems.push(format!("{live} -> {target}: no such route"));
                continue;
            };
            if self.by_short.get(live.as_str()).is_some_and(|&j| j != i) {
                problems.push(format!("{live} -> {target}: {live} is already another route"));
                continue;
            }
            let r = &mut self.routes[i as usize];
            if is_provisional(&r.short_name) && !is_provisional(live) {
                let old = std::mem::replace(&mut r.short_name, live.clone());
                r.aka.push(old);
            } else if !r.aka.contains(live) && r.short_name != *live {
                r.aka.push(live.clone());
            }
            self.index_names();
        }
        self.not_service = t.not_service.iter().map(|s| s.trim().to_string()).collect();
        problems
    }

    /// Resolves a live `servico` code to a route.
    ///
    /// The feed and the published zip disagree on zero padding (the feed says
    /// `7`, the zip says `007`), so both forms are tried. Names from the service
    /// table are already in the index.
    pub fn route_of_service(&self, service: &str) -> Option<u32> {
        let s = service.trim();
        if s.is_empty() {
            return None;
        }
        if let Some(&i) = self.by_short.get(s) {
            return Some(i);
        }
        if s.bytes().all(|c| c.is_ascii_digit()) {
            let stripped = s.trim_start_matches('0');
            if !stripped.is_empty() {
                if let Some(&i) = self.by_short.get(stripped) {
                    return Some(i);
                }
                if stripped.len() < 3 {
                    let padded = format!("{stripped:0>3}");
                    if let Some(&i) = self.by_short.get(&padded) {
                        return Some(i);
                    }
                }
            }
        }
        None
    }

    pub fn shape(&self, idx: u32) -> &Shape {
        &self.shapes[idx as usize]
    }

    pub fn route(&self, idx: u32) -> &Route {
        &self.routes[idx as usize]
    }

    pub fn load(path: &Path) -> Result<Self> {
        let f = std::fs::File::open(path)
            .with_context(|| format!("opening prepared GTFS at {}", path.display()))?;
        let dec = flate2::read::GzDecoder::new(BufReader::with_capacity(1 << 20, f));
        let mut g: Gtfs = serde_json::from_reader(BufReader::with_capacity(1 << 20, dec))
            .context("parsing prepared GTFS")?;
        if g.format < FORMAT {
            bail!(
                "{} was prepared by an older busones (format {}, need {}); rebuild it with \
                 `FORCE=1 scripts/fetch-gtfs.sh`",
                path.display(),
                g.format,
                FORMAT
            );
        }
        g.finish();
        Ok(g)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = std::fs::File::create(path)?;
        let mut enc = flate2::write::GzEncoder::new(
            std::io::BufWriter::with_capacity(1 << 20, f),
            flate2::Compression::new(6),
        );
        serde_json::to_writer(&mut enc, self)?;
        enc.finish()?.flush()?;
        Ok(())
    }

    /// Test helper: a deep copy, since `Gtfs` is normally shared read-only.
    #[cfg(test)]
    pub fn clone_for_test(&self) -> Gtfs {
        let bytes = serde_json::to_vec(self).unwrap();
        let mut g: Gtfs = serde_json::from_slice(&bytes).unwrap();
        g.finish();
        g
    }

    pub fn summary(&self) -> String {
        let stops_linked: usize = self.shapes.iter().map(|s| s.stops.len()).sum();
        format!(
            "{} routes, {} shapes ({} points), {} stops, {} trips, {} shape-stop links, {} calendar exceptions",
            self.routes.len(),
            self.shapes.len(),
            self.shapes.iter().map(|s| s.lat.len()).sum::<usize>(),
            self.stops.len(),
            self.trip_shape.len(),
            stops_linked,
            self.days.exceptions.len()
        )
    }
}

fn service_code(service_id: &str) -> Option<u16> {
    match service_id {
        "U_REG" => Some(0),
        "S_REG" => Some(1),
        "D_REG" => Some(2),
        _ => None,
    }
}

/// Reads `calendar.txt` and `calendar_dates.txt` into day types. Both files are
/// optional: without them every weekday keeps its usual service.
fn read_calendar(dir: &Path) -> Result<DayTypes> {
    use crate::timeutil::days_from_civil;
    let day = |s: &str| -> Option<i64> {
        if s.len() != 8 {
            return None;
        }
        let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
        Some(days_from_civil(n(0, 4)?, n(4, 6)?, n(6, 8)?))
    };
    // JavaScript weekday (Sunday 0) of a day count; 1970-01-01 was a Thursday.
    let weekday = |d: i64| (d + 4).rem_euclid(7) as usize;

    let mut out = DayTypes::default();
    // (service code, runs on [Sunday..Saturday], first day, last day)
    let mut base: Vec<(u8, [bool; 7], i64, i64)> = Vec::new();
    if let Ok(r) = open(dir, "calendar.txt") {
        let mut rd = csv::Reader::new(r)?;
        let names = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];
        let cols: Vec<Option<usize>> = names.iter().map(|n| rd.column(n)).collect();
        let (cid, cs, ce) = (rd.column("service_id"), rd.column("start_date"), rd.column("end_date"));
        while let Some(rec) = rd.next_record()? {
            let Some(code) = service_code(csv::get(rec, cid)) else {
                continue;
            };
            let mut runs = [false; 7];
            for (i, c) in cols.iter().enumerate() {
                runs[i] = csv::get(rec, *c) == "1";
            }
            let from = day(csv::get(rec, cs)).unwrap_or(i64::MIN);
            let to = day(csv::get(rec, ce)).unwrap_or(i64::MAX);
            base.push((code as u8, runs, from, to));
        }
        if !base.is_empty() {
            for (wd, slot) in out.weekdays.iter_mut().enumerate() {
                *slot = base.iter().filter(|b| b.1[wd]).map(|b| b.0).min();
            }
        }
    }

    // date -> (service code, exception_type)
    let mut by_date: BTreeMap<String, Vec<(u8, u8)>> = BTreeMap::new();
    if let Ok(r) = open(dir, "calendar_dates.txt") {
        let mut rd = csv::Reader::new(r)?;
        let (cid, cd, ct) = (rd.column("service_id"), rd.column("date"), rd.column("exception_type"));
        while let Some(rec) = rd.next_record()? {
            let Some(code) = service_code(csv::get(rec, cid)) else {
                continue;
            };
            let Ok(kind) = csv::get(rec, ct).parse::<u8>() else {
                continue;
            };
            by_date
                .entry(csv::get(rec, cd).to_string())
                .or_default()
                .push((code as u8, kind));
        }
    }
    for (date, changes) in by_date {
        let Some(d) = day(&date) else {
            continue;
        };
        let wd = weekday(d);
        let mut active: Vec<u8> = base
            .iter()
            .filter(|b| b.1[wd] && b.2 <= d && d <= b.3)
            .map(|b| b.0)
            .collect();
        for &(code, kind) in &changes {
            match kind {
                1 if !active.contains(&code) => active.push(code),
                2 => active.retain(|&c| c != code),
                _ => {}
            }
        }
        // Several services on one day is not something the SMTR publishes; if it
        // ever happens, the one the exception added is what the day is about.
        let added = changes.iter().find(|c| c.1 == 1 && active.contains(&c.0)).map(|c| c.0);
        let kind = added.or_else(|| active.iter().copied().min());
        if kind != out.weekdays[wd] {
            out.exceptions.insert(date, kind);
        }
    }
    Ok(out)
}

/// Hourly rates to `[service, from_hour, to_hour, minutes]` runs, merging
/// consecutive hours whose rounded headway is the same.
fn encode_headways(r: &[[f32; 24]; 3]) -> Vec<[u16; 4]> {
    let mut out = Vec::new();
    for (svc, hours) in r.iter().enumerate() {
        let mins: Vec<u16> = hours
            .iter()
            .map(|&per_hour| if per_hour > 0.05 { (60.0 / per_hour).round().max(1.0) as u16 } else { 0 })
            .collect();
        let mut h = 0;
        while h < 24 {
            if mins[h] == 0 {
                h += 1;
                continue;
            }
            let start = h;
            while h + 1 < 24 && mins[h + 1] == mins[start] {
                h += 1;
            }
            out.push([svc as u16, start as u16, (h + 1) as u16, mins[start]]);
            h += 1;
        }
    }
    out
}

/// `HH:MM:SS` to seconds. GTFS hours run past 24 for trips after midnight.
fn hms(t: &str) -> Option<u32> {
    let mut it = t.trim().split(':');
    let h: u32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let s: u32 = it.next()?.parse().ok()?;
    Some(h * 3600 + m * 60 + s)
}

fn open(dir: &Path, name: &str) -> Result<BufReader<std::fs::File>> {
    let p = dir.join(name);
    let f = std::fs::File::open(&p).with_context(|| format!("opening {}", p.display()))?;
    Ok(BufReader::with_capacity(1 << 20, f))
}

/// Builds the prepared artifact from a directory of extracted GTFS text files.
pub fn build(dir: &Path) -> Result<Gtfs> {
    let mut g = Gtfs {
        format: FORMAT,
        ..Gtfs::default()
    };

    // feed_info is optional and often has no version; keep whatever is there.
    if let Ok(r) = open(dir, "feed_info.txt") {
        if let Ok(mut rd) = csv::Reader::new(r) {
            let cv = rd.column("feed_version");
            let cs = rd.column("feed_start_date");
            if let Ok(Some(rec)) = rd.next_record() {
                let v = csv::get(rec, cv);
                let s = csv::get(rec, cs);
                g.feed_version = if v.is_empty() { s.to_string() } else { v.to_string() };
            }
        }
    }

    g.days = read_calendar(dir)?;

    // routes
    let mut route_idx: HashMap<String, u32> = HashMap::new();
    {
        let mut rd = csv::Reader::new(open(dir, "routes.txt")?)?;
        let (ci, cs, cl, ct, cc, ctc) = (
            rd.column("route_id"),
            rd.column("route_short_name"),
            rd.column("route_long_name"),
            rd.column("route_type"),
            rd.column("route_color"),
            rd.column("route_text_color"),
        );
        while let Some(rec) = rd.next_record()? {
            let id = csv::get(rec, ci).to_string();
            if id.is_empty() {
                continue;
            }
            route_idx.insert(id.clone(), g.routes.len() as u32);
            g.routes.push(Route {
                id,
                short_name: csv::get(rec, cs).to_string(),
                long_name: csv::get(rec, cl).to_string(),
                route_type: csv::get(rec, ct).parse().unwrap_or(0),
                shapes: Vec::new(),
                aka: Vec::new(),
                color: hex_color(csv::get(rec, cc)),
                text_color: hex_color(csv::get(rec, ctc)),
            });
        }
    }
    if g.routes.is_empty() {
        bail!("routes.txt produced no routes");
    }

    // trips: shape metadata, trip -> shape, and the candidate trips per shape
    struct ShapeMeta {
        route: u32,
        direction: u8,
        headsign: String,
        trips: Vec<String>,
    }
    let mut shape_meta: HashMap<String, ShapeMeta> = HashMap::new();
    let mut trip_to_shape: HashMap<String, String> = HashMap::new();
    let mut trip_service: HashMap<String, u16> = HashMap::new();
    {
        let mut rd = csv::Reader::new(open(dir, "trips.txt")?)?;
        let (ct, cr, cd, csh, chs, csv_) = (
            rd.column("trip_id"),
            rd.column("route_id"),
            rd.column("direction_id"),
            rd.column("shape_id"),
            rd.column("trip_headsign"),
            rd.column("service_id"),
        );
        while let Some(rec) = rd.next_record()? {
            let trip = csv::get(rec, ct);
            let shape = csv::get(rec, csh);
            if trip.is_empty() || shape.is_empty() {
                continue;
            }
            let Some(&route) = route_idx.get(csv::get(rec, cr)) else {
                continue;
            };
            trip_to_shape.insert(trip.to_string(), shape.to_string());
            if let Some(code) = service_code(csv::get(rec, csv_)) {
                trip_service.insert(trip.to_string(), code);
            }
            let e = shape_meta.entry(shape.to_string()).or_insert_with(|| ShapeMeta {
                route,
                direction: csv::get(rec, cd).parse().unwrap_or(0),
                headsign: csv::get(rec, chs).to_string(),
                trips: Vec::new(),
            });
            if e.headsign.is_empty() {
                e.headsign = csv::get(rec, chs).to_string();
            }
            e.trips.push(trip.to_string());
        }
    }

    // stop_times, pass 1: how many stops each trip actually has, so the
    // representative trip of a shape is its most complete one.
    let mut trip_rows: HashMap<String, u32> = trip_to_shape.keys().map(|t| (t.clone(), 0)).collect();
    {
        let mut rd = csv::Reader::new(open(dir, "stop_times.txt")?)?;
        let ct = rd.column("trip_id");
        while let Some(rec) = rd.next_record()? {
            if let Some(c) = trip_rows.get_mut(csv::get(rec, ct)) {
                *c += 1;
            }
        }
    }
    let mut rep_trip: HashMap<String, String> = HashMap::new();
    for (shape, meta) in &shape_meta {
        if let Some(best) = meta
            .trips
            .iter()
            .max_by_key(|t| trip_rows.get(*t).copied().unwrap_or(0))
        {
            rep_trip.insert(best.clone(), shape.clone());
        }
    }

    // stops
    let mut stop_idx: HashMap<String, u32> = HashMap::new();
    {
        let mut rd = csv::Reader::new(open(dir, "stops.txt")?)?;
        let (ci, cn, cla, clo) = (
            rd.column("stop_id"),
            rd.column("stop_name"),
            rd.column("stop_lat"),
            rd.column("stop_lon"),
        );
        while let Some(rec) = rd.next_record()? {
            let id = csv::get(rec, ci).to_string();
            let (Ok(lat), Ok(lon)) = (
                csv::get(rec, cla).parse::<f64>(),
                csv::get(rec, clo).parse::<f64>(),
            ) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            stop_idx.insert(id.clone(), g.stops.len() as u32);
            g.stops.push(Stop {
                id,
                name: csv::get(rec, cn).to_string(),
                lat,
                lon,
                x: 0.0,
                y: 0.0,
            });
        }
    }

    // shapes geometry
    let mut geom: HashMap<String, Vec<(u32, f64, f64, f32)>> = HashMap::new();
    {
        let mut rd = csv::Reader::new(open(dir, "shapes.txt")?)?;
        let (ci, cq, cla, clo, cd) = (
            rd.column("shape_id"),
            rd.column("shape_pt_sequence"),
            rd.column("shape_pt_lat"),
            rd.column("shape_pt_lon"),
            rd.column("shape_dist_traveled"),
        );
        while let Some(rec) = rd.next_record()? {
            let id = csv::get(rec, ci);
            if id.is_empty() || !shape_meta.contains_key(id) {
                continue;
            }
            let (Ok(lat), Ok(lon)) = (
                csv::get(rec, cla).parse::<f64>(),
                csv::get(rec, clo).parse::<f64>(),
            ) else {
                continue;
            };
            geom.entry(id.to_string()).or_default().push((
                csv::get(rec, cq).parse().unwrap_or(0),
                lat,
                lon,
                csv::get(rec, cd).parse().unwrap_or(f32::NAN),
            ));
        }
    }

    // stop_times, pass 2: the ordered stop list of each representative trip
    let mut shape_stops: HashMap<String, Vec<(u32, u32, f32)>> = HashMap::new();
    let mut shape_times: HashMap<String, (u32, f32, u32, f32)> = HashMap::new();
    {
        let mut rd = csv::Reader::new(open(dir, "stop_times.txt")?)?;
        let (ct, cq, cs, cd, ca) = (
            rd.column("trip_id"),
            rd.column("stop_sequence"),
            rd.column("stop_id"),
            rd.column("shape_dist_traveled"),
            rd.column("arrival_time"),
        );
        while let Some(rec) = rd.next_record()? {
            let Some(shape) = rep_trip.get(csv::get(rec, ct)) else {
                continue;
            };
            let Some(&stop) = stop_idx.get(csv::get(rec, cs)) else {
                continue;
            };
            let Ok(dist) = csv::get(rec, cd).parse::<f32>() else {
                continue;
            };
            if let Some(t) = hms(csv::get(rec, ca)) {
                let e = shape_times.entry(shape.clone()).or_insert((t, dist, t, dist));
                if t < e.0 {
                    e.0 = t;
                    e.1 = dist;
                }
                if t > e.2 {
                    e.2 = t;
                    e.3 = dist;
                }
            }
            shape_stops.entry(shape.clone()).or_default().push((
                csv::get(rec, cq).parse().unwrap_or(0),
                stop,
                dist,
            ));
        }
    }

    // headways: buses per hour for each shape, service and hour of the day
    let mut rates: HashMap<String, [[f32; 24]; 3]> = HashMap::new();
    if let Ok(r) = open(dir, "frequencies.txt") {
        let mut rd = csv::Reader::new(r)?;
        let (ct, cs, ce, ch) = (
            rd.column("trip_id"),
            rd.column("start_time"),
            rd.column("end_time"),
            rd.column("headway_secs"),
        );
        while let Some(rec) = rd.next_record()? {
            let trip = csv::get(rec, ct);
            let (Some(shape), Some(&svc)) = (trip_to_shape.get(trip), trip_service.get(trip)) else {
                continue;
            };
            let (Some(a), Some(b), Ok(h)) = (
                hms(csv::get(rec, cs)),
                hms(csv::get(rec, ce)),
                csv::get(rec, ch).parse::<f32>(),
            ) else {
                continue;
            };
            if h <= 0.0 || b <= a {
                continue;
            }
            let per_hour = 3600.0 / h;
            let slot = &mut rates.entry(shape.clone()).or_insert([[0.0; 24]; 3])[svc as usize];
            // Spread the window over the hours it touches, weighted by overlap.
            // Trips after midnight (GTFS hours 24+) fold back onto the clock.
            let mut t = a - a % 3600;
            while t < b {
                let lo = t.max(a);
                let hi = (t + 3600).min(b);
                slot[((t / 3600) % 24) as usize] += per_hour * (hi - lo) as f32 / 3600.0;
                t += 3600;
            }
        }
    }

    // assemble
    let mut warn_dist = 0usize;
    let mut ids: Vec<&String> = shape_meta.keys().collect();
    ids.sort();
    for id in ids {
        let meta = &shape_meta[id];
        let Some(pts) = geom.get_mut(id) else { continue };
        pts.sort_by_key(|p| p.0);
        pts.dedup_by_key(|p| p.0);
        if pts.len() < 2 {
            continue;
        }

        let mut lat = Vec::with_capacity(pts.len());
        let mut lon = Vec::with_capacity(pts.len());
        let mut cum = Vec::with_capacity(pts.len());
        let mut computed = 0.0f32;
        let mut prev: Option<(f32, f32)> = None;
        let mut feed_ok = true;
        for &(_, la, lo, d) in pts.iter() {
            let (x, y) = geo::project(la, lo);
            if let Some((px, py)) = prev {
                computed += geo::dist(px, py, x, y);
            }
            prev = Some((x, y));
            lat.push((la * 1e6).round() as i32);
            lon.push((lo * 1e6).round() as i32);
            if !d.is_finite() || (cum.last().is_some_and(|&c: &f32| d < c - 0.5)) {
                feed_ok = false;
            }
            cum.push(d);
        }
        // The published feed matches the computed polyline within 0.2% on every
        // shape today; a wider gap means the column cannot be trusted.
        if let Some(&last) = cum.last() {
            if !last.is_finite() || last <= 0.0 || (last - computed).abs() > 0.05 * computed.max(1.0)
            {
                feed_ok = false;
            }
        }
        if !feed_ok {
            // Fall back to the computed polyline length, keeping stop distances usable.
            warn_dist += 1;
            let mut acc = 0.0f32;
            let mut prev: Option<(f32, f32)> = None;
            for i in 0..lat.len() {
                let (x, y) = geo::project(lat[i] as f64 / 1e6, lon[i] as f64 / 1e6);
                if let Some((px, py)) = prev {
                    acc += geo::dist(px, py, x, y);
                }
                prev = Some((x, y));
                cum[i] = acc;
            }
        }

        let mut stops: Vec<ShapeStop> = shape_stops
            .remove(id)
            .map(|mut v| {
                v.sort_by_key(|s| s.0);
                v.into_iter().map(|(_, stop, dist)| ShapeStop { stop, dist }).collect()
            })
            .unwrap_or_default();
        stops.retain(|s| s.dist.is_finite());
        stops.dedup_by(|a, b| a.stop == b.stop && (a.dist - b.dist).abs() < 1.0);

        let first = geo::project(lat[0] as f64 / 1e6, lon[0] as f64 / 1e6);
        let last_i = lat.len() - 1;
        let last = geo::project(lat[last_i] as f64 / 1e6, lon[last_i] as f64 / 1e6);
        let circular = geo::dist(first.0, first.1, last.0, last.1) < 300.0;

        let planned_speed = shape_times
            .get(id)
            .and_then(|&(t0, d0, t1, d1)| {
                let dt = t1.saturating_sub(t0) as f32;
                (dt > 60.0 && d1 > d0).then(|| (d1 - d0) / dt)
            })
            .unwrap_or(6.0);

        let si = g.shapes.len() as u32;
        g.routes[meta.route as usize].shapes.push(si);
        g.shapes.push(Shape {
            id: id.clone(),
            route: meta.route,
            direction: meta.direction,
            headsign: meta.headsign.clone(),
            lat,
            lon,
            cum,
            circular,
            stops,
            planned_speed,
            freq: rates.get(id).map(encode_headways).unwrap_or_default(),
            x: Vec::new(),
            y: Vec::new(),
            bbox: [0.0; 4],
        });
        g.by_shape_id.insert(id.clone(), si);
    }
    if warn_dist > 0 {
        tracing::warn!(
            shapes = warn_dist,
            "shape_dist_traveled missing or non-monotonic; recomputed from geometry"
        );
    }

    for (trip, shape) in trip_to_shape {
        if let Some(&si) = g.by_shape_id.get(&shape) {
            g.trip_shape.push((trip, si));
        }
    }
    g.trip_shape.sort();

    g.finish();
    Ok(g)
}

/// Writes the static bundles the browser client loads: one file per line with
/// its shapes and stops, the stop directory, and the stop-to-lines index.
pub fn export(g: &Gtfs, out: &Path, cell_zoom: u8) -> Result<()> {
    use serde_json::json;
    std::fs::create_dir_all(out.join("lines"))?;

    let mut stop_lines: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut written: HashSet<String> = HashSet::new();
    for route in &g.routes {
        let mut dirs = Vec::new();
        for &si in &route.shapes {
            let sh = g.shape(si);
            let pts: Vec<(f64, f64)> = (0..sh.lat.len())
                .map(|i| (sh.lat[i] as f64 / 1e6, sh.lon[i] as f64 / 1e6))
                .collect();
            let stops: Vec<serde_json::Value> = sh
                .stops
                .iter()
                .map(|s| {
                    let st = &g.stops[s.stop as usize];
                    stop_lines
                        .entry(st.id.as_str())
                        .or_default()
                        .push(route.short_name.as_str());
                    let (nome, sub) = stop_label(&st.name);
                    json!([st.id, (st.lat * 1e5).round() / 1e5, (st.lon * 1e5).round() / 1e5, nome, s.dist.round(), sub])
                })
                .collect();
            dirs.push(json!({
                "shape": sh.id,
                "dir": sh.direction,
                "circular": sh.circular,
                "headsign": sh.headsign,
                "len": sh.length().round(),
                "planned": (sh.planned_speed * 10.0).round() / 10.0,
                "freq": sh.freq,
                "poly": crate::polyline::encode(&pts, 5),
                "stops": stops,
            }));
        }
        let body = json!({
            "line": route.short_name,
            "name": route.long_name,
            "type": route.route_type,
            "color": route.color,
            "text": route.text_color,
            "dirs": dirs,
        });
        let name = sanitize(&route.short_name);
        write_json(&out.join("lines").join(format!("{name}.json")), &body)?;
        written.insert(format!("{name}.json"));
    }
    // A line renamed by the service table must not leave its old bundle behind.
    for e in std::fs::read_dir(out.join("lines"))? {
        let e = e?;
        let f = e.file_name().to_string_lossy().into_owned();
        let base = f.strip_suffix(".gz").unwrap_or(&f);
        if base.ends_with(".json") && !written.contains(base) {
            std::fs::remove_file(e.path())?;
        }
    }

    let index: HashMap<&str, Vec<&str>> = stop_lines
        .into_iter()
        .map(|(k, mut v)| {
            v.sort_unstable();
            v.dedup();
            (k, v)
        })
        .collect();

    // Every line with its destinations, any other code it answers to and its
    // official colours, so a rider can add a line by number, by old code or by
    // where it goes, including lines that do not pass near them.
    let mut catalogo: Vec<serde_json::Value> = g
        .routes
        .iter()
        .map(|r| {
            let mut dests: Vec<&str> = r
                .shapes
                .iter()
                .map(|&si| g.shape(si).headsign.as_str())
                .filter(|h| !h.is_empty())
                .collect();
            dests.sort_unstable();
            dests.dedup();
            json!([r.short_name, r.long_name, r.route_type, dests, r.aka, r.color, r.text_color])
        })
        .collect();
    catalogo.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    write_json(&out.join("lines.json"), &json!(catalogo))?;

    // How often each line runs, hour by hour, for ranking the lines near you
    // by walk plus expected wait. The shortest headway across a line's shapes
    // stands for the line; per-direction detail stays in its own bundle.
    let mut freq = serde_json::Map::new();
    for r in &g.routes {
        let mut melhor = [[0u16; 24]; 3];
        for &si in &r.shapes {
            for f in &g.shape(si).freq {
                let (svc, h0, h1, min) = (f[0] as usize, f[1] as usize, f[2] as usize, f[3]);
                for h in h0..h1.min(24) {
                    let cur = &mut melhor[svc.min(2)][h];
                    if *cur == 0 || min < *cur {
                        *cur = min;
                    }
                }
            }
        }
        let mut runs = Vec::new();
        for (svc, horas) in melhor.iter().enumerate() {
            let mut h = 0;
            while h < 24 {
                if horas[h] == 0 {
                    h += 1;
                    continue;
                }
                let ini = h;
                while h + 1 < 24 && horas[h + 1] == horas[ini] {
                    h += 1;
                }
                runs.push(json!([svc, ini, h + 1, horas[ini]]));
                h += 1;
            }
        }
        if !runs.is_empty() {
            freq.insert(r.short_name.clone(), json!(runs));
        }
    }
    write_json(&out.join("freq.json"), &serde_json::Value::Object(freq))?;
    // Which of those services runs on a given date: holidays run Sunday's.
    write_json(
        &out.join("calendar.json"),
        &json!({"weekdays": g.days.weekdays, "exceptions": g.days.exceptions}),
    )?;

    let stops: Vec<serde_json::Value> = g
        .stops
        .iter()
        .map(|s| {
            let (name, sub) = stop_label(&s.name);
            json!([
                s.id,
                (s.lat * 1e5).round() / 1e5,
                (s.lon * 1e5).round() / 1e5,
                name,
                sub
            ])
        })
        .collect();
    write_json(&out.join("stops.json"), &json!(stops))?;
    write_json(&out.join("stop-lines.json"), &json!(&index))?;

    // The same stops split by map tile, so the first screen can load the few
    // hundred metres around someone instead of all 7,694 stops in the city.
    let zoom = cell_zoom;
    let mut by_cell: HashMap<(u32, u32), Vec<serde_json::Value>> = HashMap::new();
    for s in &g.stops {
        let (name, sub) = stop_label(&s.name);
        let lines = index.get(s.id.as_str()).cloned().unwrap_or_default();
        if lines.is_empty() {
            continue;
        }
        by_cell
            .entry(geo::tile_of(s.lat, s.lon, zoom))
            .or_default()
            .push(json!([
                s.id,
                (s.lat * 1e5).round() / 1e5,
                (s.lon * 1e5).round() / 1e5,
                name,
                sub,
                lines
            ]));
    }
    for (x, y) in geo::rio_tiles(zoom) {
        let dir = out.join(format!("cells/{zoom}/{x}/{y}"));
        std::fs::create_dir_all(&dir)?;
        let v = by_cell.remove(&(x, y)).unwrap_or_default();
        write_json(&dir.join("stops.json"), &json!(v))?;
    }
    Ok(())
}

fn write_json(path: &Path, v: &serde_json::Value) -> Result<()> {
    let bytes = serde_json::to_vec(v)?;
    std::fs::write(path, &bytes)?;
    let f = std::fs::File::create(path.with_extension("json.gz"))?;
    let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::new(9));
    enc.write_all(&bytes)?;
    enc.finish()?;
    Ok(())
}

/// Splits a published stop name into what a rider reads first and what they
/// read second.
///
/// The feed packs three different things into one string: the BRS corridor
/// lanes you must wait in (`BRS 1,2,3: Beira Mar`), the kind of stop
/// (`Ponto Final: Caju`), and a disambiguating complement after `::`. Left
/// whole it is noise on a phone; split, the second half becomes the
/// instruction that tells you where to stand.
pub fn stop_label(raw: &str) -> (String, String) {
    let raw = raw.trim();
    let (head, complement) = match raw.split_once("::") {
        Some((a, b)) => (a.trim(), b.trim().to_string()),
        None => (raw, String::new()),
    };

    let mut name = head.to_string();
    let mut sub = Vec::new();

    if let Some(rest) = head.strip_prefix("BRS ").or_else(|| head.strip_prefix("BRS")) {
        if let Some((lanes, after)) = rest.split_once(':') {
            let lanes: Vec<&str> = lanes.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            if !lanes.is_empty() {
                name = after.trim().to_string();
                sub.push(format!("BRS {}", lanes.join(", ")));
            }
        }
    } else if let Some(after) = head.strip_prefix("Ponto Final:") {
        name = after.trim().to_string();
        sub.push("Ponto final".to_string());
    } else if let Some(after) = head.strip_prefix("Ponto Regulador:") {
        name = after.trim().to_string();
        sub.push("Ponto regulador".to_string());
    }

    if !complement.is_empty() {
        sub.push(complement);
    }
    if name.is_empty() {
        name = raw.to_string();
    }
    (name, sub.join(" · "))
}

/// Route names are used as path segments, so keep them to a safe alphabet.
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toy() -> Gtfs {
        // A 1 km straight shape heading north with three stops.
        let mut lat = Vec::new();
        let mut lon = Vec::new();
        let mut cum = Vec::new();
        for i in 0..=10 {
            let d = i as f32 * 100.0;
            lat.push(-22.9 + (d as f64 / 111_195.0));
            lon.push(-43.4f64);
            cum.push(d);
        }
        let mut g = Gtfs {
            routes: vec![Route {
                id: "R1".into(),
                short_name: "474".into(),
                long_name: "Teste".into(),
                route_type: 700,
                shapes: vec![0],
                aka: Vec::new(),
                color: String::new(),
                text_color: String::new(),
            }],
            shapes: vec![Shape {
                id: "s1".into(),
                route: 0,
                direction: 0,
                headsign: "Centro".into(),
                lat: lat.iter().map(|v| (v * 1e6).round() as i32).collect(),
                lon: lon.iter().map(|v| (v * 1e6).round() as i32).collect(),
                cum,
                circular: false,
                stops: vec![
                    ShapeStop { stop: 0, dist: 0.0 },
                    ShapeStop { stop: 1, dist: 500.0 },
                    ShapeStop { stop: 2, dist: 1000.0 },
                ],
                planned_speed: 6.0,
                freq: vec![],
                x: vec![],
                y: vec![],
                bbox: [0.0; 4],
            }],
            stops: (0..3)
                .map(|i| Stop {
                    id: format!("p{i}"),
                    name: format!("Parada {i}"),
                    lat: -22.9,
                    lon: -43.4,
                    x: 0.0,
                    y: 0.0,
                })
                .collect(),
            ..Default::default()
        };
        g.finish();
        g
    }

    #[test]
    fn point_and_bearing_along_shape() {
        let g = toy();
        let sh = g.shape(0);
        assert!((sh.length() - 1000.0).abs() < 1.0);
        let (x, y) = sh.point_at(250.0);
        assert!(x.abs() < 1.0, "x={x}");
        assert!((y - sh.y[0] - 250.0).abs() < 2.0, "y={y}");
        // The shape heads north.
        assert!(geo::bearing_diff(sh.bearing_at(500.0), 0.0) < 5.0);
    }

    #[test]
    fn next_stop_skips_the_one_just_passed() {
        let g = toy();
        let sh = g.shape(0);
        assert_eq!(sh.next_stop_idx(0.0), Some(0), "standing at the terminal");
        assert_eq!(sh.next_stop_idx(20.0), Some(1));
        assert_eq!(sh.next_stop_idx(499.0), Some(1), "one metre before the stop");
        assert_eq!(sh.next_stop_idx(502.0), Some(1), "still at the stop");
        assert_eq!(sh.next_stop_idx(520.0), Some(2), "clearly past it");
        assert_eq!(sh.next_stop_idx(1000.0), Some(2));
        assert_eq!(sh.next_stop_idx(1010.0), None);
    }

    #[test]
    fn route_colors_are_normalised() {
        assert_eq!(hex_color("ff7600"), "#FF7600");
        assert_eq!(hex_color(" #1B47B9 "), "#1B47B9");
        assert_eq!(hex_color(""), "");
        assert_eq!(hex_color("red"), "");
    }

    #[test]
    fn holidays_run_the_sunday_service() {
        let dir = std::env::temp_dir().join(format!("busones-cal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("calendar.txt"),
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             D_REG,0,0,0,0,0,0,1,20221231,20271231\n\
             S_REG,0,0,0,0,0,1,0,20221231,20271231\n\
             U_REG,1,1,1,1,1,0,0,20221231,20271231\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("calendar_dates.txt"),
            "service_id,date,exception_type\n\
             D_REG,20261225,1\nU_REG,20261225,2\n\
             D_REG,20261114,1\nS_REG,20261114,2\n\
             U_REG,20261224,2\n\
             D_REG,20261227,1\n",
        )
        .unwrap();
        let d = read_calendar(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(d.weekdays, [Some(2), Some(0), Some(0), Some(0), Some(0), Some(0), Some(1)]);
        assert_eq!(d.exceptions.get("20261225"), Some(&Some(2)), "Christmas, a Friday");
        assert_eq!(d.exceptions.get("20261114"), Some(&Some(2)), "a Saturday holiday");
        assert_eq!(d.exceptions.get("20261224"), Some(&None), "weekday service removed, none added");
        assert!(!d.exceptions.contains_key("20261227"), "Sunday service on a Sunday changes nothing");
    }

    #[test]
    fn service_lookup_handles_zero_padding() {
        let mut g = toy();
        g.by_short.insert("007".into(), 0);
        assert_eq!(g.route_of_service("474"), Some(0));
        assert_eq!(g.route_of_service("7"), Some(0));
        assert_eq!(g.route_of_service("007"), Some(0));
        assert_eq!(g.route_of_service(""), None);
        assert_eq!(g.route_of_service("GARAGEM"), None);
    }

    #[test]
    fn service_table_gives_provisional_lines_their_public_name() {
        let mut g = toy();
        g.routes[0].short_name = "LECD140".into();
        g.index_names();
        let t = ServiceTable {
            aliases: [("685".to_string(), "LECD140".to_string())].into(),
            not_service: vec![" 3 ".into()],
        };
        assert!(g.apply_services(&t).is_empty());
        assert_eq!(g.routes[0].short_name, "685", "shown under the number riders know");
        assert_eq!(g.route_of_service("685"), Some(0));
        assert_eq!(g.route_of_service("LECD140"), Some(0), "the old code still resolves");
        assert!(g.not_service.contains("3"));
    }

    #[test]
    fn service_table_keeps_a_final_number_and_reports_bad_rows() {
        let mut g = toy();
        g.routes[0].short_name = "856".into();
        g.index_names();
        let t = ServiceTable {
            aliases: [
                ("LECD156".to_string(), "856".to_string()),
                ("999".to_string(), "LECD999".to_string()),
            ]
            .into(),
            not_service: vec![],
        };
        let problems = g.apply_services(&t);
        assert_eq!(g.routes[0].short_name, "856", "a final number is not replaced");
        assert_eq!(g.route_of_service("LECD156"), Some(0));
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn stop_names_split_into_what_to_read_first_and_second() {
        assert_eq!(stop_label("Barão de Tefé"), ("Barão de Tefé".into(), String::new()));
        assert_eq!(
            stop_label("BRS 1,2,3: Beira Mar"),
            ("Beira Mar".into(), "BRS 1, 2, 3".into())
        );
        assert_eq!(
            stop_label("BRS 1,2,3,4,5,I: Cruz Vermelha"),
            ("Cruz Vermelha".into(), "BRS 1, 2, 3, 4, 5, I".into())
        );
        assert_eq!(
            stop_label("Ponto Final: Caju :: General Sampaio"),
            ("Caju".into(), "Ponto final · General Sampaio".into())
        );
        assert_eq!(
            stop_label("Ponto Regulador: Castelo :: Linhas Área Central"),
            ("Castelo".into(), "Ponto regulador · Linhas Área Central".into())
        );
        assert_eq!(
            stop_label("Dinah de Queiroz :: Sentido Ponte / T. Gentileza"),
            ("Dinah de Queiroz".into(), "Sentido Ponte / T. Gentileza".into())
        );
        // Names that are already clean, and shapes we do not recognise, survive.
        assert_eq!(stop_label("Terminal Churchill").0, "Terminal Churchill");
        assert_eq!(stop_label("BRSomething odd").0, "BRSomething odd");
        assert_eq!(stop_label("  ").0, "");
    }

    #[test]
    fn headways_merge_equal_hours_and_skip_idle_ones() {
        let mut r = [[0.0f32; 24]; 3];
        for h in 6..9 { r[0][h] = 12.0; }   // every 5 min
        for h in 9..16 { r[0][h] = 6.0; }   // every 10 min
        r[2][10] = 1.0;                     // Sunday, once an hour
        assert_eq!(
            encode_headways(&r),
            vec![[0, 6, 9, 5], [0, 9, 16, 10], [2, 10, 11, 60]]
        );
    }

    #[test]
    fn parses_gtfs_times_past_midnight() {
        assert_eq!(hms("17:55:00"), Some(64_500));
        assert_eq!(hms("25:10:05"), Some(90_605));
        assert_eq!(hms(""), None);
        assert_eq!(hms("xx"), None);
    }

    #[test]
    fn sanitizes_route_names_for_paths() {
        assert_eq!(sanitize("474"), "474");
        assert_eq!(sanitize("SN238"), "SN238");
        assert_eq!(sanitize("a/b c"), "a_b_c");
    }
}
