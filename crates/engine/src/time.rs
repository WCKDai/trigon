//! UTC timestamps without a date-time dependency.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now as `YYYY-MM-DDThh:mm:ssZ`.
pub fn now_utc() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    format_utc(secs)
}

/// Days-to-civil conversion (Howard Hinnant's algorithm, public domain).
pub fn format_utc(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn known_instants() {
        assert_eq!(super::format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(super::format_utc(1_793_403_000), "2026-10-30T23:30:00Z");
    }
}
