//! Calendar days as numbers counted from 1970-01-01, enough for licence dates.

pub(crate) fn today() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    secs.div_euclid(86_400)
}

/// `YYYY-MM-DD` to its day number.
pub(crate) fn parse(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = s[0..4].parse().ok()?;
    let m: i64 = s[5..7].parse().ok()?;
    let d: i64 = s[8..10].parse().ok()?;
    let n = from_civil(y, m, d);
    (iso(n) == s).then_some(n)
}

pub(crate) fn iso(n: i64) -> String {
    let (y, m, d) = to_civil(n);
    format!("{y:04}-{m:02}-{d:02}")
}

fn from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn to_civil(n: i64) -> (i64, i64, i64) {
    let z = n + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_round_trip_and_bad_dates_are_refused() {
        assert_eq!(parse("1970-01-01"), Some(0));
        assert_eq!(iso(parse("2028-02-29").unwrap() + 1), "2028-03-01");
        assert_eq!(iso(parse("2026-12-31").unwrap() + 1), "2027-01-01");
        assert_eq!(parse("2026-02-30"), None);
        assert_eq!(parse("2026-1-01"), None);
        assert_eq!(iso(today()).len(), 10);
    }
}
