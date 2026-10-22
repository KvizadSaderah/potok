//! Analytics: months side by side, a category over time, payments that
//! repeat, and the overview the first screen shows.
//!
//! Everything here is read through the flows' own rule (`query::flows`), so a
//! month in analytics is the same month the flows screen shows — one rule for
//! what counts as spending, not two that drift.
use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate};
use rusqlite::Connection;

use crate::{Currency, Money, MoneyBag};

#[derive(Debug, Clone)]
pub struct Month {
    /// First day of the accounting month.
    pub start: NaiveDate,
    /// Last day counted: the month's end, or today for the current one.
    pub end: NaiveDate,
    /// Its start was corrected by hand.
    pub corrected: bool,
    /// The same in the base currency, at the month's last day's rates;
    /// `None` when a currency in it has no rate.
    pub inn_base: Option<Money>,
    pub out_base: Option<Money>,
    /// In less out, in the base currency; `None` without a rate for either.
    pub net_base: Option<Money>,
    pub inn: MoneyBag,
    pub out: MoneyBag,
}

/// The last `count` months up to the one `today` is in, oldest first; the
/// current one only up to `today`.
pub fn months(conn: &Connection, today: NaiveDate, count: u32) -> Result<Vec<Month>> {
    let rule = crate::period::MonthRule::load(conn)?;
    let mut out = Vec::new();
    for back in (0..count).rev() {
        let cycle = rule.cycle(today, back);
        let end = cycle.end.min(today);
        let mut month = months_between(conn, cycle.start, end)?;
        month.corrected = cycle.corrected;
        out.push(month);
    }
    Ok(out)
}

/// Spending in one flow row (a category, or a counterparty with none) month
/// by month, oldest first.
pub fn trend(conn: &Connection, label: &str, today: NaiveDate, count: u32) -> Result<Vec<(NaiveDate, MoneyBag)>> {
    let rule = crate::period::MonthRule::load(conn)?;
    let mut out = Vec::new();
    for back in (0..count).rev() {
        let cycle = rule.cycle(today, back);
        let end = cycle.end.min(today);
        let flows = crate::query::flows(conn, &cycle.start.to_string(), &end.to_string())?;
        let spent = flows.into_iter().find(|f| f.label == label).map(|f| f.out).unwrap_or_default();
        out.push((cycle.start, spent));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regular {
    pub who: String,
    /// The usual amount: the median of the payments.
    pub typical: Money,
    /// Days between payments, the median.
    pub every_days: i64,
    pub last: NaiveDate,
    /// When the next one is expected.
    pub next: NaiveDate,
    pub count: usize,
}

/// Payments that repeat: the same counterparty, out, monthly (26–35 days
/// apart) or weekly (6–8), in at least three months of the last half year,
/// most of them within a tenth of the usual amount, and still going — one
/// that missed two of its dates has stopped. A supermarket visited often is
/// not a bill, and the first version, looser, listed Lidl every nine days.
// ponytail: a median and two tolerances, not a seasonal model; a yearly
// subscription is not caught (needs more than half a year of history).
pub fn regular(conn: &Connection, today: NaiveDate) -> Result<Vec<Regular>> {
    let since = (today - Duration::days(190)).to_string();
    let mut stmt = conn.prepare(
        "select p.name, date(o.occurred_at), -o.effect_minor, o.currency
           from operation o join counterparty p on p.id = o.counterparty_id
          where o.state = 'POSTED' and o.role in ('expense', 'debt_movement') and o.effect_minor < 0
            and date(o.occurred_at) >= date(?1)
          order by p.name, o.occurred_at",
    )?;
    let rows = stmt
        .query_map([&since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_who: std::collections::BTreeMap<(String, String), Vec<(NaiveDate, i64)>> = Default::default();
    for (who, day, amount, currency) in rows {
        if let Ok(day) = NaiveDate::parse_from_str(&day, "%Y-%m-%d") {
            by_who.entry((who, currency)).or_default().push((day, amount));
        }
    }
    let median = |mut v: Vec<i64>| -> i64 {
        v.sort_unstable();
        v[v.len() / 2]
    };
    let mut out = Vec::new();
    for ((who, currency), payments) in by_who {
        let months: std::collections::BTreeSet<(i32, u32)> = payments.iter().map(|(d, _)| (d.year(), d.month())).collect();
        if months.len() < 3 {
            continue;
        }
        let typical = median(payments.iter().map(|(_, a)| *a).collect());
        // Payments of about the usual size; small extras to the same payee
        // (a fee, a coffee) do not make it irregular.
        let usual: Vec<&(NaiveDate, i64)> = payments.iter().filter(|(_, a)| (a - typical).abs() * 10 <= typical).collect();
        if usual.len() * 4 < payments.len() * 3 {
            continue;
        }
        let usual_months: std::collections::BTreeSet<(i32, u32)> = usual.iter().map(|(d, _)| (d.year(), d.month())).collect();
        if usual_months.len() < 3 {
            continue;
        }
        let gaps: Vec<i64> = usual.windows(2).map(|w| (w[1].0 - w[0].0).num_days()).filter(|g| *g > 0).collect();
        if gaps.is_empty() {
            continue;
        }
        let every = median(gaps.clone());
        let monthly = (26..=35).contains(&every);
        let weekly = (6..=8).contains(&every);
        if !(monthly || weekly) || gaps.iter().filter(|g| (**g - every).abs() * 4 <= every).count() * 4 < gaps.len() * 3 {
            continue;
        }
        let last = usual.last().map(|(d, _)| *d).expect("not empty");
        if last + Duration::days(every * 2) < today {
            continue;
        }
        out.push(Regular {
            who,
            typical: Money::new(typical, Currency::new(&currency)?),
            every_days: every,
            last,
            next: last + Duration::days(every),
            count: usual.len(),
        });
    }
    out.sort_by_key(|r| r.next);
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct Overview {
    pub total: crate::rate::Total,
    pub this_month: Month,
    /// The same days of last month — a fair comparison mid-month.
    pub last_month_so_far: Month,
    pub i_owe: MoneyBag,
    pub owed_to_me: MoneyBag,
    pub budgets: Vec<crate::budget::BudgetLine>,
    /// Regular payments due within two weeks, or overdue by up to a week.
    pub upcoming: Vec<Regular>,
    /// What they come to.
    pub upcoming_total: MoneyBag,
    /// This month by category, the biggest first — the flows screen's rows.
    pub where_it_went: Vec<crate::query::Flow>,
    /// Accounts never reconciled, or off at their last reconciliation.
    pub unreconciled: Vec<String>,
    pub duplicates: usize,
    /// Latest import per bank: (bank, day).
    pub imports: Vec<(String, String)>,
    /// The day of the oldest rate the totals stand on, when it is more than a
    /// week old: rates that stopped coming are still used, and said so here.
    pub stale_rates: Option<NaiveDate>,
    /// Money back from purchases that still counts as income.
    pub refunds_as_income: usize,
}

pub fn overview(conn: &Connection, today: NaiveDate) -> Result<Overview> {
    let rule = crate::period::MonthRule::load(conn)?;
    let this = rule.cycle(today, 0);
    let start = this.start;
    let this_month = months_between(conn, start, today)?;
    let before = rule.cycle(today, 1);
    let same_day = (before.start + Duration::days((today - start).num_days())).min(before.end);
    let last_month_so_far = months_between(conn, before.start, same_day)?;

    let mut i_owe = MoneyBag::new();
    let mut owed_to_me = MoneyBag::new();
    for debt in crate::query::debts(conn, false)? {
        if debt.direction == "I_OWE" {
            i_owe.add(debt.remaining);
        } else {
            owed_to_me.add(debt.remaining);
        }
    }

    let upcoming: Vec<Regular> = regular(conn, today)?
        .into_iter()
        .filter(|r| r.next <= today + Duration::days(14) && r.next >= today - Duration::days(7))
        .collect();
    let mut upcoming_total = MoneyBag::new();
    for r in &upcoming {
        upcoming_total.add(r.typical);
    }

    let mut unreconciled = Vec::new();
    // An account with nothing on it has nothing to reconcile.
    for account in crate::query::balances(conn)?.into_iter().filter(|a| a.operations > 0) {
        match crate::reconcile::last(conn, account.id)? {
            Some(r) if r.matches() => {}
            _ => unreconciled.push(account.name),
        }
    }

    let mut stmt = conn.prepare(
        "select bank, max(date(imported_at)) from import_file where bank <> 'saldo-migration' group by bank order by 2 desc",
    )?;
    let imports = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Overview {
        total: crate::rate::total(conn, &today.to_string())?,
        this_month,
        last_month_so_far,
        i_owe,
        owed_to_me,
        budgets: crate::budget::status(conn, &start.to_string(), &today.to_string())?,
        upcoming,
        upcoming_total,
        where_it_went: crate::query::flows(conn, &start.to_string(), &today.to_string())?,
        unreconciled,
        duplicates: crate::reconcile::duplicates(conn, None)?.len(),
        imports,
        stale_rates: crate::rate::table(conn, &today.to_string())?
            .iter()
            .filter_map(|row| row.stored.first().and_then(|(_, _, day, _)| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()))
            .min()
            .filter(|oldest| (today - *oldest).num_days() > 7),
        refunds_as_income: crate::counterparty::refunds_counted_as_income(conn)?.len(),
    })
}

/// An account's balance at the end of each of `days` — the line drawn next
/// to it. The same balance reconciliation compares with the bank.
pub fn balance_history(conn: &Connection, account_id: i64, days: &[NaiveDate]) -> Result<Vec<Money>> {
    days.iter().map(|day| crate::reconcile::balance_on(conn, account_id, &day.to_string())).collect()
}

/// What moved an account between two days, by kind, signed as the account
/// saw it: money in, money spent, to and from own accounts, debts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountMoves {
    pub inn: Money,
    pub out: Money,
    pub transfers: Money,
    pub debts: Money,
}

pub fn account_moves(conn: &Connection, account_id: i64, from: NaiveDate, to: NaiveDate) -> Result<AccountMoves> {
    let currency: String = conn.query_row("select currency from account where id = ?1", [account_id], |r| r.get(0))?;
    let currency = Currency::new(&currency)?;
    let sum = |roles: &str| -> Result<Money> {
        let minor: i64 = conn.query_row(
            &format!(
                "select coalesce(sum(effect_minor), 0) from operation
                  where account_id = ?1 and state = 'POSTED' and role in ({roles})
                    and date(occurred_at) between date(?2) and date(?3)"
            ),
            rusqlite::params![account_id, from.to_string(), to.to_string()],
            |r| r.get(0),
        )?;
        Ok(Money::new(minor, currency))
    };
    Ok(AccountMoves {
        inn: sum("'income'")?,
        out: sum("'expense'")?,
        transfers: sum("'transfer'")?,
        debts: sum("'debt_movement'")?,
    })
}

fn months_between(conn: &Connection, from: NaiveDate, to: NaiveDate) -> Result<Month> {
    let flows = crate::query::flows(conn, &from.to_string(), &to.to_string())?;
    let (out, inn) = crate::query::flow_totals(&flows);
    let (inn_base, out_base) = (to_base(conn, &inn, to)?, to_base(conn, &out, to)?);
    Ok(Month {
        start: from,
        end: to,
        corrected: false,
        net_base: match (inn_base, out_base) {
            (Some(i), Some(o)) => Some(Money::new(i.minor() - o.minor(), i.currency())),
            _ => None,
        },
        inn_base,
        out_base,
        inn,
        out,
    })
}

/// A bag of currencies as one sum in the base currency, at the rates of `on`;
/// `None` if any currency in it has no rate — a partial sum would read as
/// a whole one. Summed in full precision before rounding once at the end (§2).
pub fn to_base(conn: &Connection, bag: &MoneyBag, on: NaiveDate) -> Result<Option<Money>> {
    let base = crate::settings::base_currency(conn)?;
    crate::rate::convert_bag(conn, bag, base, &on.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Rent on the third for half a year, plus coffee there and a chance purchase.
    fn ledger() -> Connection {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'iCard','EUR','CARD');
             insert into counterparty (id,name) values (1,'Landlord'), (2,'IKEA');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id) values
               (1,'expense','2026-05-03',160000,'EUR',-160000,1),
               (1,'expense','2026-06-03',160000,'EUR',-160000,1),
               (1,'expense','2026-07-02',160000,'EUR',-160000,1),
               (1,'expense','2026-08-03',160000,'EUR',-160000,1),
               (1,'expense','2026-08-04',300,'EUR',-300,1),
               (1,'expense','2026-09-03',160000,'EUR',-160000,1),
               (1,'expense','2026-10-03',160000,'EUR',-160000,1),
               (1,'expense','2026-09-15',5000,'EUR',-5000,2),
               (1,'income','2026-10-01',300000,'EUR',300000,null);",
        )
        .unwrap();
        conn
    }

    /// The figures the redrawn screens show come from here: a balance line,
    /// what moved an account, what is due, what each day cost.
    #[test]
    fn the_screens_figures_are_counted_here() {
        let conn = ledger();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (2,'Revolut','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor) values
               (100,1,'transfer','2026-10-02',10000,'EUR',-10000), (101,2,'transfer','2026-10-02',10000,'EUR',10000);
             update operation set pair_id = 101 where id = 100;
             update operation set pair_id = 100 where id = 101;",
        )
        .unwrap();
        let line = balance_history(&conn, 1, &[day("2026-09-30"), day("2026-10-01"), day("2026-10-03")]).unwrap();
        let minors: Vec<i64> = line.iter().map(|m| m.minor()).collect();
        assert_eq!(minors, [-805300, -505300, -675300]);

        let moved = account_moves(&conn, 1, day("2026-10-01"), day("2026-10-31")).unwrap();
        assert_eq!(
            (moved.inn.minor(), moved.out.minor(), moved.transfers.minor(), moved.debts.minor()),
            (300000, -160000, -10000, 0)
        );

        let o = overview(&conn, day("2026-10-25")).unwrap();
        assert_eq!(o.upcoming.len(), 1, "rent again on the 3rd");
        assert_eq!(o.upcoming_total.in_display_order(None)[0].minor(), 160000);
        assert_eq!(o.where_it_went.first().map(|f| f.label.as_str()), Some("Landlord"));

        let ops = crate::query::operations(&conn, None, false).unwrap();
        let days = crate::query::spent_by_day(&ops);
        assert_eq!(days["2026-08-03"].in_display_order(None)[0].minor(), 160000);
        assert!(!days.contains_key("2026-10-02"), "a transfer is not spending");
        assert!(!days.contains_key("2026-10-01"), "nor is income");
    }

    #[test]
    fn months_are_counted_by_the_flows_rule() {
        let conn = ledger();
        let m = months(&conn, day("2026-10-19"), 3).unwrap();
        assert_eq!(m.iter().map(|m| m.start.to_string()).collect::<Vec<_>>(), ["2026-08-01", "2026-09-01", "2026-10-01"]);
        assert_eq!(m[1].out.in_display_order(None)[0].minor(), 165000);
        assert_eq!(m[2].inn.in_display_order(None)[0].minor(), 300000);
        assert_eq!(m[1].out_base.map(|m| m.minor()), Some(165000), "one currency is its own base");
        crate::settings::set(&conn, crate::settings::MONTH_MODE, "last_workday").unwrap();
        let m = months(&conn, day("2026-10-19"), 2).unwrap();
        assert_eq!((m[1].start, m[1].end), (day("2026-09-30"), day("2026-10-19")), "from the last working day of September");
        assert_eq!((m[0].start, m[0].end), (day("2026-08-31"), day("2026-09-29")));
        let t = trend(&conn, "Landlord", day("2026-10-19"), 2).unwrap();
        assert_eq!(t[0].1.in_display_order(None)[0].minor(), 160000);
    }

    #[test]
    fn rent_is_regular_and_a_one_off_is_not() {
        let conn = ledger();
        let found = regular(&conn, day("2026-10-19")).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        let rent = &found[0];
        assert_eq!((rent.who.as_str(), rent.typical.minor()), ("Landlord", 160000));
        assert!((28..=32).contains(&rent.every_days), "{rent:?}");
        assert_eq!(rent.last, day("2026-10-03"));
        let o = overview(&conn, day("2026-10-30")).unwrap();
        assert_eq!(o.upcoming.len(), 1, "due early November");
        assert_eq!(o.unreconciled, ["iCard"]);
    }
}
