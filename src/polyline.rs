//! Google's encoded polyline algorithm, used for the static client bundles.

pub fn encode(points: &[(f64, f64)], precision: u32) -> String {
    let factor = 10f64.powi(precision as i32);
    let mut out = String::with_capacity(points.len() * 6);
    let (mut plat, mut plon) = (0i64, 0i64);
    for &(lat, lon) in points {
        let ilat = (lat * factor).round() as i64;
        let ilon = (lon * factor).round() as i64;
        encode_value(ilat - plat, &mut out);
        encode_value(ilon - plon, &mut out);
        plat = ilat;
        plon = ilon;
    }
    out
}

fn encode_value(v: i64, out: &mut String) {
    let mut v = if v < 0 { !(v << 1) } else { v << 1 };
    while v >= 0x20 {
        out.push((((0x20 | (v & 0x1f)) + 63) as u8) as char);
        v >>= 5;
    }
    out.push(((v + 63) as u8) as char);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference_vector() {
        // The canonical example from Google's documentation.
        let pts = [(38.5, -120.2), (40.7, -120.95), (43.252, -126.453)];
        assert_eq!(encode(&pts, 5), "_p~iF~ps|U_ulLnnqC_mqNvxq`@");
    }
}
