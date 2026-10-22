//! The details: what the pane beside the list shows.
//!
//! In the interface nothing could be seen behind a list row — «neither open the
//! details nor look at them», as was said as soon as it began to be used on the
//! real ledger. Here is gathered what the pane shows about the chosen row. The
//! queries are in the core, like everything else (§5): the interface gets
//! finished values and draws them.
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::money::{Currency, Money, MoneyBag};
use crate::query::{self, OperationFilter, OperationLine};

/// How many of the latest operations to show in the details. The pane is no
/// journal; everything else is behind Enter.
pub const RECENT: usize = 12;

// ── An operation ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CategorySource {
    /// Its own, on the operation itself — an exception to the counterparty's rule.
    Own,
    /// From the counterparty: it changes together with it.
    Counterparty,
}

#[derive(Debug, Clone)]
pub struct ImportSource {
    pub bank: String,
    pub filename: String,
    pub line_no: i64,
    /// The row as the bank printed it — the answer to «where did this come from».
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct DebtLink {
    pub counterparty: String,
    pub kind: String,
    pub amount: Money,
}

#[derive(Debug, Clone)]
pub struct OperationDetail {
    /// An own note: what the bank did not write.
    pub note: Option<String>,
    pub line: OperationLine,
    pub role: String,
    pub state: String,
    pub category_source: Option<CategorySource>,
    /// The second leg of a transfer between one's own accounts.
    pub pair: Option<OperationLine>,
    pub source: Option<ImportSource>,
    pub debt: Option<DebtLink>,
    /// The split across categories, if there is one.
    pub split: Vec<(String, Money)>,
}

pub fn operation(conn: &Connection, id: i64) -> Result<Option<OperationDetail>> {
    let Some(line) = query::operation_by_id(conn, id)? else { return Ok(None) };
    let (role, state, own, inherited, pair_id, import_row_id): (
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "select o.role, o.state, o.category_id, p.category_id, o.pair_id, o.import_row_id
           from operation o left join counterparty p on p.id = o.counterparty_id
          where o.id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    let category_source = match (own, inherited) {
        (Some(_), _) => Some(CategorySource::Own),
        (None, Some(_)) => Some(CategorySource::Counterparty),
        (None, None) => None,
    };
    let pair = match pair_id {
        Some(pair) => query::operation_by_id(conn, pair)?,
        None => None,
    };
    let source = match import_row_id {
        Some(row) => conn
            .query_row(
                "select f.bank, f.filename, r.line_no, r.raw
                   from import_row r join import_file f on f.id = r.file_id where r.id = ?1",
                [row],
                |r| {
                    Ok(ImportSource {
                        bank: r.get(0)?,
                        filename: r.get(1)?,
                        line_no: r.get(2)?,
                        raw: r.get(3)?,
                    })
                },
            )
            .optional()?,
        None => None,
    };
    let debt = conn
        .query_row(
            "select p.name, m.kind, m.amount_minor, d.currency
               from debt_movement m
               join debt d on d.id = m.debt_id
               join counterparty p on p.id = d.counterparty_id
              where m.operation_id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)),
        )
        .optional()?
        .map(|(counterparty, kind, amount, currency)| -> Result<DebtLink> {
            Ok(DebtLink { counterparty, kind, amount: Money::new(amount, Currency::new(&currency)?) })
        })
        .transpose()?;
    let currency = line.amount.currency();
    let split = crate::edit::split_of(conn, id)?.into_iter().map(|(c, a)| (c, Money::new(a, currency))).collect();
    let note: Option<String> = conn.query_row("select note from operation where id = ?1", [id], |r| r.get(0)).optional()?.flatten();
    Ok(Some(OperationDetail { note, line, role, state, category_source, pair, source, debt, split }))
}

// ── An account ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AccountDetail {
    pub name: String,
    pub kind: String,
    pub state: String,
    pub inclusion: String,
    pub opening: Money,
    pub balance: Money,
    pub operations: usize,
    pub first: Option<String>,
    pub last: Option<String>,
    pub recent: Vec<OperationLine>,
    /// The last reconciliation with the bank, recomputed by the current operations.
    pub reconciliation: Option<crate::reconcile::Reconciliation>,
    /// The pending rows of this account's last statement — as the bank printed
    /// them. They are not in the balance; they arrive completed in the next one.
    pub pending: Vec<String>,
}

pub fn account(conn: &Connection, id: i64) -> Result<Option<AccountDetail>> {
    let found = conn
        .query_row(
            "select a.name, a.kind, a.state, a.inclusion, a.currency, a.opening_minor, b.balance_minor,
                    (select count(*) from operation o where o.account_id = a.id and o.state = 'POSTED'),
                    (select min(date(occurred_at)) from operation where account_id = a.id and state = 'POSTED'),
                    (select max(date(occurred_at)) from operation where account_id = a.id and state = 'POSTED')
               from account a join account_balance b on b.account_id = a.id
              where a.id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?;
    let Some((name, kind, state, inclusion, currency, opening, balance, operations, first, last)) = found
    else {
        return Ok(None);
    };
    let currency = Currency::new(&currency)?;
    let mut recent = query::operations(conn, Some(id), false)?;
    recent.truncate(RECENT);
    Ok(Some(AccountDetail {
        name,
        kind,
        state,
        inclusion,
        opening: Money::new(opening, currency),
        balance: Money::new(balance, currency),
        operations: operations as usize,
        first,
        last,
        recent,
        reconciliation: crate::reconcile::last(conn, id)?,
        pending: {
            let mut stmt = conn.prepare(
                "select r.raw from import_row r
                  where r.pending = 1
                    and r.file_id = (select max(id) from import_file where account_id = ?1)
                  order by r.line_no",
            )?;
            let rows = stmt.query_map([id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        },
    }))
}

// ── A counterparty ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CounterpartyDetail {
    pub name: String,
    pub category: Option<String>,
    /// A note by hand — what this place actually is.
    pub note: Option<String>,
    /// The commonest category of its operations, when it has none itself.
    pub hint: Option<(String, usize)>,
    /// Every key that leads to it. It shows why it is one rather than five.
    pub aliases: Vec<String>,
    pub operations: usize,
    pub spent: MoneyBag,
    pub received: MoneyBag,
    pub first: Option<String>,
    pub last: Option<String>,
    pub recent: Vec<OperationLine>,
}

pub fn counterparty(conn: &Connection, name: &str) -> Result<Option<CounterpartyDetail>> {
    let Some(line) = query::counterparties(conn)?.into_iter().find(|l| l.name == name) else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "select a.alias from counterparty_alias a join counterparty p on p.id = a.counterparty_id
          where p.name = ?1 order by a.alias",
    )?;
    let aliases = stmt
        .query_map([name], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let all = query::search_operations(
        conn,
        &OperationFilter { counterparty: Some(name.to_string()), limit: usize::MAX, ..Default::default() },
    )?
    .operations;
    let mut received = MoneyBag::new();
    for op in &all {
        if op.role == "income" && op.amount.minor() > 0 {
            received.add(op.amount);
        }
    }
    let first = all.last().map(|op| op.occurred_on.clone());
    let last = all.first().map(|op| op.occurred_on.clone());
    let note: Option<String> =
        conn.query_row("select note from counterparty where name = ?1", [name], |r| r.get(0)).optional()?.flatten();
    Ok(Some(CounterpartyDetail {
        name: line.name,
        category: line.category,
        note,
        hint: line.hint,
        aliases,
        operations: line.operations,
        spent: line.spent,
        received,
        first,
        last,
        recent: all.into_iter().take(RECENT).collect(),
    }))
}

// ── A debt ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MovementLine {
    pub id: i64,
    pub operation_id: Option<i64>,
    /// Whether it enters the remainder. It does not if its operation is removed.
    pub counts: bool,
    pub occurred_on: String,
    pub kind: String,
    pub amount: Money,
    pub note: Option<String>,
    /// The description of the operation the movement is tied to — if it is.
    pub operation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DebtDetail {
    pub counterparty: String,
    pub direction: String,
    pub issued: Money,
    pub remaining: Money,
    pub opened_on: String,
    pub due_on: Option<String>,
    /// The ribbon of movements, from the first to the last.
    pub movements: Vec<MovementLine>,
}

pub fn debt(conn: &Connection, id: i64) -> Result<Option<DebtDetail>> {
    let found = conn
        .query_row(
            "select p.name, d.direction, d.currency, r.issued_minor, r.remaining_minor, d.opened_on, d.due_on
               from debt d
               join counterparty p on p.id = d.counterparty_id
               join debt_remaining r on r.debt_id = d.id
              where d.id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((counterparty, direction, currency, issued, remaining, opened_on, due_on)) = found else {
        return Ok(None);
    };
    let currency = Currency::new(&currency)?;
    let mut stmt = conn.prepare(
        "select m.id, m.operation_id, coalesce(o.state, 'POSTED') = 'POSTED',
                m.occurred_on, m.kind, m.amount_minor, m.note, o.description
           from debt_movement m left join operation o on o.id = m.operation_id
          where m.debt_id = ?1
          order by m.occurred_on, m.id",
    )?;
    let movements = stmt
        .query_map(params![id], |r| {
            Ok(MovementLine {
                id: r.get(0)?,
                operation_id: r.get(1)?,
                counts: r.get(2)?,
                occurred_on: r.get(3)?,
                kind: r.get(4)?,
                amount: Money::new(r.get(5)?, currency),
                note: r.get(6)?,
                operation: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(DebtDetail {
        counterparty,
        direction,
        issued: Money::new(issued, currency),
        remaining: Money::new(remaining, currency),
        opened_on,
        due_on,
        movements,
    }))
}

// ── A flow row ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FlowDetail {
    /// Who is inside the row and for how much, the large on top.
    pub counterparties: Vec<(String, MoneyBag, usize)>,
    pub recent: Vec<OperationLine>,
    pub operations: usize,
}

pub fn flow(conn: &Connection, label: &str, from: &str, to: &str) -> Result<FlowDetail> {
    let all = query::flow_operations(conn, label, from, to)?;
    let mut by: std::collections::BTreeMap<String, (MoneyBag, usize)> = Default::default();
    for op in &all {
        let who = op.counterparty.clone().unwrap_or_else(|| op.description.clone());
        let entry = by.entry(who).or_default();
        let split_minor: Option<i64> = conn.query_row(
            "select sum(s.amount_minor) from operation_split s join category c on c.id = s.category_id
              where s.operation_id = ?1 and c.name = ?2",
            params![op.id, label],
            |r| r.get(0),
        )?;
        let effect = match split_minor {
            Some(part) => if op.amount.minor() < 0 { -part } else { part },
            None => op.amount.minor(),
        };
        if effect < 0 || op.role == "expense" {
            entry.0.add(Money::new(-effect, op.amount.currency()));
        }
        entry.1 += 1;
    }
    let mut counterparties: Vec<(String, MoneyBag, usize)> =
        by.into_iter().map(|(name, (bag, count))| (name, bag, count)).collect();
    // The order is by spend in the main currency; a sort, not a sum to show.
    counterparties.sort_by_key(|(name, bag, _)| {
        let spent = crate::rate::rank_bag_minor(conn, bag, to);
        (-spent, name.clone())
    });
    let operations = all.len();
    Ok(FlowDetail { counterparties, recent: all.into_iter().take(RECENT).collect(), operations })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('Revolut','EUR','BANK',10000), ('Cash','EUR','CASH',0);
             insert into category (name) values ('Food');
             insert into counterparty (name, category_id) values ('Kaufland', 1), ('Pat', null);
             insert into counterparty_alias (alias, counterparty_id) values ('kaufland', 1), ('kaufland bulgaria', 1);
             insert into import_file (sha256, bank, filename, archived_at) values ('s', 'revolut', 'sept.pdf', 'a');
             insert into import_row (file_id, line_no, raw, signature) values (1, 42, 'Sep 1, 2026 Kaufland €6.24 €93.76', 'x');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,import_row_id) values
               (1,1,'expense','2026-09-01T00:00:00Z',624,'EUR',-624,'Kaufland',1,1),
               (2,1,'transfer','2026-09-02T00:00:00Z',5000,'EUR',-5000,'To cash',null,null),
               (3,2,'transfer','2026-09-02T00:00:00Z',5000,'EUR',5000,'From revolut',null,null),
               (4,1,'debt_movement','2026-09-03T00:00:00Z',3000,'EUR',3000,'Borrowed from Pat',2,null);
             update operation set pair_id = 3 where id = 2;
             update operation set pair_id = 2 where id = 3;
             insert into debt (counterparty_id,direction,currency,opened_on,due_on) values (2,'I_OWE','EUR','2026-09-03','2026-12-01');
             insert into debt_movement (debt_id,operation_id,kind,amount_minor,occurred_on) values
               (1,4,'ISSUE',3000,'2026-09-03'), (1,null,'REPAYMENT',1000,'2026-09-20');",
        )
        .unwrap();
        conn
    }

    /// The answer to «where did this come from»: the file, the row, the bank's line.
    #[test]
    fn an_operation_says_where_it_came_from() {
        let detail = operation(&ledger(), 1).unwrap().unwrap();
        let source = detail.source.unwrap();
        assert_eq!((source.bank.as_str(), source.filename.as_str(), source.line_no), ("revolut", "sept.pdf", 42));
        assert!(source.raw.contains("€6.24"));
        assert_eq!(detail.category_source, Some(CategorySource::Counterparty), "the category comes from the counterparty");
    }

    #[test]
    fn a_transfer_shows_its_other_leg() {
        let detail = operation(&ledger(), 2).unwrap().unwrap();
        let pair = detail.pair.unwrap();
        assert_eq!(pair.account, "Cash");
        assert_eq!(pair.amount.minor(), 5000);
    }

    #[test]
    fn an_operation_tied_to_a_debt_says_which() {
        let detail = operation(&ledger(), 4).unwrap().unwrap();
        let debt = detail.debt.unwrap();
        assert_eq!((debt.counterparty.as_str(), debt.kind.as_str(), debt.amount.minor()), ("Pat", "ISSUE", 3000));
    }

    #[test]
    fn an_account_knows_its_span_and_recent_rows() {
        let detail = account(&ledger(), 1).unwrap().unwrap();
        assert_eq!(detail.operations, 3);
        assert_eq!(detail.first.as_deref(), Some("2026-09-01"));
        assert_eq!(detail.last.as_deref(), Some("2026-09-03"));
        assert_eq!(detail.balance.minor(), 10000 - 624 - 5000 + 3000);
        assert_eq!(detail.recent[0].description, "Borrowed from Pat", "the new on top");
    }

    /// Every key that leads to it — it shows why it is one rather than two.
    #[test]
    fn a_counterparty_shows_every_spelling_that_leads_to_it() {
        let detail = counterparty(&ledger(), "Kaufland").unwrap().unwrap();
        assert_eq!(detail.aliases, ["kaufland", "kaufland bulgaria"]);
        assert_eq!(detail.operations, 1);
        assert_eq!(detail.recent.len(), 1);
    }

    /// A debt's ribbon in order, with the tied operation where there is one.
    #[test]
    fn a_debt_is_a_timeline_of_its_movements() {
        let detail = debt(&ledger(), 1).unwrap().unwrap();
        assert_eq!(detail.remaining.minor(), 2000);
        assert_eq!(detail.due_on.as_deref(), Some("2026-12-01"));
        let kinds: Vec<_> = detail.movements.iter().map(|m| m.kind.as_str()).collect();
        assert_eq!(kinds, ["ISSUE", "REPAYMENT"]);
        assert_eq!(detail.movements[0].operation.as_deref(), Some("Borrowed from Pat"));
        assert_eq!(detail.movements[1].operation, None);
    }

    #[test]
    fn a_flow_breaks_down_by_who() {
        let detail = flow(&ledger(), "Food", "2026-09-01", "2026-09-30").unwrap();
        assert_eq!(detail.operations, 1);
        assert_eq!(detail.counterparties[0].0, "Kaufland");
        assert_eq!(detail.counterparties[0].2, 1);
    }

    #[test]
    fn flow_and_counterparty_detail_respect_splits_and_refunds() {
        let mut conn = ledger();
        crate::edit::split(&mut conn, 1, &[("Food".into(), 400), ("Home".into(), 224)]).unwrap();
        let food = flow(&conn, "Food", "2026-09-01", "2026-09-30").unwrap();
        assert_eq!(food.counterparties[0].1.in_display_order(None)[0].minor(), 400, "only the split part for Food");
        let home = flow(&conn, "Home", "2026-09-01", "2026-09-30").unwrap();
        assert_eq!(home.counterparties[0].1.in_display_order(None)[0].minor(), 224, "only the split part for Home");

        // A refund on Kaufland reduces spent and is not counted as received income.
        conn.execute(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
             values (10,1,'expense','2026-09-04T00:00:00Z',100,'EUR',100,'Kaufland refund',1)",
            [],
        )
        .unwrap();
        let cp = counterparty(&conn, "Kaufland").unwrap().unwrap();
        assert_eq!(cp.spent.in_display_order(None)[0].minor(), 524, "6.24 - 1.00 = 5.24");
        assert!(cp.received.is_empty(), "a refund is not received income");
    }

    #[test]
    fn something_that_is_not_there_is_none_not_an_error() {
        let conn = ledger();
        assert!(operation(&conn, 999).unwrap().is_none());
        assert!(account(&conn, 999).unwrap().is_none());
        assert!(counterparty(&conn, "Nobody").unwrap().is_none());
        assert!(debt(&conn, 999).unwrap().is_none());
    }
}
