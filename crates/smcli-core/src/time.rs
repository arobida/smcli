//! ISO 8601 in UTC, as Foundation's `ISO8601DateFormatter` writes it (`2023-11-14T22:13:20Z`).

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

// Howard Hinnant's days_from_civil / civil_from_days.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Whole seconds, like Foundation's default options; `fractional` adds milliseconds.
pub fn format_iso8601(epoch: f64, fractional: bool) -> String {
    let millis = (epoch * 1000.0).floor() as i64;
    let secs = millis.div_euclid(1000);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    let base = format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60);
    if fractional { format!("{base}.{:03}Z", millis.rem_euclid(1000)) } else { format!("{base}Z") }
}

fn days_in_month(y: i64, mo: i64) -> i64 {
    match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.fff](Z|±HH:MM)`.
pub fn parse_iso8601(text: &str) -> Option<f64> {
    if !text.is_ascii() {
        return None;
    }
    let b = text.as_bytes();
    // ASCII digits only — `str::parse` would accept a leading sign (`+-1:00`).
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = text.get(r)?;
        if s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, s) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut fraction = 0.0;
    if let Some(after) = rest.strip_prefix('.') {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        fraction = format!("0.{}", &after[..digits]).parse().ok()?;
        rest = &after[digits..];
    }
    let offset = match rest {
        "Z" => 0,
        _ if rest.len() == 6 && (rest.starts_with('+') || rest.starts_with('-')) && rest.as_bytes()[3] == b':' => {
            let digits2 = |s: &str| -> Option<i64> {
                if s.len() == 2 && s.bytes().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None }
            };
            let (oh, om) = (digits2(&rest[1..3])?, digits2(&rest[4..6])?);
            if oh > 23 || om > 59 {
                return None;
            }
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            sign * (oh * 3600 + om * 60)
        }
        _ => return None,
    };
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset) as f64 + fraction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_iso8601() {
        assert_eq!(format_iso8601(1_700_000_000.0, false), "2023-11-14T22:13:20Z");
        assert_eq!(format_iso8601(1_700_000_000.25, true), "2023-11-14T22:13:20.250Z");
        assert_eq!(format_iso8601(0.0, false), "1970-01-01T00:00:00Z");
        assert_eq!(parse_iso8601("2023-11-14T22:13:20Z"), Some(1_700_000_000.0));
        assert_eq!(parse_iso8601("2023-11-14T23:13:20.5+01:00"), Some(1_700_000_000.5));
        assert_eq!(parse_iso8601("2024-02-29T00:00:00Z"), Some(1_709_164_800.0));
        assert_eq!(parse_iso8601("2023-11-14"), None);
    }

    #[test]
    fn parse_iso8601_rejects_impossible_values() {
        assert_eq!(parse_iso8601("2023-02-31T00:00:00Z"), None);
        assert_eq!(parse_iso8601("2023-11-14T22:13:20+99:99"), None);
        assert_eq!(parse_iso8601("2026-01-01T00:00:00+1é00"), None);
        assert_eq!(parse_iso8601("2023-11-14T22:13:20+-1:00"), None);
    }
}
