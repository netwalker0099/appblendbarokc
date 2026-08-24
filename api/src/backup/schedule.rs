//! Turning a cron expression plus an IANA timezone into "when does this next
//! run".
//!
//! Two decisions are baked in here.
//!
//! **Five fields, not six.** The `cron` crate wants a leading seconds field.
//! Cron expressions everywhere else in the world — crontab, Kubernetes, every
//! tutorial someone will paste from — have five. Accepting six would mean
//! `0 2 * * *` is read as "every second of 02:00 on the 2nd", which is a
//! catastrophe dressed as a typo: an hourly-looking schedule that dumps the
//! database sixty times a minute. So exactly five fields are accepted and the
//! seconds are pinned to `0` here.
//!
//! **The zone is resolved per run.** Storing "2am Chicago" as a UTC instant
//! works until March, when it becomes 1am or 3am and stays wrong for half the
//! year. The expression is evaluated in local time on every scheduling decision,
//! so 2am means 2am.

use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use cron::Schedule;

/// Parse a 5-field cron expression, rejecting anything else.
pub fn parse(expr: &str) -> Result<Schedule, String> {
    let trimmed = expr.trim();
    let fields = trimmed.split_whitespace().count();
    if fields != 5 {
        return Err(format!(
            "a schedule needs exactly 5 fields (minute hour day-of-month month day-of-week) \
             — this has {fields}"
        ));
    }
    // Seconds pinned to 0: the expression selects a minute, and the job runs
    // once at the top of it.
    Schedule::from_str(&format!("0 {trimmed}"))
        .map_err(|e| format!("that isn't a valid cron expression: {e}"))
}

pub fn parse_tz(name: &str) -> Result<Tz, String> {
    name.parse::<Tz>()
        .map_err(|_| format!("unknown timezone '{name}' — use an IANA name like America/Chicago"))
}

/// The first firing strictly after `after`.
///
/// `None` when the expression can never fire again (`0 0 30 2 *` — the 30th of
/// February). That is a real possibility with hand-written cron, and it must not
/// be reported as success: a destination that can never run is broken, and the
/// caller stores the reason rather than leaving a row that looks scheduled.
pub fn next_after(expr: &str, timezone: &str, after: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let schedule = parse(expr)?;
    let tz = parse_tz(timezone)?;
    let local_after = tz.from_utc_datetime(&after.naive_utc());
    schedule
        .after(&local_after)
        .next()
        .map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| {
            format!("'{expr}' has no next run — check the day-of-month and month fields")
        })
}

/// Plain-English rendering for the admin list, so nobody has to read cron to
/// know whether their database is being backed up hourly or yearly.
///
/// Deliberately partial: it recognises the shapes the UI's presets generate and
/// falls back to showing the raw expression. A wrong guess in this string would
/// be worse than no guess, because it is the only thing most people will read.
pub fn describe(expr: &str, timezone: &str) -> String {
    let f: Vec<&str> = expr.trim().split_whitespace().collect();
    if f.len() != 5 {
        return expr.to_string();
    }
    let (min, hour, dom, mon, dow) = (f[0], f[1], f[2], f[3], f[4]);

    let at = |h: &str, m: &str| -> String {
        match (h.parse::<u32>(), m.parse::<u32>()) {
            (Ok(h), Ok(m)) => {
                let suffix = if h < 12 { "am" } else { "pm" };
                let display = match h % 12 {
                    0 => 12,
                    other => other,
                };
                format!("{display}:{m:02}{suffix}")
            }
            _ => format!("{h}:{m}"),
        }
    };

    if dom != "*" || mon != "*" {
        return format!("{expr} ({timezone})");
    }

    let day = match dow {
        "*" => None,
        "0" | "7" | "sun" => Some("Sunday"),
        "1" | "mon" => Some("Monday"),
        "2" | "tue" => Some("Tuesday"),
        "3" | "wed" => Some("Wednesday"),
        "4" | "thu" => Some("Thursday"),
        "5" | "fri" => Some("Friday"),
        "6" | "sat" => Some("Saturday"),
        _ => return format!("{expr} ({timezone})"),
    };

    match (min, hour, day) {
        // Every hour, on the given minute.
        (m, "*", None) if m.parse::<u32>().is_ok() => {
            let m: u32 = m.parse().unwrap();
            if m == 0 {
                "Hourly, on the hour".to_string()
            } else {
                format!("Hourly, at {m} past")
            }
        }
        // Every N hours.
        (m, h, None) if h.starts_with("*/") && m.parse::<u32>().is_ok() => {
            match h.trim_start_matches("*/").parse::<u32>() {
                Ok(n) => format!("Every {n} hours ({timezone})"),
                Err(_) => format!("{expr} ({timezone})"),
            }
        }
        (m, h, None) if m.parse::<u32>().is_ok() && h.parse::<u32>().is_ok() => {
            format!("Daily at {} ({timezone})", at(h, m))
        }
        (m, h, Some(d)) if m.parse::<u32>().is_ok() && h.parse::<u32>().is_ok() => {
            format!("Every {d} at {} ({timezone})", at(h, m))
        }
        _ => format!("{expr} ({timezone})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn five_fields_are_required() {
        assert!(parse("0 2 * * *").is_ok());
        // Six fields is the dangerous case: accepted by the underlying crate,
        // and means something wildly different from what the user typed.
        assert!(parse("0 0 2 * * *").is_err());
        assert!(parse("0 2 * *").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn garbage_is_rejected_rather_than_defaulted() {
        assert!(parse("banana 2 * * *").is_err());
        assert!(parse("99 2 * * *").is_err());
    }

    #[test]
    fn a_daily_time_keeps_its_local_hour_across_the_dst_change() {
        // A schedule stored as a UTC offset would drift by an hour when the
        // clocks move. Resolving the zone per run must not: 3:30am stays 3:30am
        // and the UTC instant is what changes.
        let winter = Utc.with_ymd_and_hms(2026, 3, 6, 12, 0, 0).unwrap();
        let next = next_after("30 3 * * *", "America/Chicago", winter).unwrap();
        // 3:30am CST (UTC-6) on the 7th.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 7, 9, 30, 0).unwrap());

        let summer = Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0).unwrap();
        let next = next_after("30 3 * * *", "America/Chicago", summer).unwrap();
        // 3:30am CDT (UTC-5) on the 10th. Different instant, same local time.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 10, 8, 30, 0).unwrap());
    }

    #[test]
    fn a_job_scheduled_in_the_hour_dst_skips_does_not_run_that_day() {
        // Documenting a real gap rather than hiding it. On 2026-03-08 the US
        // clocks go 01:59:59 -> 03:00:00, so 02:00 never happens and a 2am job
        // is skipped for that one day — standard cron behaviour everywhere, but
        // surprising if you have not met it.
        //
        // This is why the UI's daily preset defaults to 3:30am: it exists on the
        // spring-forward day, and unlike 1:30am it does not occur twice on the
        // autumn one.
        let before = Utc.with_ymd_and_hms(2026, 3, 7, 12, 0, 0).unwrap();
        let next = next_after("0 2 * * *", "America/Chicago", before).unwrap();
        assert_eq!(
            next,
            Utc.with_ymd_and_hms(2026, 3, 9, 7, 0, 0).unwrap(),
            "expected the 8th to be skipped and the next run to be 2am on the 9th"
        );
    }

    #[test]
    fn hourly_advances_by_an_hour() {
        let now = Utc.with_ymd_and_hms(2026, 8, 24, 14, 30, 0).unwrap();
        let next = next_after("0 * * * *", "America/Chicago", now).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 8, 24, 15, 0, 0).unwrap());
    }

    #[test]
    fn a_schedule_that_can_never_fire_is_an_error_not_a_silent_success() {
        // The 30th of February. Reporting this as scheduled would leave a
        // destination that looks healthy and never runs.
        assert!(next_after("0 0 30 2 *", "America/Chicago", Utc::now()).is_err());
    }

    #[test]
    fn unknown_timezones_are_refused() {
        assert!(next_after("0 2 * * *", "Mars/Olympus_Mons", Utc::now()).is_err());
    }

    #[test]
    fn descriptions_cover_the_presets_and_fall_back_safely() {
        assert_eq!(describe("0 * * * *", "America/Chicago"), "Hourly, on the hour");
        assert_eq!(
            describe("0 2 * * *", "America/Chicago"),
            "Daily at 2:00am (America/Chicago)"
        );
        assert_eq!(
            describe("30 14 * * *", "America/Chicago"),
            "Daily at 2:30pm (America/Chicago)"
        );
        assert_eq!(
            describe("0 3 * * 1", "America/Chicago"),
            "Every Monday at 3:00am (America/Chicago)"
        );
        assert_eq!(
            describe("0 */6 * * *", "America/Chicago"),
            "Every 6 hours (America/Chicago)"
        );
        // Anything it does not recognise shows the expression rather than a
        // confident wrong answer.
        assert_eq!(
            describe("0 0 1 1 *", "America/Chicago"),
            "0 0 1 1 * (America/Chicago)"
        );
    }

    #[test]
    fn midnight_and_noon_read_correctly() {
        assert_eq!(describe("0 0 * * *", "UTC"), "Daily at 12:00am (UTC)");
        assert_eq!(describe("0 12 * * *", "UTC"), "Daily at 12:00pm (UTC)");
    }
}
