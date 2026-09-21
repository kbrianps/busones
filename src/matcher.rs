//! Map matching: where a vehicle is along which shape, and in which direction.
//!
//! Position alone cannot answer that in Rio. 311 of the 441 two-way routes run
//! their outbound and inbound shapes within 30 m of each other over more than
//! half their length, 120 shapes double back on the same street, and 41 routes
//! are closed loops. So a hypothesis here is a *pass*: a shape plus one of the
//! places along it that is near the vehicle. Hypotheses compete over several
//! fixes, and a direction is only published once one of them has shown real
//! forward progress.

use crate::geo;
use crate::gtfs::{Gtfs, Shape};

/// A projection this close to the shape is taken at face value.
pub const CONFIDENT_M: f32 = 60.0;
/// Beyond this the vehicle is not considered to be on the shape at all.
pub const TENTATIVE_M: f32 = 200.0;
/// GPS noise below this is not treated as going backwards.
pub const BACKWARD_TOLERANCE_M: f32 = 30.0;
/// Net advance required before a direction is published.
pub const PROGRESS_TO_CONFIRM_M: f32 = 100.0;
/// Transitions, so four fixes: about 90 s at the 20 to 30 s vendor cadence.
pub const FIXES_TO_CONFIRM: u8 = 3;
/// Two hypotheses this far apart along a shape are different passes.
pub const PASS_SEPARATION_M: f32 = 300.0;
pub const BEAM_WIDTH: usize = 4;
/// Cost a rival hypothesis must be behind by before we commit.
pub const COMMIT_MARGIN: f32 = 15.0;

/// Hypothesis cost is an exponentially weighted *average* of the per-fix cost,
/// not a running sum. A sum converges to `step / (1 - DECAY)`, which for any
/// realistic GPS offset overtakes the cost of starting fresh: the tracked
/// hypothesis then loses to a newly spawned one and its accumulated progress
/// is thrown away, so a moving bus never confirms a direction. Averaging keeps
/// the two comparable, and a hypothesis is only displaced by a fresh one when
/// it has been worse by `SPAWN_PENALTY` per fix.
const DECAY: f32 = 0.7;
const SPAWN_PENALTY: f32 = 60.0;
const MOTION_WEIGHT: f32 = 0.5;
/// Two units on the same bus disagree by up to 80 m, and a straight line
/// between fixes is shorter than the road. Only a gap wider than this counts.
const MOTION_SLACK_M: f32 = 25.0;
const BACKWARD_PENALTY: f32 = 150.0;
const HEADING_WEIGHT: f32 = 0.35;
const MAX_CANDIDATES_PER_SHAPE: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct Cand {
    pub along: f32,
    pub dist: f32,
}

/// Signed distance from `from` to `to` along a shape, taking the short way
/// round on a closed loop.
pub fn along_delta(sh: &Shape, from: f32, to: f32) -> f32 {
    let d = to - from;
    if !sh.circular {
        return d;
    }
    let l = sh.length();
    if l <= 1.0 {
        return d;
    }
    let mut d = d % l;
    if d > l * 0.5 {
        d -= l;
    } else if d < -l * 0.5 {
        d += l;
    }
    d
}

/// Every distinct place along `sh` within `radius` of the point.
///
/// A pass is a contiguous run of segments near the point, so runs are what we
/// split on: the outbound and inbound halves of a street touch different parts
/// of the segment list even when they are metres apart on the ground.
pub fn pass_candidates(sh: &Shape, px: f32, py: f32, radius: f32) -> Vec<Cand> {
    let n = sh.x.len();
    if n < 2 {
        return Vec::new();
    }
    if px < sh.bbox[0] - radius
        || px > sh.bbox[2] + radius
        || py < sh.bbox[1] - radius
        || py > sh.bbox[3] + radius
    {
        return Vec::new();
    }
    // (first segment, last segment, best along, best distance)
    let mut runs: Vec<(usize, usize, f32, f32)> = Vec::new();
    for i in 0..n - 1 {
        let (t, d) = geo::project_on_segment(px, py, sh.x[i], sh.y[i], sh.x[i + 1], sh.y[i + 1]);
        if d > radius {
            continue;
        }
        let along = sh.cum[i] + t * (sh.cum[i + 1] - sh.cum[i]);
        match runs.last_mut() {
            Some(r) if r.1 + 1 == i => {
                r.1 = i;
                if d < r.3 {
                    r.2 = along;
                    r.3 = d;
                }
            }
            _ => runs.push((i, i, along, d)),
        }
    }
    if sh.circular && runs.len() >= 2 {
        let wraps = runs[0].0 == 0 && runs[runs.len() - 1].1 == n - 2;
        if wraps {
            let last = runs.pop().unwrap();
            if last.3 < runs[0].3 {
                runs[0].2 = last.2;
                runs[0].3 = last.3;
            }
        }
    }
    let mut out: Vec<Cand> = runs
        .into_iter()
        .map(|r| Cand {
            along: r.2,
            dist: r.3,
        })
        .collect();
    out.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(MAX_CANDIDATES_PER_SHAPE);
    out
}

#[derive(Clone, Copy, Debug)]
pub struct Hyp {
    pub shape: u32,
    pub along: f32,
    pub lap: u16,
    pub cost: f32,
    /// Distance from the last fix to the shape, in metres.
    pub dist: f32,
    /// Net metres advanced since the last backwards step.
    pub progress: f32,
    /// Transitions that contributed to `progress`.
    pub fixes: u8,
}

pub struct Step<'a> {
    pub gtfs: &'a Gtfs,
    /// Shapes worth considering for this fix, already filtered by service and direction.
    pub shapes: &'a [u32],
    pub px: f32,
    pub py: f32,
    /// Straight-line metres since the previous fix.
    pub travelled: f32,
    /// False when the previous fix is too close in time (or from the other
    /// on-board unit) for the motion model to mean anything.
    pub usable_motion: bool,
    pub heading: Option<f32>,
    pub moving: bool,
}

/// Advances the beam with one fix.
pub fn advance(beam: &mut Vec<Hyp>, s: &Step) {
    let mut next: Vec<Hyp> = Vec::with_capacity(BEAM_WIDTH * 2);
    for &si in s.shapes {
        let sh = s.gtfs.shape(si);
        for c in pass_candidates(sh, s.px, s.py, TENTATIVE_M) {
            let heading_cost = match (s.heading, s.moving) {
                (Some(h), true) => geo::bearing_diff(h, sh.bearing_at(c.along)) * HEADING_WEIGHT,
                _ => 0.0,
            };
            let base = c.dist + heading_cost;

            let mut best: Option<Hyp> = None;
            for old in beam.iter().filter(|h| h.shape == si) {
                let delta = along_delta(sh, old.along, c.along);
                let motion = if s.usable_motion {
                    ((delta - s.travelled).abs() - MOTION_SLACK_M).max(0.0) * MOTION_WEIGHT
                        + if delta < -BACKWARD_TOLERANCE_M {
                            BACKWARD_PENALTY
                        } else {
                            0.0
                        }
                } else {
                    0.0
                };
                let cost = DECAY * old.cost + (1.0 - DECAY) * (base + motion);
                if best.is_none_or(|b| cost < b.cost) {
                    let (progress, fixes) = if !s.usable_motion {
                        (old.progress, old.fixes)
                    } else if delta < -BACKWARD_TOLERANCE_M {
                        (0.0, 0)
                    } else {
                        (old.progress + delta.max(0.0), old.fixes.saturating_add(1))
                    };
                    let wrapped = sh.circular && c.along < old.along && delta > 0.0;
                    best = Some(Hyp {
                        shape: si,
                        along: c.along,
                        lap: old.lap + u16::from(wrapped),
                        cost,
                        dist: c.dist,
                        progress,
                        fixes,
                    });
                }
            }

            let spawn = Hyp {
                shape: si,
                along: c.along,
                lap: 0,
                cost: base + SPAWN_PENALTY,
                dist: c.dist,
                progress: 0.0,
                fixes: 0,
            };
            next.push(match best {
                Some(b) if b.cost <= spawn.cost => b,
                Some(_) | None => spawn,
            });
        }
    }

    if next.is_empty() {
        // Nothing projected this time: a noisy fix, a detour, or a bus pulling
        // into a yard. Keeping the hypotheses costs nothing and protects
        // minutes of accumulated evidence from a single bad reading; the
        // recorded miss is what the off-route timer and the confidence score
        // react to.
        for h in beam.iter_mut() {
            h.dist = f32::MAX;
        }
        return;
    }

    // Collapse hypotheses that landed on the same pass.
    next.sort_by(|a, b| {
        a.shape
            .cmp(&b.shape)
            .then(a.cost.partial_cmp(&b.cost).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut kept: Vec<Hyp> = Vec::with_capacity(next.len());
    for h in next {
        if kept
            .iter()
            .any(|k| k.shape == h.shape && (k.along - h.along).abs() < PASS_SEPARATION_M)
        {
            continue;
        }
        kept.push(h);
    }
    kept.sort_by(|a, b| a.cost.partial_cmp(&b.cost).unwrap_or(std::cmp::Ordering::Equal));
    kept.truncate(BEAM_WIDTH);
    *beam = kept;
}

/// The hypothesis worth publishing a direction for, if any.
pub fn committed(beam: &[Hyp]) -> Option<Hyp> {
    let best = *beam.first()?;
    if best.progress < PROGRESS_TO_CONFIRM_M
        || best.fixes < FIXES_TO_CONFIRM
        || best.dist > TENTATIVE_M
    {
        return None;
    }
    let rival = beam.iter().skip(1).find(|h| {
        h.shape != best.shape || (h.along - best.along).abs() > PASS_SEPARATION_M
    });
    match rival {
        Some(r) if r.cost < best.cost + COMMIT_MARGIN => None,
        _ => Some(best),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gtfs::{Route, Shape, Stop};

    /// A shape that runs 2 km east, turns round and comes back on the same
    /// street 20 m to the side: the geometry that breaks naive snapping.
    fn out_and_back() -> Gtfs {
        let mut lat = Vec::new();
        let mut lon = Vec::new();
        let mut cum = Vec::new();
        let m_lon = 1.0 / (6_371_000.0f64 * std::f64::consts::PI / 180.0 * (-22.9f64).to_radians().cos());
        let m_lat = 1.0 / (6_371_000.0f64 * std::f64::consts::PI / 180.0);
        let push = |x_m: f64, y_m: f64, d: f32, lat: &mut Vec<i32>, lon: &mut Vec<i32>, cum: &mut Vec<f32>| {
            lat.push(((-22.9 + y_m * m_lat) * 1e6).round() as i32);
            lon.push(((-43.4 + x_m * m_lon) * 1e6).round() as i32);
            cum.push(d);
        };
        for i in 0..=40 {
            let x = i as f64 * 50.0;
            push(x, 0.0, (i * 50) as f32, &mut lat, &mut lon, &mut cum);
        }
        for i in 0..=40 {
            let x = 2000.0 - i as f64 * 50.0;
            push(x, 20.0, 2000.0 + (i * 50) as f32, &mut lat, &mut lon, &mut cum);
        }
        let mut g = Gtfs {
            routes: vec![Route {
                id: "R".into(),
                short_name: "999".into(),
                long_name: "Vai e volta".into(),
                route_type: 700,
                shapes: vec![0],
            }],
            shapes: vec![Shape {
                id: "sh".into(),
                route: 0,
                direction: 0,
                headsign: "Fim".into(),
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
            }],
            stops: Vec::<Stop>::new(),
            ..Default::default()
        };
        g.finish();
        g
    }

    fn at(g: &Gtfs, along: f32) -> (f32, f32) {
        g.shape(0).point_at(along)
    }

    #[test]
    fn finds_both_passes_of_a_doubled_back_street() {
        let g = out_and_back();
        let sh = g.shape(0);
        let (x, y) = sh.point_at(1000.0);
        let c = pass_candidates(sh, x, y, TENTATIVE_M);
        assert_eq!(c.len(), 2, "outbound and inbound pass: {c:?}");
        let mut alongs: Vec<f32> = c.iter().map(|c| c.along).collect();
        alongs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!((alongs[0] - 1000.0).abs() < 30.0, "{alongs:?}");
        assert!((alongs[1] - 3000.0).abs() < 30.0, "{alongs:?}");
    }

    #[test]
    fn the_radius_decides_how_many_passes_are_in_play() {
        let g = out_and_back();
        let sh = g.shape(0);
        let (x, y) = sh.point_at(1000.0);
        // The return leg runs 20 m to the side, so a tight radius sees one pass.
        assert_eq!(pass_candidates(sh, x, y, 10.0).len(), 1);
        assert_eq!(pass_candidates(sh, x, y, 60.0).len(), 2);
        // Far from the shape, nothing matches.
        assert!(pass_candidates(sh, x + 5_000.0, y, 60.0).is_empty());
    }

    fn run(g: &Gtfs, alongs: &[f32], travelled: f32) -> Vec<Hyp> {
        let mut beam = Vec::new();
        for &a in alongs {
            let (px, py) = at(g, a);
            advance(
                &mut beam,
                &Step {
                    gtfs: g,
                    shapes: &[0],
                    px,
                    py,
                    travelled,
                    usable_motion: true,
                    heading: None,
                    moving: true,
                },
            );
        }
        beam
    }

    /// The regression that kept moving buses pending: with noisy fixes the
    /// tracked hypothesis must stay cheaper than starting over, so progress
    /// survives fix after fix.
    #[test]
    fn noisy_fixes_do_not_restart_the_hypothesis() {
        let g = out_and_back();
        let sh = g.shape(0);
        let mut beam = Vec::new();
        let mut progress_seen = Vec::new();
        for k in 0..12 {
            let along = 200.0 + k as f32 * 150.0;
            let (px, py) = sh.point_at(along.min(1900.0));
            // Ten metres off the centre line, alternating sides, plus a heading.
            let jitter = if k % 2 == 0 { 10.0 } else { -12.0 };
            advance(
                &mut beam,
                &Step {
                    gtfs: &g,
                    shapes: &[0],
                    px,
                    py: py + jitter,
                    travelled: 150.0,
                    usable_motion: true,
                    heading: Some(90.0),
                    moving: true,
                },
            );
            progress_seen.push(beam[0].progress);
        }
        assert!(
            progress_seen[11] > 1_000.0,
            "progress was reset along the way: {progress_seen:?}"
        );
        assert!(committed(&beam).is_some(), "{beam:?}");
    }

    #[test]
    fn one_bad_fix_does_not_erase_the_hypothesis() {
        let g = out_and_back();
        let sh = g.shape(0);
        let mut beam = Vec::new();
        for a in [200.0f32, 350.0, 500.0, 650.0] {
            let (px, py) = sh.point_at(a);
            advance(&mut beam, &Step { gtfs: &g, shapes: &[0], px, py, travelled: 150.0, usable_motion: true, heading: None, moving: true });
        }
        let progress = beam[0].progress;
        assert!(committed(&beam).is_some());
        // A fix a kilometre off the route.
        let (px, py) = sh.point_at(800.0);
        advance(&mut beam, &Step { gtfs: &g, shapes: &[0], px, py: py + 1_000.0, travelled: 150.0, usable_motion: true, heading: None, moving: true });
        assert!(!beam.is_empty(), "the beam survived");
        assert_eq!(beam[0].progress, progress, "progress kept");
        assert_eq!(beam[0].dist, f32::MAX, "the miss is recorded");
        assert!(committed(&beam).is_none(), "but nothing is published from it");
        // Back on the route, it picks up where it left off.
        let (px, py) = sh.point_at(800.0);
        advance(&mut beam, &Step { gtfs: &g, shapes: &[0], px, py, travelled: 150.0, usable_motion: true, heading: None, moving: true });
        assert!(committed(&beam).is_some(), "{beam:?}");
    }

    #[test]
    fn commits_after_sustained_forward_progress() {
        let g = out_and_back();
        // Four fixes, 150 m apart, all on the outbound pass.
        let beam = run(&g, &[200.0, 350.0, 500.0, 650.0], 150.0);
        let c = committed(&beam).expect("should commit");
        assert_eq!(c.shape, 0);
        assert!((c.along - 650.0).abs() < 40.0, "{c:?}");
        assert!(c.progress >= PROGRESS_TO_CONFIRM_M);
    }

    #[test]
    fn refuses_to_commit_on_the_first_fixes() {
        let g = out_and_back();
        assert!(committed(&run(&g, &[200.0], 0.0)).is_none());
        assert!(committed(&run(&g, &[200.0, 260.0], 60.0)).is_none());
    }

    #[test]
    fn standing_still_never_commits() {
        let g = out_and_back();
        // Ten fixes at the same place: plenty of fixes, no progress.
        let beam = run(&g, &[500.0; 10], 0.0);
        assert!(committed(&beam).is_none());
        assert!(beam[0].progress < PROGRESS_TO_CONFIRM_M);
    }

    #[test]
    fn going_backwards_resets_progress() {
        let g = out_and_back();
        let mut beam = Vec::new();
        for a in [200.0f32, 350.0, 500.0] {
            let (px, py) = at(&g, a);
            advance(&mut beam, &Step { gtfs: &g, shapes: &[0], px, py, travelled: 150.0, usable_motion: true, heading: None, moving: true });
        }
        assert!(beam[0].progress >= 300.0);
        // A fix 200 m behind on the same pass wipes the accumulated progress.
        let (px, py) = at(&g, 300.0);
        advance(&mut beam, &Step { gtfs: &g, shapes: &[0], px, py, travelled: 200.0, usable_motion: true, heading: None, moving: true });
        let same_pass = beam.iter().find(|h| (h.along - 300.0).abs() < 60.0).unwrap();
        assert_eq!(same_pass.progress, 0.0, "{beam:?}");
    }

    #[test]
    fn heading_separates_the_two_passes() {
        let g = out_and_back();
        let sh = g.shape(0);
        let mut beam = Vec::new();
        // Four fixes heading west, which is the inbound pass (along 2000+).
        for a in [3000.0f32, 3150.0, 3300.0, 3450.0] {
            let (px, py) = sh.point_at(a);
            advance(
                &mut beam,
                &Step {
                    gtfs: &g,
                    shapes: &[0],
                    px,
                    py,
                    travelled: 150.0,
                    usable_motion: true,
                    heading: Some(270.0),
                    moving: true,
                },
            );
        }
        let c = committed(&beam).expect("heading should disambiguate");
        assert!(c.along > 2000.0, "picked the outbound pass: {c:?}");
    }

    #[test]
    fn unusable_motion_does_not_accrue_progress() {
        let g = out_and_back();
        let mut beam = Vec::new();
        for a in [200.0f32, 350.0, 500.0, 650.0] {
            let (px, py) = at(&g, a);
            advance(&mut beam, &Step { gtfs: &g, shapes: &[0], px, py, travelled: 150.0, usable_motion: false, heading: None, moving: true });
        }
        assert!(committed(&beam).is_none(), "{beam:?}");
    }

    #[test]
    fn circular_delta_takes_the_short_way() {
        let mut g = out_and_back();
        g.shapes[0].circular = true;
        let sh = g.shape(0);
        let l = sh.length();
        assert!((along_delta(sh, l - 50.0, 50.0) - 100.0).abs() < 1.0);
        assert!((along_delta(sh, 50.0, l - 50.0) + 100.0).abs() < 1.0);
        assert!((along_delta(sh, 100.0, 300.0) - 200.0).abs() < 1.0);
    }
}
