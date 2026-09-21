//! Minimal UTC time handling. No external dependency: the feeds use a fixed
//! ISO 8601 shape and Brazil has had no daylight saving time since 2019, so a
//! civil-calendar conversion is all we need.

/// Days from 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date from days since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn digits(b: &[u8], from: usize, len: usize) -> Option<i64> {
    if b.len() < from + len {
        return None;
    }
    let mut v: i64 = 0;
    for &c in &b[from..from + len] {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as i64;
    }
    Some(v)
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff][Z|+HH:MM|-HH:MM]` into a unix timestamp.
/// A missing zone is treated as UTC, which is what every SMTR feed we consume
/// claims to send. Fractional seconds are ignored.
pub fn parse_iso8601(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let y = digits(b, 0, 4)?;
    let mo = digits(b, 5, 2)?;
    let d = digits(b, 8, 2)?;
    let h = digits(b, 11, 2)?;
    let mi = digits(b, 14, 2)?;
    let sec = digits(b, 17, 2)?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut ts = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec;

    // Skip fractional seconds, then look for an offset.
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    match b.get(i) {
        None | Some(b'Z') | Some(b'z') => {}
        Some(&sign @ (b'+' | b'-')) => {
            let oh = digits(b, i + 1, 2)?;
            let om = if b.get(i + 3) == Some(&b':') {
                digits(b, i + 4, 2)?
            } else {
                digits(b, i + 3, 2).unwrap_or(0)
            };
            let off = oh * 3600 + om * 60;
            ts += if sign == b'+' { -off } else { off };
        }
        _ => return None,
    }
    Some(ts)
}

/// `YYYY-MM-DDTHH:MM:00Z` for the UTC minute containing `ts`.
pub fn format_minute_utc(ts: i64) -> String {
    let ts = ts - ts.rem_euclid(60);
    let days = ts.div_euclid(86_400);
    let rem = ts.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:00Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60
    )
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_roundtrip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for ts in [0i64, 1_000_000_000, 1_789_902_038, 2_000_000_000] {
            let days = ts.div_euclid(86_400);
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "ts {ts}");
        }
    }

    #[test]
    fn parses_feed_timestamps() {
        // Shape used by the ITS aggregator and the per-vendor endpoints.
        assert_eq!(parse_iso8601("1970-01-01T00:00:00Z"), Some(0));
        let t = parse_iso8601("2026-09-20T11:35:07Z").unwrap();
        assert_eq!(format_minute_utc(t), "2026-09-20T11:35:00Z");
        // Fractional seconds and explicit offsets.
        assert_eq!(
            parse_iso8601("2026-09-20T11:35:07.123456Z"),
            parse_iso8601("2026-09-20T11:35:07Z")
        );
        assert_eq!(
            parse_iso8601("2026-09-20T08:35:07-03:00"),
            parse_iso8601("2026-09-20T11:35:07Z")
        );
        assert_eq!(
            parse_iso8601("2026-09-20T11:35:07"),
            parse_iso8601("2026-09-20T11:35:07Z")
        );
        assert_eq!(parse_iso8601("garbage"), None);
        assert_eq!(parse_iso8601(""), None);
    }

    #[test]
    fn minute_formatting_truncates() {
        let t = parse_iso8601("2026-09-20T11:35:59Z").unwrap();
        assert_eq!(format_minute_utc(t), "2026-09-20T11:35:00Z");
        assert_eq!(format_minute_utc(t - 60), "2026-09-20T11:34:00Z");
    }
}
