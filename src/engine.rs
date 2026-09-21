//! Fleet state: hygiene, deduplication, matching and the phase machine.
//!
//! Everything that needs more than one vehicle, or more than one fix, lives
//! here rather than in the browser. That is the difference from the previous
//! attempt, where direction came from two client polls and nothing remembered
//! anything.

use std::collections::hash_map::{DefaultHasher, Entry};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::geo;
use crate::gtfs::Gtfs;
use crate::matcher::{self, Hyp};
use crate::model::{Dir, Fix, Phase, ServiceState, Vendor};
use crate::publish::Out;

/// A fix older than this never touches live state; it can only feed history.
pub const LIVE_MAX_AGE_S: i64 = 300;
/// No new fix for this long and the vehicle stops being predicted.
pub const STALE_AFTER_S: i64 = 180;
/// No new fix for this long and the vehicle is forgotten entirely.
pub const DROP_AFTER_S: i64 = 300;
pub const FUTURE_TOLERANCE_S: i64 = 60;
/// Faster than this between two fixes of the same unit and the fix is bogus.
pub const MAX_SPEED_MS: f32 = 27.0;
pub const TERMINAL_RADIUS_M: f32 = 250.0;
/// 3 km/h, the threshold SMTR's own pipeline uses for "stopped".
pub const STOPPED_MS: f32 = 0.83;
pub const OFF_ROUTE_S: i64 = 600;
/// A vehicle that has not moved this far in [`PARKED_AFTER_S`] and never
/// confirmed a direction is sitting in a yard, not serving its line.
pub const PARKED_RADIUS_M: f32 = 100.0;
pub const PARKED_AFTER_S: i64 = 600;
/// Two fixes from the two on-board units can be seconds apart; below this gap
/// no physical rule may be applied to the pair.
pub const MOTION_MIN_DT_S: f32 = 10.0;
const SPEED_HALF_LIFE_S: f32 = 60.0;
const DEDUPE_WINDOW_S: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accepted,
    Duplicate,
    NotNewer,
    TooOld,
    Future,
    OutsideRio,
    OutOfService,
    UnknownService,
    ImplausibleJump,
}

#[derive(Default, Clone)]
pub struct Counters {
    pub accepted: u64,
    pub duplicate: u64,
    pub not_newer: u64,
    pub too_old: u64,
    pub future: u64,
    pub outside: u64,
    pub out_of_service: u64,
    pub unknown_service: u64,
    pub jump: u64,
    pub unmatched_service: u64,
}

impl Counters {
    fn bump(&mut self, o: Outcome) {
        match o {
            Outcome::Accepted => self.accepted += 1,
            Outcome::Duplicate => self.duplicate += 1,
            Outcome::NotNewer => self.not_newer += 1,
            Outcome::TooOld => self.too_old += 1,
            Outcome::Future => self.future += 1,
            Outcome::OutsideRio => self.outside += 1,
            Outcome::OutOfService => self.out_of_service += 1,
            Outcome::UnknownService => self.unknown_service += 1,
            Outcome::ImplausibleJump => self.jump += 1,
        }
    }
}

#[derive(Default)]
struct Seen {
    map: HashMap<u64, i64>,
    last_prune: i64,
}

impl Seen {
    fn key(f: &Fix) -> u64 {
        let mut h = DefaultHasher::new();
        f.vehicle.hash(&mut h);
        f.vendor.code().hash(&mut h);
        f.t.hash(&mut h);
        h.finish()
    }

    /// True when this exact report has not been seen in the last ten minutes.
    fn insert(&mut self, f: &Fix, now: i64) -> bool {
        if now - self.last_prune > 60 {
            self.map.retain(|_, &mut t| now - t < DEDUPE_WINDOW_S);
            self.last_prune = now;
        }
        match self.map.entry(Self::key(f)) {
            Entry::Occupied(_) => false,
            Entry::Vacant(v) => {
                v.insert(now);
                true
            }
        }
    }
}

pub struct Vehicle {
    pub id: String,
    pub vendor: Vendor,
    pub t: i64,
    pub lat: f64,
    pub lon: f64,
    pub x: f32,
    pub y: f32,
    pub heading: Option<f32>,
    pub service: String,
    pub route: Option<u32>,
    pub dir_hint: Option<Dir>,
    pub beam: Vec<Hyp>,
    pub fixed: Option<Hyp>,
    pub phase: Phase,
    /// Metres per second, derived from movement, never from the feed's own
    /// `velocidade` field (zero in a third of the records).
    pub v_est: f32,
    pub brt: bool,
    /// Accepted fixes and hypothesis resets, for diagnosing a low match rate.
    pub accepted: u32,
    pub resets: u32,
    last_by_vendor: HashMap<Vendor, (i64, f32, f32)>,
    motion_ref: Option<(i64, f32, f32)>,
    /// Time and place the vehicle was last seen more than 100 m from.
    moved_ref: Option<(i64, f32, f32)>,
    off_route_since: Option<i64>,
    slow_fixes: u8,
    fast_fixes: u8,
}

impl Vehicle {
    fn new(f: &Fix) -> Vehicle {
        Vehicle {
            id: f.vehicle.clone(),
            vendor: f.vendor,
            t: 0,
            lat: f.lat,
            lon: f.lon,
            x: f.x,
            y: f.y,
            heading: f.heading,
            service: String::new(),
            route: None,
            dir_hint: None,
            beam: Vec::new(),
            fixed: None,
            phase: Phase::Pending,
            v_est: 0.0,
            brt: f.brt,
            accepted: 0,
            resets: 0,
            last_by_vendor: HashMap::new(),
            motion_ref: None,
            moved_ref: None,
            off_route_since: None,
            slow_fixes: 0,
            fast_fixes: 0,
        }
    }
}

pub struct Engine {
    pub gtfs: Arc<Gtfs>,
    pub vehicles: HashMap<String, Vehicle>,
    pub counters: Counters,
    /// Live service codes with no route in the published feed, counted so the
    /// alias table can be filled from evidence instead of guesswork.
    pub unknown_services: HashMap<String, usize>,
    seen: Seen,
}

impl Engine {
    pub fn new(gtfs: Arc<Gtfs>) -> Engine {
        Engine {
            gtfs,
            vehicles: HashMap::new(),
            counters: Counters::default(),
            unknown_services: HashMap::new(),
            seen: Seen::default(),
        }
    }

    /// Applies a batch in chronological order, which is what the matcher needs
    /// and what the aggregator's server-time ordering does not guarantee.
    pub fn apply_batch(&mut self, mut fixes: Vec<Fix>, now: i64) {
        fixes.sort_by_key(|f| f.t);
        for f in fixes {
            let o = self.apply(f, now);
            self.counters.bump(o);
        }
    }

    pub fn apply(&mut self, f: Fix, now: i64) -> Outcome {
        if !geo::in_rio(f.lat, f.lon) {
            return Outcome::OutsideRio;
        }
        if f.t > now + FUTURE_TOLERANCE_S {
            return Outcome::Future;
        }
        let service = f.service.clone().unwrap_or_default();
        let service_state = crate::model::classify_service(&service);
        if service_state == ServiceState::OutOfService
            || self.gtfs.not_service.contains(service.trim())
        {
            self.vehicles.remove(&f.vehicle);
            return Outcome::OutOfService;
        }
        // A vendor that sends no service tells us nothing; a vehicle we have
        // never seen a line for cannot be placed on one.
        if service_state == ServiceState::Unknown
            && self
                .vehicles
                .get(&f.vehicle)
                .is_none_or(|v| v.service.is_empty())
        {
            return Outcome::UnknownService;
        }
        if !self.seen.insert(&f, now) {
            return Outcome::Duplicate;
        }
        if now - f.t > LIVE_MAX_AGE_S {
            // Real but late: useful for history, never for the live map.
            return Outcome::TooOld;
        }

        let gtfs = self.gtfs.clone();
        let v = self
            .vehicles
            .entry(f.vehicle.clone())
            .or_insert_with(|| Vehicle::new(&f));

        // The timestamp must advance. This is what stops the BRT snapshot, which
        // repeats the same rows every minute, from looking like fresh movement.
        if v.t != 0 && f.t <= v.t {
            return Outcome::NotNewer;
        }

        // Physical plausibility is judged against the same on-board unit only:
        // the two units on an SPPO bus report independently, metres apart.
        if let Some(&(pt, px, py)) = v.last_by_vendor.get(&f.vendor) {
            let dt = (f.t - pt) as f32;
            if dt > 0.0 && geo::dist(px, py, f.x, f.y) / dt > MAX_SPEED_MS {
                return Outcome::ImplausibleJump;
            }
        }

        let prev = if v.t == 0 { None } else { Some((v.t, v.x, v.y)) };
        let same_vendor_prev = v.last_by_vendor.get(&f.vendor).copied();
        v.last_by_vendor.insert(f.vendor, (f.t, f.x, f.y));

        v.accepted += 1;
        v.vendor = f.vendor;
        v.t = f.t;
        v.lat = f.lat;
        v.lon = f.lon;
        v.x = f.x;
        v.y = f.y;
        v.brt |= f.brt;
        if f.heading.is_some() {
            v.heading = f.heading;
        }
        if service_state == ServiceState::Named && service != v.service {
            v.service = service.clone();
            v.route = gtfs.route_of_service(&service);
            // A new service means the previous trip is over.
            v.beam.clear();
            v.fixed = None;
            v.phase = Phase::Pending;
            v.resets += 1;
        }
        if f.dir.is_some() {
            v.dir_hint = f.dir;
        }

        match v.moved_ref {
            Some((_, mx, my)) if geo::dist(mx, my, f.x, f.y) > PARKED_RADIUS_M => {
                v.moved_ref = Some((f.t, f.x, f.y));
            }
            None => v.moved_ref = Some((f.t, f.x, f.y)),
            _ => {}
        }

        // Derived speed, sampled only over gaps long enough to mean something.
        if let Some((mt, mx, my)) = v.motion_ref {
            let dt = (f.t - mt) as f32;
            if dt >= MOTION_MIN_DT_S {
                let inst = (geo::dist(mx, my, f.x, f.y) / dt).min(MAX_SPEED_MS);
                let alpha = 1.0 - 0.5f32.powf(dt / SPEED_HALF_LIFE_S);
                v.v_est += alpha * (inst - v.v_est);
                v.motion_ref = Some((f.t, f.x, f.y));
            }
        } else {
            v.motion_ref = Some((f.t, f.x, f.y));
        }

        let cands = candidate_shapes(&gtfs, v, &f);
        let unknown = v.route.is_none() && f.shape_id.is_none() && f.trip_id.is_none();
        if !cands.preferred.is_empty() {
            let (travelled, usable) = match (prev, same_vendor_prev) {
                (Some((pt, px, py)), sv) => {
                    let dt = (f.t - pt) as f32;
                    let same_unit = sv.is_some_and(|(st, _, _)| st == pt);
                    (
                        geo::dist(px, py, f.x, f.y),
                        dt >= MOTION_MIN_DT_S || same_unit,
                    )
                }
                (None, _) => (0.0, false),
            };
            // The candidate set must include whatever the beam is already
            // tracking. One vendor names a shape_id while the other only gives
            // a direction, so recomputing the set from scratch on every fix
            // would alternate between two shapes and force a fresh hypothesis
            // each time, which is how a bus moving at 6 m/s can report twenty
            // fixes and never confirm a direction. The cost model, not the
            // feed's inconsistency, decides which shape wins.
            let mut shapes = cands.preferred.clone();
            for h in &v.beam {
                if !shapes.contains(&h.shape) {
                    shapes.push(h.shape);
                }
            }
            let mut step = matcher::Step {
                gtfs: &gtfs,
                shapes: &shapes,
                px: f.x,
                py: f.y,
                travelled,
                usable_motion: usable,
                heading: v.heading,
                moving: v.v_est > 1.5,
            };
            let before = v.beam.len();
            matcher::advance(&mut v.beam, &step);
            // The feed's `sentido` is a hint, not a fact. When trusting it
            // leaves the vehicle nowhere near any shape, try the whole route.
            let missed = v.beam.first().is_some_and(|h| h.dist == f32::MAX)
                || (before == 0 && v.beam.is_empty());
            if missed && !cands.fallback.is_empty() {
                step.shapes = &cands.fallback;
                matcher::advance(&mut v.beam, &step);
            }
            if v.beam.first().is_some_and(|h| h.fixes == 0) && before > 0 {
                v.resets += 1;
            }
            if let Some(c) = matcher::committed(&v.beam) {
                v.fixed = Some(c);
            } else if v.fixed.is_some() {
                // Keep the previous commitment but follow the best hypothesis on
                // the same shape, so a momentary tie does not drop the vehicle.
                if let Some(h) = v
                    .beam
                    .iter()
                    .find(|h| Some(h.shape) == v.fixed.map(|f| f.shape))
                {
                    v.fixed = Some(*h);
                }
            }
        }

        update_phase(v, &gtfs, now);
        if unknown {
            self.counters.unmatched_service += 1;
            let svc = service.clone();
            if !svc.is_empty() {
                *self.unknown_services.entry(svc).or_insert(0) += 1;
            }
        }
        Outcome::Accepted
    }

    /// Ages vehicles out. Called on every publish cycle.
    pub fn tick(&mut self, now: i64) {
        self.vehicles.retain(|_, v| now - v.t <= DROP_AFTER_S);
        for v in self.vehicles.values_mut() {
            if now - v.t > STALE_AFTER_S {
                v.phase = Phase::Stale;
            }
        }
    }

    pub fn snapshot(&self) -> Vec<Out> {
        let mut out = Vec::with_capacity(self.vehicles.len());
        for v in self.vehicles.values() {
            let route = v.route.map(|r| self.gtfs.route(r));
            let line = route
                .map(|r| r.short_name.clone())
                .unwrap_or_else(|| v.service.clone());
            if line.trim().is_empty() {
                continue;
            }
            let (dir, shape, along, next_stop) = match v.fixed {
                Some(h) => {
                    let sh = self.gtfs.shape(h.shape);
                    let d = if sh.circular {
                        Dir::Circular
                    } else if sh.direction == 1 {
                        Dir::Volta
                    } else {
                        Dir::Ida
                    };
                    let ns = sh
                        .next_stop_idx(h.along)
                        .map(|i| self.gtfs.stops[sh.stops[i].stop as usize].id.clone());
                    (Some(d.as_str()), Some(sh.id.clone()), Some(h.along), ns)
                }
                None => (None, None, None, None),
            };
            let conf = match (v.fixed, v.beam.first()) {
                (Some(h), _) if h.dist <= matcher::CONFIDENT_M => 0.95,
                (Some(_), _) => 0.7,
                (None, Some(h)) if h.dist <= matcher::CONFIDENT_M => 0.35,
                _ => 0.15,
            };
            out.push(Out {
                id: v.id.clone(),
                line,
                t: v.t,
                lat: v.lat,
                lon: v.lon,
                bearing: v.heading,
                phase: v.phase.as_str(),
                dir,
                shape,
                along,
                speed_kmh: v.v_est * 3.6,
                next_stop,
                conf,
                vendor: v.vendor.as_str(),
                brt: v.brt,
            });
        }
        out.sort_by(|a, b| a.line.cmp(&b.line).then(a.id.cmp(&b.id)));
        out
    }

    /// Everything approaching a stop, ready for the "near me" screen.
    ///
    /// A browser cannot work this out for itself: in the densest part of the
    /// city it would need 820 KB of route bundles to turn vehicle positions
    /// into "three stops away". The server already knows where each bus is
    /// along its shape, so it costs a few milliseconds here and about 2 KB on
    /// the wire.
    pub fn arrivals(&self) -> Vec<crate::publish::Arrival> {
        const MAX_AHEAD_M: f32 = 8_000.0;
        const MAX_STOPS_AHEAD: usize = 25;
        const MIN_FOR_MEDIAN: usize = 3;

        // How fast each line is moving right now, from its own buses.
        let mut speeds: HashMap<u32, Vec<f32>> = HashMap::new();
        for v in self.vehicles.values() {
            if v.phase == Phase::InProgress && v.fixed.is_some() {
                if let Some(r) = v.route {
                    speeds.entry(r).or_default().push(v.v_est);
                }
            }
        }
        let line_speed: HashMap<u32, f32> = speeds
            .into_iter()
            .filter(|(_, s)| s.len() >= MIN_FOR_MEDIAN)
            .map(|(r, mut s)| {
                s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                (r, s[s.len() / 2])
            })
            .collect();

        let mut out = Vec::new();
        for v in self.vehicles.values() {
            if v.phase != Phase::InProgress {
                continue;
            }
            let Some(h) = v.fixed else { continue };
            let Some(r) = v.route else { continue };
            let sh = self.gtfs.shape(h.shape);
            let line = &self.gtfs.route(r).short_name;
            let dir = if sh.circular {
                Dir::Circular
            } else if sh.direction == 1 {
                Dir::Volta
            } else {
                Dir::Ida
            };
            let Some(first) = sh.next_stop_idx(h.along) else {
                continue;
            };
            for (n, s) in sh.stops.iter().enumerate().skip(first).take(MAX_STOPS_AHEAD) {
                let ahead = s.dist - h.along;
                if ahead > MAX_AHEAD_M {
                    break;
                }
                let (eta_lo, eta_hi) =
                    eta_range(ahead, v.v_est, line_speed.get(&r).copied(), sh.planned_speed);
                out.push(crate::publish::Arrival {
                    stop: s.stop,
                    line: line.clone(),
                    to: sh.headsign.clone(),
                    dir: dir.as_str(),
                    vehicle: v.id.clone(),
                    t: v.t,
                    stops_away: (n - first) as u16,
                    metres: ahead.max(0.0),
                    eta_lo,
                    eta_hi,
                });
            }
        }
        out
    }

    pub fn phase_counts(&self) -> std::collections::BTreeMap<String, usize> {
        let mut m = std::collections::BTreeMap::new();
        for v in self.vehicles.values() {
            *m.entry(v.phase.as_str().to_string()).or_insert(0) += 1;
        }
        m.insert("tracked".into(), self.vehicles.len());
        let matched = self.vehicles.values().filter(|v| v.fixed.is_some()).count();
        // The denominator the direction target is measured against: a vehicle
        // on a known line, recently heard from, and not sitting in a yard.
        let in_service = self
            .vehicles
            .values()
            .filter(|v| {
                v.route.is_some() && v.phase != Phase::Parked && v.phase != Phase::Stale
            })
            .count();
        m.insert("matched".into(), matched);
        m.insert("in_service".into(), in_service);
        m.insert(
            "direction_rate_pct".into(),
            if in_service == 0 {
                0
            } else {
                (matched.min(in_service) * 100) / in_service
            },
        );
        m
    }

    /// Why vehicles are not publishing a direction. This is the breakdown that
    /// tells a low match rate caused by parked buses from one caused by the
    /// matcher, so it stays in the health endpoint rather than in a debug flag.
    pub fn diagnostics(&self) -> std::collections::BTreeMap<String, usize> {
        let mut m = std::collections::BTreeMap::new();
        let mut bump = |k: &str| *m.entry(k.to_string()).or_insert(0) += 1;
        for v in self.vehicles.values() {
            if v.fixed.is_some() || v.phase == Phase::Stale || v.phase == Phase::Parked {
                continue;
            }
            if v.route.is_none() {
                bump("no_route");
                continue;
            }
            let Some(best) = v.beam.first() else {
                bump("no_candidate_pass");
                continue;
            };
            if best.dist > matcher::TENTATIVE_M {
                bump("far_from_shape");
            } else if best.fixes < matcher::FIXES_TO_CONFIRM {
                bump("too_few_fixes");
            } else if best.progress < matcher::PROGRESS_TO_CONFIRM_M {
                bump("not_enough_progress");
            } else {
                bump("rival_pass_too_close");
            }
        }
        m
    }

    /// A few pending vehicles in full, for `BUSONES_DEBUG_PENDING`.
    pub fn debug_pending(&self, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        for v in self.vehicles.values() {
            if v.fixed.is_some() || v.phase != Phase::Pending || v.v_est < 2.0 || v.route.is_none()
            {
                continue;
            }
            let best = v
                .beam
                .first()
                .map(|h| {
                    format!(
                        "shape={} along={:.0} cost={:.1} dist={:.0} prog={:.0} fixes={}",
                        self.gtfs.shape(h.shape).id,
                        h.along,
                        h.cost,
                        h.dist,
                        h.progress,
                        h.fixes
                    )
                })
                .unwrap_or_else(|| "no hypothesis".into());
            out.push(format!(
                "{} svc={} shapes={} beam={} accepted={} resets={} v={:.1}m/s | {}",
                v.id,
                v.service,
                v.route.map(|r| self.gtfs.route(r).shapes.len()).unwrap_or(0),
                v.beam.len(),
                v.accepted,
                v.resets,
                v.v_est,
                best
            ));
            if out.len() >= limit {
                break;
            }
        }
        out
    }

    pub fn newest_fix(&self) -> i64 {
        self.vehicles.values().map(|v| v.t).max().unwrap_or(0)
    }
}

struct Candidates {
    /// What the feed says the vehicle is doing.
    preferred: Vec<u32>,
    /// Everything the route runs, tried only when `preferred` matches nothing.
    fallback: Vec<u32>,
}

/// Travel time to a stop `ahead` metres downstream, as a range in seconds.
///
/// The timetable cannot provide this: its stop times are interpolated at
/// constant speed, so they know a route's average and nothing about where it
/// is slow. What we do know is live. Three sources, blended by distance:
///
/// * the bus's own recent speed, the best evidence for the next kilometre or
///   so, because a bus crawling in a jam keeps crawling for a while;
/// * the median speed of the other buses on the same line right now, which is
///   how traffic is on this corridor at this moment;
/// * the timetable's planned speed, only when too few buses are running for a
///   median to mean anything.
///
/// The answer is always a range: 0.7 to 1.4 times the central estimate, plus
/// half a minute. Asymmetric on purpose, because the bus that arrives earlier
/// than promised is the one people miss, and every app in this market shows a
/// single confident minute that turns out to be wrong.
pub fn eta_range(ahead: f32, own: f32, line: Option<f32>, planned: f32) -> (f32, f32) {
    const MIN_MS: f32 = 8.0 / 3.6;
    const MAX_MS: f32 = 40.0 / 3.6;
    let w = (-ahead.max(0.0) / 1_500.0).exp();
    let base = line.unwrap_or(planned).clamp(MIN_MS, MAX_MS);
    let v = (w * own.max(0.0) + (1.0 - w) * base).clamp(MIN_MS, MAX_MS);
    let c = ahead.max(0.0) / v;
    (0.7 * c, 1.4 * c + 30.0)
}

/// Shapes worth testing for this fix, most trustworthy source first.
fn candidate_shapes(gtfs: &Gtfs, v: &Vehicle, f: &Fix) -> Candidates {
    let none = Candidates {
        preferred: Vec::new(),
        fallback: Vec::new(),
    };
    if let Some(sid) = f.shape_id.as_deref() {
        if let Some(&si) = gtfs.by_shape_id.get(sid) {
            let fallback = v
                .route
                .map(|r| gtfs.route(r).shapes.clone())
                .unwrap_or_default();
            return Candidates {
                preferred: vec![si],
                fallback,
            };
        }
    }
    if let Some(tid) = f.trip_id.as_deref() {
        if let Some(&si) = gtfs.by_trip.get(tid) {
            return Candidates {
                preferred: vec![si],
                fallback: Vec::new(),
            };
        }
    }
    let Some(r) = v.route else {
        return none;
    };
    let all = &gtfs.route(r).shapes;
    let want = f.dir.or(v.dir_hint).and_then(|d| d.direction_id());
    let filtered: Vec<u32> = all
        .iter()
        .copied()
        .filter(|&s| want.is_none_or(|d| gtfs.shape(s).direction == d))
        .collect();
    if filtered.is_empty() || filtered.len() == all.len() {
        Candidates {
            preferred: all.clone(),
            fallback: Vec::new(),
        }
    } else {
        Candidates {
            preferred: filtered,
            fallback: all.clone(),
        }
    }
}

fn update_phase(v: &mut Vehicle, gtfs: &Gtfs, now: i64) {
    let best_dist = v.beam.first().map(|h| h.dist).unwrap_or(f32::MAX);
    if best_dist > matcher::TENTATIVE_M {
        let since = *v.off_route_since.get_or_insert(now);
        if now - since > OFF_ROUTE_S {
            v.phase = Phase::OffRoute;
            return;
        }
    } else {
        v.off_route_since = None;
    }

    let Some(h) = v.fixed else {
        if v.phase != Phase::OffRoute {
            let stuck = v
                .moved_ref
                .is_some_and(|(mt, _, _)| now - mt > PARKED_AFTER_S);
            v.phase = if stuck { Phase::Parked } else { Phase::Pending };
        }
        return;
    };

    let sh = gtfs.shape(h.shape);
    let len = sh.length();
    let in_terminal = h.along < TERMINAL_RADIUS_M || h.along > len - TERMINAL_RADIUS_M;
    let slow = v.v_est < STOPPED_MS;

    if v.phase == Phase::Layover {
        // Repositioning between bays inside the terminal is not a departure.
        let clear = h.along > TERMINAL_RADIUS_M + 100.0 && h.along < len - TERMINAL_RADIUS_M;
        if clear && !slow {
            v.fast_fixes = v.fast_fixes.saturating_add(1);
        } else {
            v.fast_fixes = 0;
        }
        if v.fast_fixes >= 2 {
            v.phase = Phase::InProgress;
            v.slow_fixes = 0;
        }
        return;
    }

    if in_terminal && slow {
        v.slow_fixes = v.slow_fixes.saturating_add(1);
    } else {
        v.slow_fixes = 0;
    }
    v.phase = if v.slow_fixes >= 2 {
        v.fast_fixes = 0;
        Phase::Layover
    } else {
        Phase::InProgress
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtfs::{Route, Shape, ShapeStop, Stop};

    fn gtfs() -> Arc<Gtfs> {
        // 4 km straight shape heading east with stops every kilometre.
        let m_lon =
            1.0 / (6_371_000.0f64 * std::f64::consts::PI / 180.0 * (-22.9f64).to_radians().cos());
        let mut lat = Vec::new();
        let mut lon = Vec::new();
        let mut cum = Vec::new();
        for i in 0..=80 {
            lat.push((-22.9f64 * 1e6).round() as i32);
            lon.push(((-43.4 + (i as f64 * 50.0) * m_lon) * 1e6).round() as i32);
            cum.push((i * 50) as f32);
        }
        let mut g = Gtfs {
            routes: vec![Route {
                id: "R".into(),
                short_name: "474".into(),
                long_name: "Teste".into(),
                route_type: 700,
                shapes: vec![0],
                aka: Vec::new(),
                color: String::new(),
                text_color: String::new(),
            }],
            shapes: vec![Shape {
                id: "sh1".into(),
                route: 0,
                direction: 0,
                headsign: "Centro".into(),
                lat,
                lon,
                cum,
                circular: false,
                stops: (0..5)
                    .map(|i| ShapeStop {
                        stop: i,
                        dist: i as f32 * 1000.0,
                    })
                    .collect(),
                planned_speed: 6.0,
                freq: vec![],
                x: vec![],
                y: vec![],
                bbox: [0.0; 4],
            }],
            stops: (0..5)
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
        Arc::new(g)
    }

    fn fix_at(g: &Gtfs, id: &str, t: i64, along: f32, vendor: Vendor) -> Fix {
        let (x, y) = g.shape(0).point_at(along);
        let (lat, lon) = geo::unproject(x, y);
        let mut f = Fix::new(id.into(), vendor, t, lat, lon);
        f.service = Some("474".into());
        f.dir = Some(Dir::Ida);
        f
    }

    #[test]
    fn confirms_direction_and_reports_the_next_stop() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let t0 = 1_800_000_000;
        for (k, along) in [1500.0f32, 1650.0, 1800.0, 1950.0].iter().enumerate() {
            let now = t0 + k as i64 * 30;
            assert_eq!(
                e.apply(fix_at(&g, "A1", now, *along, Vendor::Zirix), now),
                Outcome::Accepted
            );
        }
        let out = e.snapshot();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].dir, Some("ida"));
        assert_eq!(out[0].ph_line(), "live");
        assert_eq!(out[0].next_stop.as_deref(), Some("p2"), "next km stop");
        assert!(out[0].speed_kmh > 10.0, "{}", out[0].speed_kmh);
    }

    #[test]
    fn garage_and_reserve_codes_never_reach_the_map() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        let mut f = fix_at(&g, "A1", now, 1000.0, Vendor::Zirix);
        f.service = Some("GARAGEM".into());
        assert_eq!(e.apply(f, now), Outcome::OutOfService);
        assert!(e.snapshot().is_empty());
    }

    #[test]
    fn codes_the_service_table_rules_out_never_reach_the_map() {
        let mut g = Arc::try_unwrap(gtfs()).ok().unwrap();
        g.not_service.insert("3".into());
        let g = Arc::new(g);
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        let mut f = fix_at(&g, "A1", now, 1000.0, Vendor::Conecta);
        f.service = Some("3".into());
        assert_eq!(e.apply(f, now), Outcome::OutOfService);
        assert!(e.snapshot().is_empty());
    }

    #[test]
    fn repeated_and_stale_rows_are_rejected() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        let f = fix_at(&g, "A1", now, 1000.0, Vendor::Zirix);
        assert_eq!(e.apply(f.clone(), now), Outcome::Accepted);
        // The BRT snapshot repeats identical rows every minute.
        assert_eq!(e.apply(f.clone(), now), Outcome::Duplicate);
        // A different unit reporting the same instant is not newer.
        let mut same = fix_at(&g, "A1", now, 1000.0, Vendor::Conecta);
        same.t = now;
        assert_eq!(e.apply(same, now), Outcome::NotNewer);
        // Hours old: history only.
        let old = fix_at(&g, "A2", now - 7200, 1000.0, Vendor::Sonda);
        assert_eq!(e.apply(old, now), Outcome::TooOld);
        assert_eq!(e.vehicles.len(), 1);
    }

    #[test]
    fn a_teleport_within_one_unit_is_dropped() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        assert_eq!(
            e.apply(fix_at(&g, "A1", now, 0.0, Vendor::Zirix), now),
            Outcome::Accepted
        );
        // 4 km in 10 s is 400 m/s.
        assert_eq!(
            e.apply(fix_at(&g, "A1", now + 10, 4000.0, Vendor::Zirix), now + 10),
            Outcome::ImplausibleJump
        );
    }

    #[test]
    fn the_other_onboard_unit_does_not_trigger_the_jump_filter() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        assert_eq!(
            e.apply(fix_at(&g, "A1", now, 1000.0, Vendor::Zirix), now),
            Outcome::Accepted
        );
        // Conecta reports the same bus 2 s later, 80 m away: 40 m/s across
        // vendors, which must not be read as a teleport.
        assert_eq!(
            e.apply(fix_at(&g, "A1", now + 2, 1080.0, Vendor::Conecta), now + 2),
            Outcome::Accepted
        );
    }

    #[test]
    fn silence_makes_a_vehicle_stale_then_forgotten() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let t0 = 1_800_000_000;
        e.apply(fix_at(&g, "A1", t0, 1000.0, Vendor::Zirix), t0);
        e.tick(t0 + 100);
        assert_ne!(e.vehicles["A1"].phase, Phase::Stale);
        e.tick(t0 + STALE_AFTER_S + 1);
        assert_eq!(e.vehicles["A1"].phase, Phase::Stale);
        e.tick(t0 + DROP_AFTER_S + 1);
        assert!(e.vehicles.is_empty(), "forgotten after five minutes");
    }

    #[test]
    fn a_bus_sitting_at_the_terminal_is_on_layover() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let t0 = 1_800_000_000;
        // Arrive with real progress so the direction commits.
        for (k, along) in [3300.0f32, 3500.0, 3700.0, 3900.0].iter().enumerate() {
            let now = t0 + k as i64 * 30;
            e.apply(fix_at(&g, "A1", now, *along, Vendor::Zirix), now);
        }
        assert_eq!(e.vehicles["A1"].phase, Phase::InProgress);
        // Then stand still inside the terminal zone.
        for k in 4..12 {
            let now = t0 + k * 30;
            e.apply(fix_at(&g, "A1", now, 3950.0, Vendor::Zirix), now);
        }
        assert_eq!(e.vehicles["A1"].phase, Phase::Layover);
    }

    #[test]
    fn vendors_naming_different_shapes_do_not_restart_the_hypothesis() {
        // A route with the usual pair of shapes: the return leg is the outbound
        // one reversed. Conecta names the return shape on every other fix while
        // Zirix only says "ida". The bus is on the outbound shape throughout.
        let mut g = (*gtfs()).clone_for_test();
        let mut lat = g.shapes[0].lat.clone();
        let mut lon = g.shapes[0].lon.clone();
        lat.reverse();
        lon.reverse();
        let cum = g.shapes[0].cum.clone();
        g.shapes.push(crate::gtfs::Shape {
            id: "sh2".into(),
            route: 0,
            direction: 1,
            headsign: "Volta".into(),
            lat,
            lon,
            cum,
            circular: false,
            stops: vec![],
            planned_speed: 6.0,
                freq: vec![],
            x: vec![],
            y: vec![],
            bbox: [0.0; 4],
        });
        g.routes[0].shapes = vec![0, 1];
        g.finish();
        let g = Arc::new(g);

        let mut e = Engine::new(g.clone());
        let t0 = 1_800_000_000;
        for (k, along) in [1500.0f32, 1650.0, 1800.0, 1950.0, 2100.0, 2250.0, 2400.0, 2550.0]
            .iter()
            .enumerate()
        {
            let now = t0 + k as i64 * 20;
            let mut f = fix_at(&g, "A1", now, *along, Vendor::Zirix);
            f.heading = Some(90.0); // heading east, along the outbound shape
            if k % 2 == 0 {
                f.vendor = Vendor::Conecta;
                f.shape_id = Some("sh2".into());
            }
            assert_eq!(e.apply(f, now), Outcome::Accepted, "fix {k}");
        }
        let v = &e.vehicles["A1"];
        // A couple of restarts while the beam settles on the right shape are
        // expected; restarting on most fixes is the bug this guards against.
        assert!(
            (v.resets as usize) * 3 < v.accepted as usize,
            "hypothesis restarted {} times in {} fixes",
            v.resets,
            v.accepted
        );
        let fixed = v.fixed.expect("direction never confirmed");
        assert_eq!(fixed.shape, 0, "committed to the wrong shape: {:?}", v.beam);
    }

    #[test]
    fn a_blank_service_from_one_vendor_does_not_wipe_the_vehicle() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let t0 = 1_800_000_000;
        // Conecta names the line, Zirix leaves it blank, alternating. The
        // vehicle must survive and keep accumulating evidence.
        for (k, along) in [1500.0f32, 1650.0, 1800.0, 1950.0, 2100.0, 2250.0]
            .iter()
            .enumerate()
        {
            let now = t0 + k as i64 * 15;
            let mut f = fix_at(&g, "A1", now, *along, Vendor::Zirix);
            if k % 2 == 0 {
                f.vendor = Vendor::Conecta;
            } else {
                f.service = None;
            }
            assert_eq!(e.apply(f, now), Outcome::Accepted, "fix {k}");
        }
        let v = &e.vehicles["A1"];
        assert_eq!(v.service, "474", "the known line is kept");
        assert!(v.fixed.is_some(), "direction still confirms");
    }

    #[test]
    fn a_never_seen_vehicle_with_no_service_is_not_tracked() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        let mut f = fix_at(&g, "A1", now, 1000.0, Vendor::Zirix);
        f.service = None;
        assert_eq!(e.apply(f, now), Outcome::UnknownService);
        assert!(e.vehicles.is_empty());
    }

    #[test]
    fn a_bus_that_never_moves_is_parked_not_pending() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let t0 = 1_800_000_000;
        for k in 0..25 {
            let now = t0 + k * 30;
            e.apply(fix_at(&g, "A1", now, 500.0, Vendor::Zirix), now);
        }
        assert_eq!(e.vehicles["A1"].phase, Phase::Parked);
        let counts = e.phase_counts();
        // A parked bus is outside the direction-rate denominator.
        assert_eq!(counts["in_service"], 0);
        assert_eq!(counts["parked"], 1);
    }

    #[test]
    fn eta_is_a_range_that_never_collapses_to_a_point() {
        // 3 km at a line speed of 20 km/h is 9 minutes in the middle.
        let (lo, hi) = eta_range(3_000.0, 5.5, Some(20.0 / 3.6), 6.0);
        assert!(lo < 540.0 && hi > 540.0, "{lo}..{hi}");
        // Asymmetric: more room on the late side than the early one.
        let mid = 3_000.0 / (20.0 / 3.6);
        assert!(hi - mid > mid - lo, "late side {} early side {}", hi - mid, mid - lo);
    }

    #[test]
    fn a_stopped_bus_close_by_does_not_produce_an_absurd_wait() {
        // Standing at a light 100 m away: own speed 0, but the floor holds.
        let (lo, hi) = eta_range(100.0, 0.0, Some(5.0), 6.0);
        assert!(hi < 120.0, "{hi} s for 100 m");
        assert!(lo > 0.0);
    }

    #[test]
    fn far_stops_follow_the_line_not_the_bus() {
        // The bus is racing at 40 km/h but the line is crawling at 10 km/h;
        // eight kilometres out, the line's pace is what will happen.
        let (_, hi_far) = eta_range(8_000.0, 11.0, Some(10.0 / 3.6), 6.0);
        let crawl = 8_000.0 / (10.0 / 3.6);
        assert!(hi_far > crawl, "far estimate ignored the line speed: {hi_far} vs {crawl}");
        // Without enough buses to measure, the timetable is the fallback.
        let (lo_plan, _) = eta_range(3_000.0, 6.0, None, 30.0 / 3.6);
        assert!(lo_plan < 3_000.0 / (30.0 / 3.6));
    }

    #[test]
    fn unknown_services_are_tracked_without_a_direction() {
        let g = gtfs();
        let mut e = Engine::new(g.clone());
        let now = 1_800_000_000;
        let mut f = fix_at(&g, "A9", now, 1000.0, Vendor::Zirix);
        f.service = Some("959".into());
        assert_eq!(e.apply(f, now), Outcome::Accepted);
        let out = e.snapshot();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line, "959");
        assert_eq!(out[0].dir, None);
        assert_eq!(out[0].ph_line(), "pending");
        assert_eq!(e.counters.unmatched_service, 1);
    }
}
