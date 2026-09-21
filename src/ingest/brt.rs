//! GTFS-Realtime VehiclePositions for the BRT.
//!
//! The feed is a 17 KB protobuf refreshed every 30 s whose `trip_id` values
//! match the published static GTFS exactly, so a hand-written reader for the
//! handful of fields we use avoids a code generator and a `protoc` build
//! dependency. Field numbers come from the GTFS-Realtime specification.

use crate::model::{Fix, Vendor};
use crate::pb::Buf;

#[derive(Default, Debug)]
pub struct RtVehicle {
    pub vehicle_id: Option<String>,
    pub label: Option<String>,
    pub trip_id: Option<String>,
    pub route_id: Option<String>,
    pub direction_id: Option<u8>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub bearing: Option<f32>,
    pub speed: Option<f32>,
    pub timestamp: Option<i64>,
}

fn parse_trip(b: &[u8], v: &mut RtVehicle) {
    let mut p = Buf::new(b);
    while !p.done() {
        let Some((f, w)) = p.tag() else { return };
        match (f, w) {
            (1, 2) => v.trip_id = p.bytes().map(|s| String::from_utf8_lossy(s).into_owned()),
            (5, 2) => v.route_id = p.bytes().map(|s| String::from_utf8_lossy(s).into_owned()),
            (6, 0) => v.direction_id = p.varint().map(|x| x as u8),
            _ => {
                if p.skip(w).is_none() {
                    return;
                }
            }
        }
    }
}

fn parse_position(b: &[u8], v: &mut RtVehicle) {
    let mut p = Buf::new(b);
    while !p.done() {
        let Some((f, w)) = p.tag() else { return };
        match (f, w) {
            (1, 5) => v.lat = p.fixed32().map(|x| f32::from_bits(x) as f64),
            (2, 5) => v.lon = p.fixed32().map(|x| f32::from_bits(x) as f64),
            (3, 5) => v.bearing = p.fixed32().map(f32::from_bits),
            (5, 5) => v.speed = p.fixed32().map(f32::from_bits),
            _ => {
                if p.skip(w).is_none() {
                    return;
                }
            }
        }
    }
}

fn parse_descriptor(b: &[u8], v: &mut RtVehicle) {
    let mut p = Buf::new(b);
    while !p.done() {
        let Some((f, w)) = p.tag() else { return };
        match (f, w) {
            (1, 2) => v.vehicle_id = p.bytes().map(|s| String::from_utf8_lossy(s).into_owned()),
            (2, 2) => v.label = p.bytes().map(|s| String::from_utf8_lossy(s).into_owned()),
            _ => {
                if p.skip(w).is_none() {
                    return;
                }
            }
        }
    }
}

fn parse_vehicle_position(b: &[u8]) -> RtVehicle {
    let mut v = RtVehicle::default();
    let mut p = Buf::new(b);
    while !p.done() {
        let Some((f, w)) = p.tag() else { break };
        match (f, w) {
            (1, 2) => {
                if let Some(s) = p.bytes() {
                    parse_trip(s, &mut v);
                }
            }
            (2, 2) => {
                if let Some(s) = p.bytes() {
                    parse_position(s, &mut v);
                }
            }
            (5, 0) => v.timestamp = p.varint().map(|x| x as i64),
            (8, 2) => {
                if let Some(s) = p.bytes() {
                    parse_descriptor(s, &mut v);
                }
            }
            _ => {
                if p.skip(w).is_none() {
                    break;
                }
            }
        }
    }
    v
}

/// Decodes a `FeedMessage`, returning every entity that carries a vehicle position.
pub fn decode_feed(bytes: &[u8]) -> Vec<RtVehicle> {
    let mut out = Vec::new();
    let mut p = Buf::new(bytes);
    while !p.done() {
        let Some((f, w)) = p.tag() else { break };
        match (f, w) {
            (2, 2) => {
                let Some(entity) = p.bytes() else { break };
                let mut e = Buf::new(entity);
                while !e.done() {
                    let Some((ef, ew)) = e.tag() else { break };
                    match (ef, ew) {
                        (4, 2) => {
                            if let Some(s) = e.bytes() {
                                out.push(parse_vehicle_position(s));
                            }
                        }
                        _ => {
                            if e.skip(ew).is_none() {
                                break;
                            }
                        }
                    }
                }
            }
            _ => {
                if p.skip(w).is_none() {
                    break;
                }
            }
        }
    }
    out
}

/// Turns decoded entities into fixes, dropping anything without a usable position.
pub fn to_fixes(vs: Vec<RtVehicle>, fallback_t: i64) -> Vec<Fix> {
    let mut out = Vec::with_capacity(vs.len());
    for v in vs {
        let (Some(lat), Some(lon)) = (v.lat, v.lon) else {
            continue;
        };
        let id = v
            .vehicle_id
            .clone()
            .or_else(|| v.label.clone())
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let mut f = Fix::new(id, Vendor::BrtRt, v.timestamp.unwrap_or(fallback_t), lat, lon);
        f.heading = v.bearing.filter(|b| b.is_finite() && *b > 0.0);
        f.trip_id = v.trip_id;
        f.brt = true;
        f.dir = v.direction_id.and_then(|d| match d {
            0 => Some(crate::model::Dir::Ida),
            1 => Some(crate::model::Dir::Volta),
            _ => None,
        });
        out.push(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn tag(field: u32, wire: u8, out: &mut Vec<u8>) {
        varint(((field as u64) << 3) | wire as u64, out);
    }

    fn len_delimited(field: u32, body: &[u8], out: &mut Vec<u8>) {
        tag(field, 2, out);
        varint(body.len() as u64, out);
        out.extend_from_slice(body);
    }

    /// Builds a FeedMessage with one vehicle, mirroring the real feed's shape.
    fn sample_feed() -> Vec<u8> {
        let mut trip = Vec::new();
        len_delimited(1, b"7d6fcf5d-f2fb-48db-8320-e31150beed75", &mut trip);
        len_delimited(5, b"20000221110", &mut trip);
        tag(6, 0, &mut trip);
        varint(1, &mut trip);

        let mut pos = Vec::new();
        tag(1, 5, &mut pos);
        pos.extend_from_slice(&(-22.9711f32).to_bits().to_le_bytes());
        tag(2, 5, &mut pos);
        pos.extend_from_slice(&(-43.1822f32).to_bits().to_le_bytes());
        tag(3, 5, &mut pos);
        pos.extend_from_slice(&137.5f32.to_bits().to_le_bytes());

        let mut desc = Vec::new();
        len_delimited(1, b"901008", &mut desc);
        len_delimited(2, b"RJN9A01", &mut desc);

        let mut vp = Vec::new();
        len_delimited(1, &trip, &mut vp);
        len_delimited(2, &pos, &mut vp);
        tag(5, 0, &mut vp);
        varint(1_789_903_013, &mut vp);
        // An unknown field in the middle must be skipped, not abort the parse.
        tag(3, 0, &mut vp);
        varint(7, &mut vp);
        len_delimited(8, &desc, &mut vp);

        let mut entity = Vec::new();
        len_delimited(1, b"901008", &mut entity);
        len_delimited(4, &vp, &mut entity);

        let mut header = Vec::new();
        len_delimited(1, b"2.0", &mut header);

        let mut feed = Vec::new();
        len_delimited(1, &header, &mut feed);
        len_delimited(2, &entity, &mut feed);
        feed
    }

    #[test]
    fn decodes_a_vehicle_position() {
        let v = decode_feed(&sample_feed());
        assert_eq!(v.len(), 1);
        let v = &v[0];
        assert_eq!(v.vehicle_id.as_deref(), Some("901008"));
        assert_eq!(v.label.as_deref(), Some("RJN9A01"));
        assert_eq!(
            v.trip_id.as_deref(),
            Some("7d6fcf5d-f2fb-48db-8320-e31150beed75")
        );
        assert_eq!(v.route_id.as_deref(), Some("20000221110"));
        assert_eq!(v.direction_id, Some(1));
        assert!((v.lat.unwrap() + 22.9711).abs() < 1e-4);
        assert!((v.lon.unwrap() + 43.1822).abs() < 1e-4);
        assert!((v.bearing.unwrap() - 137.5).abs() < 1e-3);
        assert_eq!(v.timestamp, Some(1_789_903_013));
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let full = sample_feed();
        for cut in 0..full.len() {
            let _ = decode_feed(&full[..cut]);
        }
    }

    #[test]
    fn converts_to_fixes() {
        let fixes = to_fixes(decode_feed(&sample_feed()), 0);
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].vendor, Vendor::BrtRt);
        assert!(fixes[0].brt);
        assert_eq!(fixes[0].dir, Some(crate::model::Dir::Volta));
        assert_eq!(fixes[0].t, 1_789_903_013);
    }
}
