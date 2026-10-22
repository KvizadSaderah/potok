//! Accounting months: when one starts, by a rule, and corrected by hand.
//!
//! A month can start on a fixed day (1 — the calendar month) or on the last
//! working day of the calendar month — the day a salary usually arrives. The
//! rule knows weekends, not holidays: a salary paid early because the 31st is
//! a bank holiday is the person's to correct, and a correction for one month
//! is kept and used instead of the rule. Corrections are journaled and undone
//! like any other edit.
//!
//! A cycle is named by the calendar month its rule-given start falls in: the
//! cycle that starts on the last working day of September is «September»,
//! even when a correction moves its start a day earlier into August's end.
//! That is the key it is stored under; the screens name the same cycle by
//! the month it ends in — 30.09–29.10 is «October» to whoever reads it.
use std::collections::BTreeMap;

use anyhow::{bail, Result};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use rusqlite::{params, Connection, OptionalExtension};

use crate::tr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A fixed day, 1–28.
    Day(u32),
    /// The last Monday-to-Friday of the calendar month.
    LastWorkday,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthRule {
    pub kind: Kind,
    /// Corrections by hand: calendar month (year, month) → the day it starts.
    pub overrides: BTreeMap<(i32, u32), NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cycle {
    /// The calendar month it is named by.
    pub key: (i32, u32),
    pub start: NaiveDate,
    /// The last day, inclusive.
    pub end: NaiveDate,
    /// Its start was set by hand.
    pub corrected: bool,
}

fn shift((year, month): (i32, u32), by: i32) -> (i32, u32) {
    let n = year * 12 + month as i32 - 1 + by;
    (n.div_euclid(12), n.rem_euclid(12) as u32 + 1)
}

fn last_day((year, month): (i32, u32)) -> NaiveDate {
    let (y, m) = shift((year, month), 1);
    NaiveDate::from_ymd_opt(y, m, 1).expect("a first day") - Duration::days(1)
}

/// The last Monday-to-Friday of a calendar month.
pub fn last_workday(key: (i32, u32)) -> NaiveDate {
    let mut day = last_day(key);
    while matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
        day -= Duration::days(1);
    }
    day
}

impl MonthRule {
    /// The calendar month: starts on the 1st, no corrections.
    pub fn calendar() -> Self {
        Self { kind: Kind::Day(1), overrides: BTreeMap::new() }
    }

    pub fn load(conn: &Connection) -> Result<Self> {
        let kind = match crate::settings::get(conn, crate::settings::MONTH_MODE)?.as_deref() {
            Some("last_workday") => Kind::LastWorkday,
            _ => Kind::Day(crate::settings::month_day(conn)?),
        };
        let mut stmt = conn.prepare("select month, start from month_start")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut overrides = BTreeMap::new();
        for row in rows {
            let (month, start) = row?;
            let (Ok(first), Ok(start)) = (
                NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d"),
                NaiveDate::parse_from_str(&start, "%Y-%m-%d"),
            ) else {
                continue;
            };
            overrides.insert((first.year(), first.month()), start);
        }
        Ok(Self { kind, overrides })
    }

    /// What the rule alone says for a calendar month.
    pub fn by_rule(&self, key: (i32, u32)) -> NaiveDate {
        match self.kind {
            Kind::Day(day) => NaiveDate::from_ymd_opt(key.0, key.1, day.clamp(1, 28)).expect("days up to 28 exist"),
            Kind::LastWorkday => last_workday(key),
        }
    }

    fn start_of(&self, key: (i32, u32)) -> NaiveDate {
        self.overrides.get(&key).copied().unwrap_or_else(|| self.by_rule(key))
    }

    /// The cycle `day` is in, or the one `back` cycles before it.
    pub fn cycle(&self, day: NaiveDate, back: u32) -> Cycle {
        let mut key = (day.year(), day.month());
        // The start of a neighbour can fall into this calendar month either
        // way: a rule on the last working day, or a correction.
        if day < self.start_of(key) {
            key = shift(key, -1);
        } else if day >= self.start_of(shift(key, 1)) {
            key = shift(key, 1);
        }
        let key = shift(key, -(back as i32));
        Cycle {
            key,
            start: self.start_of(key),
            end: self.start_of(shift(key, 1)) - Duration::days(1),
            corrected: self.overrides.contains_key(&key),
        }
    }
}

/// Correct when a month starts — or with `None`, go back to the rule.
pub fn set_start(conn: &Connection, key: (i32, u32), start: Option<NaiveDate>) -> Result<()> {
    let month = format!("{:04}-{:02}", key.0, key.1);
    if let Some(day) = start {
        let rule = MonthRule::load(conn)?.by_rule(key);
        if (day - rule).num_days().abs() > 15 {
            bail!(tr!(
                "{day} is more than two weeks from {rule}, where {month} starts by the rule",
                "{day} дальше двух недель от {rule}, где {month} начинается по правилу"
            ));
        }
    }
    crate::db::atomic(conn, || {
        let before: Option<String> =
            conn.query_row("select start from month_start where month = ?1", [&month], |r| r.get(0)).optional()?;
        match start {
            Some(day) => {
                conn.execute(
                    "insert into month_start (month, start) values (?1, ?2)
                     on conflict (month) do update set start = excluded.start",
                    params![month, day.to_string()],
                )?;
            }
            None => {
                conn.execute("delete from month_start where month = ?1", [&month])?;
            }
        }
        let now = start.map(|d| d.to_string()).unwrap_or_else(|| tr!("by the rule", "по правилу"));
        crate::journal::log(
            conn,
            &tr!("month {month} starts: {} → {now}", "месяц {month} начинается: {} → {now}", before.clone().unwrap_or_else(|| tr!("by the rule", "по правилу"))),
            Some(crate::journal::Undo::MonthStart(month, before)),
        )?;
        Ok(())
    })
}

/// Which span the flows are shown for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    ThisMonth,
    PreviousMonth,
    ThisYear,
    Everything,
}

impl Period {
    /// How the period is stored in the settings.
    pub fn code(self) -> &'static str {
        match self {
            Period::ThisMonth => "this_month",
            Period::PreviousMonth => "previous_month",
            Period::ThisYear => "this_year",
            Period::Everything => "everything",
        }
    }

    pub fn from_code(code: &str) -> Option<Period> {
        Period::ALL.into_iter().find(|p| p.code() == code)
    }

    pub const ALL: [Period; 4] =
        [Period::ThisMonth, Period::PreviousMonth, Period::ThisYear, Period::Everything];

    pub fn label(self) -> &'static str {
        match self {
            Period::ThisMonth => crate::trs!("this month", "этот месяц"),
            Period::PreviousMonth => crate::trs!("last month", "прошлый месяц"),
            Period::ThisYear => crate::trs!("this year", "этот год"),
            Period::Everything => crate::trs!("all time", "всё время"),
        }
    }

    /// The bounds are inclusive. Counted from the date passed in rather than
    /// from `now()`, so the test does not depend on when it ran.
    pub fn bounds(self, today: NaiveDate) -> (String, String) {
        self.bounds_on(today, &MonthRule::calendar())
    }

    pub fn previous_bounds(self, today: NaiveDate) -> Option<(String, String)> {
        self.previous_bounds_on(today, &MonthRule::calendar())
    }

    /// The period by the ledger's month rule (a salary day, the last working
    /// day, corrections). The year and everything are calendar ones.
    pub fn bounds_on(self, today: NaiveDate, rule: &MonthRule) -> (String, String) {
        match self {
            Period::ThisMonth => (rule.cycle(today, 0).start.to_string(), today.to_string()),
            Period::PreviousMonth => {
                let c = rule.cycle(today, 1);
                (c.start.to_string(), c.end.to_string())
            }
            _ => self.calendar_bounds(today),
        }
    }

    /// The same stretch before: as long as this one, so mid-month compares
    /// with mid-month.
    pub fn previous_bounds_on(self, today: NaiveDate, rule: &MonthRule) -> Option<(String, String)> {
        match self {
            Period::ThisMonth => {
                let (this, before) = (rule.cycle(today, 0), rule.cycle(today, 1));
                let end = (before.start + (today - this.start)).min(before.end);
                Some((before.start.to_string(), end.to_string()))
            }
            Period::PreviousMonth => {
                let c = rule.cycle(today, 2);
                Some((c.start.to_string(), c.end.to_string()))
            }
            _ => self.calendar_previous_bounds(today),
        }
    }

    fn calendar_bounds(self, today: NaiveDate) -> (String, String) {
        match self {
            Period::ThisMonth => {
                let first = today.with_day(1).expect("every month has a first day");
                (first.to_string(), today.to_string())
            }
            Period::PreviousMonth => {
                let first_this = today.with_day(1).expect("the first day");
                let last_previous = first_this.pred_opt().expect("the day before the first");
                let first_previous =
                    last_previous.with_day(1).expect("the first day of last month");
                (first_previous.to_string(), last_previous.to_string())
            }
            Period::ThisYear => (format!("{}-01-01", today.year()), today.to_string()),
            Period::Everything => ("0001-01-01".to_string(), "9999-12-31".to_string()),
        }
    }

    /// What to compare with. A span of the same length before this one, not
    /// «the whole of last month»: eighteen days of October against thirty days
    /// of September would always show you spending half as much.
    fn calendar_previous_bounds(self, today: NaiveDate) -> Option<(String, String)> {
        match self {
            Period::ThisMonth => {
                let first_this = today.with_day(1).expect("the first day");
                let last_previous = first_this.pred_opt().expect("the day before the first");
                let first_previous = last_previous.with_day(1).expect("the first day");
                // 31 March → 28 February: February has no thirty-first.
                let end = last_previous.with_day(today.day()).unwrap_or(last_previous);
                Some((first_previous.to_string(), end.to_string()))
            }
            Period::PreviousMonth => {
                let (first, _) = Period::PreviousMonth.bounds(today);
                let first = NaiveDate::parse_from_str(&first, "%Y-%m-%d").expect("a date of our own making");
                Some(Period::PreviousMonth.bounds(first))
            }
            Period::ThisYear => {
                let year = today.year() - 1;
                // 29 February of a leap year → 28 February of the year before.
                let end = NaiveDate::from_ymd_opt(year, today.month(), today.day())
                    .or_else(|| NaiveDate::from_ymd_opt(year, today.month(), today.day() - 1))
                    .expect("the same day last year");
                Some((format!("{year}-01-01"), end.to_string()))
            }
            Period::Everything => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn the_last_working_day_skips_the_weekend() {
        assert_eq!(last_workday((2026, 10)), day("2026-10-30"), "31 October 2026 is a Saturday");
        assert_eq!(last_workday((2026, 5)), day("2026-05-29"), "31 May is a Sunday");
        assert_eq!(last_workday((2026, 9)), day("2026-09-30"), "a Wednesday");
    }

    #[test]
    fn a_month_runs_from_one_payday_to_the_day_before_the_next() {
        let rule = MonthRule { kind: Kind::LastWorkday, overrides: BTreeMap::new() };
        let now = rule.cycle(day("2026-10-19"), 0);
        assert_eq!((now.key, now.start, now.end), ((2026, 9), day("2026-09-30"), day("2026-10-29")));
        let paid = rule.cycle(day("2026-10-30"), 0);
        assert_eq!((paid.key, paid.start), ((2026, 10), day("2026-10-30")), "payday starts the next one");
        let before = rule.cycle(day("2026-10-19"), 1);
        assert_eq!((before.start, before.end), (day("2026-08-31"), day("2026-09-29")));
        let calendar = MonthRule::calendar().cycle(day("2026-10-19"), 0);
        assert_eq!((calendar.start, calendar.end), (day("2026-10-01"), day("2026-10-31")));
    }

    #[test]
    fn a_correction_moves_one_month_and_is_undone() {
        let mut conn = crate::db::open_memory().unwrap();
        crate::settings::set(&conn, crate::settings::MONTH_MODE, "last_workday").unwrap();
        // the salary came a day early in October
        set_start(&conn, (2026, 10), Some(day("2026-10-29"))).unwrap();
        let rule = MonthRule::load(&conn).unwrap();
        let now = rule.cycle(day("2026-10-29"), 0);
        assert_eq!((now.key, now.start, now.corrected), ((2026, 10), day("2026-10-29"), true));
        assert_eq!(rule.cycle(day("2026-10-28"), 0).end, day("2026-10-28"));
        assert!(set_start(&conn, (2026, 10), Some(day("2026-12-01"))).is_err(), "too far from the rule");

        let entry = crate::journal::entries(&conn, 1).unwrap().remove(0);
        crate::journal::undo(&mut conn, entry.id).unwrap();
        assert!(MonthRule::load(&conn).unwrap().overrides.is_empty());
    }
}
