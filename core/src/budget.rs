//! Budgets: a monthly limit per category, and how much of it is left.
//!
//! Only the limit is stored. What is spent is the flows' figure for the same
//! category and month — one rule for both, so a budget and the flows screen
//! can never disagree about what was spent.
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::journal::{self, Undo};
use crate::{tr, Currency, Money};

/// The limit in the ledger's base currency.
pub fn get(conn: &Connection, category: &str) -> Result<Option<Money>> {
    let found: Option<(String, i64)> = conn
        .query_row(
            "select b.currency, b.limit_minor from budget b join category c on c.id = b.category_id where c.name = ?1",
            [category],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match found {
        Some((currency, minor)) => Some(Money::new(minor, Currency::new(&currency)?)),
        None => None,
    })
}

/// Set a limit, or drop it with `None`.
pub fn set(conn: &Connection, category: &str, limit_minor: Option<i64>) -> Result<()> {
    crate::db::atomic(conn, || {
        let id: i64 = conn
            .query_row("select id from category where name = ?1", [category], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!(tr!("no category «{category}»", "нет категории «{category}»")))?;
        let before = get(conn, category)?;
        match limit_minor {
            Some(minor) if minor <= 0 => bail!(tr!("a budget is more than zero", "бюджет — больше нуля")),
            Some(minor) => {
                let currency = crate::settings::base_currency(conn)?;
                conn.execute(
                    "insert into budget (category_id, currency, limit_minor) values (?1, ?2, ?3)
                     on conflict (category_id) do update set currency = excluded.currency, limit_minor = excluded.limit_minor",
                    params![id, currency.as_str(), minor],
                )?;
            }
            None => {
                conn.execute("delete from budget where category_id = ?1", [id])?;
            }
        }
        let now = limit_minor.map(crate::money::format_minor).unwrap_or_else(|| "—".into());
        journal::log(
            conn,
            &tr!("budget «{category}»: {} → {now}", "бюджет «{category}»: {} → {now}", before.map(|m| m.decimal()).unwrap_or_else(|| "—".into())),
            Some(Undo::Budget(category.to_string(), before.map(|m| m.minor()))),
        )?;
        Ok(())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetLine {
    pub category: String,
    pub limit: Money,
    /// Spent in the budget's currency over the period (all currencies with a
    /// rate on `to` converted in full precision).
    pub spent: Money,
    pub left: Money,
    /// Currencies spent in this category that had no exchange rate to the
    /// budget's currency on `to`.
    pub missing: Vec<Currency>,
}

/// Every budget against what was spent from `from` to `to`, the most used
/// first.
pub fn status(conn: &Connection, from: &str, to: &str) -> Result<Vec<BudgetLine>> {
    let mut stmt = conn.prepare(
        "select c.name, b.currency, b.limit_minor from budget b join category c on c.id = b.category_id order by c.name",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let flows = crate::query::flows(conn, from, to)?;
    let mut out = Vec::new();
    for (category, currency, limit) in rows {
        let currency = Currency::new(&currency)?;
        let (spent, missing) = match flows.iter().find(|f| f.label == category) {
            Some(f) => {
                let (m, miss) = crate::rate::convert_bag_partial(conn, &f.out, currency, to)?;
                (m.minor(), miss)
            }
            None => (0, Vec::new()),
        };
        out.push(BudgetLine {
            category,
            limit: Money::new(limit, currency),
            spent: Money::new(spent, currency),
            left: Money::new(limit - spent, currency),
            missing,
        });
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.spent.minor() * 1000 / b.limit.minor().max(1)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_counts_what_the_flows_count() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'Cash','EUR','CASH');
             insert into category (id,name) values (1,'Food');
             insert into counterparty (id,name,category_id) values (1,'Lidl',1);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id) values
               (1,'expense','2026-09-10T00:00:00Z',3000,'EUR',-3000,1),
               (1,'expense','2026-08-10T00:00:00Z',9000,'EUR',-9000,1);",
        )
        .unwrap();
        set(&conn, "Food", Some(20000)).unwrap();
        let lines = status(&conn, "2026-09-01", "2026-09-30").unwrap();
        assert_eq!((lines[0].spent.minor(), lines[0].left.minor()), (3000, 17000));

        // Spends in another currency count in the budget at the rate rather than vanishing.
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (2,'Card RUB','RUB','CARD');
             insert into rate (on_date,base,quote,rate_e9,source) values ('2026-09-01','RUB','EUR',10000000,'manual');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id) values
               (2,'expense','2026-09-15T00:00:00Z',500000,'RUB',-500000,1);",
        )
        .unwrap();
        let lines = status(&conn, "2026-09-01", "2026-09-30").unwrap();
        assert_eq!((lines[0].spent.minor(), lines[0].left.minor()), (8000, 12000), "30 EUR + 5000 RUB (50 EUR) = 80 EUR");

        assert!(set(&conn, "Food", Some(0)).is_err());
        set(&conn, "Food", None).unwrap();
        assert!(status(&conn, "2026-09-01", "2026-09-30").unwrap().is_empty());
        assert!(set(&conn, "Nope", Some(1)).is_err());
    }
}
