//! Schedules for scheduled jobs (ADR 0017): standard 5-field cron expressions
//! (`minute hour day-of-month month day-of-week`) in local time, plus plain-language
//! descriptions of the presets the UI offers.
//!
//! Supported per field: `*`, numbers, `a-b` ranges, `a,b` lists and `/n` steps (`*/5`, `1-30/2`).
//! Day of week is 0–6 (0 = Sunday; 7 is accepted as Sunday). As in classic cron, when both
//! day-of-month and day-of-week are restricted a day matches if either does.

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    days_any: bool,
    weekdays_any: bool,
    source: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CronError {
    #[error("a schedule needs 5 fields (minute hour day month weekday), found {0}")]
    FieldCount(usize),
    #[error("invalid {field} \"{value}\": {reason}")]
    Field {
        field: &'static str,
        value: String,
        reason: String,
    },
}

const FIELDS: [(&str, u32, u32); 5] = [
    ("minute", 0, 59),
    ("hour", 0, 23),
    ("day of month", 1, 31),
    ("month", 1, 12),
    ("day of week", 0, 7),
];

impl Cron {
    pub fn parse(expr: &str) -> Result<Self, CronError> {
        let parts: Vec<&str> = expr.split_whitespace().collect();
        if parts.len() != 5 {
            return Err(CronError::FieldCount(parts.len()));
        }
        let mut sets = [0u64; 5];
        for (i, part) in parts.iter().enumerate() {
            let (name, min, max) = FIELDS[i];
            sets[i] = parse_field(part, min, max).map_err(|reason| CronError::Field {
                field: name,
                value: (*part).to_owned(),
                reason,
            })?;
        }
        // 7 is Sunday too.
        if sets[4] & (1 << 7) != 0 {
            sets[4] = (sets[4] & !(1 << 7)) | 1;
        }
        Ok(Self {
            minutes: sets[0],
            hours: sets[1],
            days: sets[2],
            months: sets[3],
            weekdays: sets[4],
            days_any: parts[2] == "*",
            weekdays_any: parts[4] == "*",
            source: parts.join(" "),
        })
    }

    /// The normalised expression (single spaces).
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether the schedule fires in the minute containing `t`.
    pub fn matches<Tz: TimeZone>(&self, t: &DateTime<Tz>) -> bool {
        let bit = |set: u64, v: u32| set & (1 << v) != 0;
        if !bit(self.minutes, t.minute())
            || !bit(self.hours, t.hour())
            || !bit(self.months, t.month())
        {
            return false;
        }
        let dom = bit(self.days, t.day());
        let dow = bit(self.weekdays, t.weekday().num_days_from_sunday());
        match (self.days_any, self.weekdays_any) {
            (true, true) => true,
            (true, false) => dow,
            (false, true) => dom,
            (false, false) => dom || dow,
        }
    }

    /// The next `n` firing minutes strictly after `after`, searching up to ~4 years ahead
    /// (enough for 29 February).
    pub fn next_runs<Tz: TimeZone>(&self, after: &DateTime<Tz>, n: usize) -> Vec<DateTime<Tz>> {
        let mut out = Vec::with_capacity(n);
        let start = after
            .clone()
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0));
        let Some(mut t) = start else { return out };
        let limit = 4 * 366 * 24 * 60;
        for _ in 0..limit {
            t += Duration::minutes(1);
            if self.matches(&t) {
                out.push(t.clone());
                if out.len() == n {
                    break;
                }
            }
        }
        out
    }

    /// A plain-language description for the presets the UI offers; otherwise the expression.
    pub fn describe(&self) -> String {
        let parts: Vec<&str> = self.source.split(' ').collect();
        let [m, h, dom, mon, dow] = [parts[0], parts[1], parts[2], parts[3], parts[4]];
        let at = |h: &str, m: &str| -> Option<String> {
            let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
            Some(format!("{h:02}:{m:02}"))
        };
        match (m, h, dom, mon, dow) {
            ("*", "*", "*", "*", "*") => "Every minute".into(),
            (step, "*", "*", "*", "*") if step.starts_with("*/") => {
                format!("Every {} minutes", &step[2..])
            }
            ("0", "*", "*", "*", "*") => "Every hour".into(),
            (m, h, "*", "*", "*") if at(h, m).is_some() => {
                format!("Every day at {}", at(h, m).unwrap_or_default())
            }
            (m, h, "*", "*", d) if at(h, m).is_some() && d.parse::<u32>().is_ok_and(|d| d <= 7) => {
                let day = [
                    "Sunday",
                    "Monday",
                    "Tuesday",
                    "Wednesday",
                    "Thursday",
                    "Friday",
                    "Saturday",
                    "Sunday",
                ][d.parse::<usize>().unwrap_or(0)];
                format!("Every {day} at {}", at(h, m).unwrap_or_default())
            }
            _ => format!("Cron: {}", self.source),
        }
    }
}

fn parse_field(field: &str, min: u32, max: u32) -> Result<u64, String> {
    let mut set = 0u64;
    for item in field.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s
                    .parse()
                    .map_err(|_| format!("step \"{s}\" is not a number"))?;
                if step == 0 {
                    return Err("a step must be at least 1".into());
                }
                (r, step)
            }
            None => (item, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (number(a, min, max)?, number(b, min, max)?)
        } else {
            let v = number(range, min, max)?;
            // `5/10` means "from 5 to the end, every 10", as in Vixie cron.
            (v, if item.contains('/') { max } else { v })
        };
        if lo > hi {
            return Err(format!("range {lo}-{hi} goes backwards"));
        }
        let mut v = lo;
        while v <= hi {
            set |= 1 << v;
            v += step;
        }
    }
    Ok(set)
}

fn number(s: &str, min: u32, max: u32) -> Result<u32, String> {
    let v: u32 = s.parse().map_err(|_| format!("\"{s}\" is not a number"))?;
    if v < min || v > max {
        return Err(format!("{v} is outside {min}-{max}"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, Utc};

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(y, mo, d)
                .unwrap()
                .and_hms_opt(h, mi, 0)
                .unwrap(),
        )
    }

    #[test]
    fn every_minute_and_steps() {
        let c = Cron::parse("*  *  * * *").unwrap();
        assert_eq!(c.as_str(), "* * * * *");
        assert!(c.matches(&at(2026, 10, 16, 9, 3)));
        let c = Cron::parse("*/15 * * * *").unwrap();
        let next: Vec<u32> = c
            .next_runs(&at(2026, 10, 16, 9, 3), 3)
            .iter()
            .map(|t| t.minute())
            .collect();
        assert_eq!(next, vec![15, 30, 45]);
    }

    #[test]
    fn next_runs_are_strictly_after_and_cross_days() {
        let c = Cron::parse("30 2 * * *").unwrap();
        let runs = c.next_runs(&at(2026, 10, 16, 2, 30), 2);
        assert_eq!(runs, vec![at(2026, 10, 17, 2, 30), at(2026, 10, 18, 2, 30)]);
    }

    #[test]
    fn weekdays_and_sunday_as_seven() {
        // 2026-10-18 is a Sunday.
        let c = Cron::parse("0 9 * * 7").unwrap();
        assert_eq!(
            c.next_runs(&at(2026, 10, 16, 0, 0), 1),
            vec![at(2026, 10, 18, 9, 0)]
        );
        let c = Cron::parse("0 9 * * 1-5").unwrap();
        assert!(!c.matches(&at(2026, 10, 18, 9, 0)));
        assert!(c.matches(&at(2026, 10, 19, 9, 0)));
    }

    #[test]
    fn day_of_month_or_day_of_week() {
        // The 1st, or any Monday.
        let c = Cron::parse("0 0 1 * 1").unwrap();
        assert!(c.matches(&at(2026, 10, 1, 0, 0))); // a Thursday, but the 1st
        assert!(c.matches(&at(2026, 10, 19, 0, 0))); // a Monday
        assert!(!c.matches(&at(2026, 10, 20, 0, 0)));
    }

    #[test]
    fn leap_day_is_found() {
        let c = Cron::parse("0 0 29 2 *").unwrap();
        assert_eq!(
            c.next_runs(&at(2026, 10, 16, 0, 0), 1),
            vec![at(2028, 2, 29, 0, 0)]
        );
    }

    #[test]
    fn invalid_expressions_are_explained() {
        assert_eq!(Cron::parse("* * *"), Err(CronError::FieldCount(3)));
        assert!(matches!(
            Cron::parse("60 * * * *"),
            Err(CronError::Field {
                field: "minute",
                ..
            })
        ));
        assert!(matches!(
            Cron::parse("*/0 * * * *"),
            Err(CronError::Field { .. })
        ));
        assert!(matches!(
            Cron::parse("5-1 * * * *"),
            Err(CronError::Field { .. })
        ));
        assert!(matches!(
            Cron::parse("* * * * mon"),
            Err(CronError::Field {
                field: "day of week",
                ..
            })
        ));
        let e = Cron::parse("61 * * * *").unwrap_err().to_string();
        assert!(e.contains("minute") && e.contains("0-59"), "{e}");
    }

    #[test]
    fn presets_are_described_in_words() {
        let d = |e: &str| Cron::parse(e).unwrap().describe();
        assert_eq!(d("* * * * *"), "Every minute");
        assert_eq!(d("*/5 * * * *"), "Every 5 minutes");
        assert_eq!(d("0 * * * *"), "Every hour");
        assert_eq!(d("30 2 * * *"), "Every day at 02:30");
        assert_eq!(d("0 9 * * 1"), "Every Monday at 09:00");
        assert_eq!(d("0 9 1 * *"), "Cron: 0 9 1 * *");
    }
}
