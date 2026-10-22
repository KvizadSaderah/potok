//! Parsing a statement before writing: what is new, what is there, what looks like what.
//!
//! Until 2026-10-18 an import wrote every new row as a spend or income, and
//! that was all. Topping up Revolut from iCard arrived as two rows strangers to
//! each other, the 720 refund on the Northwind debt as an ordinary spend, and
//! tying them up was by hand, one at a time, every month. The import in saldo
//! did it before writing, and a person saw what would be written. The same
//! here: `plan` writes nothing, a person accepts or rejects the hints, `store::apply` writes.
//!
//! Hints are not decisions. Two fees on one day, a transfer to a friend that is
//! no debt — all of that happens, and the last word is a person's.
use std::path::Path;

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};

use super::store::{self, Account};
use super::Statement;
use crate::query::{self, OperationLine};
use crate::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    New,
    /// The same signature is in the ledger already, from another file.
    Duplicate,
    /// Pending: it is not money (invariant 6).
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// The transfer's second leg is already on another account.
    Transfer { with: OperationLine },
    /// The same counterparty has paid on this debt, or it is their debt.
    Debt { debt_id: i64, counterparty: String, kind: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    /// A date later than today. Not written by default.
    Future,
    /// Much larger than usual for the account, and this counterparty has
    /// received or sent no such sums. Or, for a counterparty paid regularly, three
    /// times its usual — the subscription that charged a year at once.
    Large,
    /// A transfer to or from a person with whom there is no debt. May be a debt.
    Person,
    /// The same operation has already been entered by hand on this account — the
    /// same day or next to it, the same amount. Not written by default: else a duplicate.
    ByHand,
}

#[derive(Debug, Clone)]
pub struct PlannedRow {
    pub line_no: usize,
    pub occurred_on: NaiveDate,
    pub description: String,
    /// The counterparty the description already means, when known: «KAUFLAND»
    /// rather than iCard's «POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR».
    pub who: Option<String>,
    pub amount: Money,
    pub status: RowStatus,
    pub hint: Option<Hint>,
    /// Accept the hint when writing.
    pub accept_hint: bool,
    pub flags: Vec<Flag>,
    /// Write the row. Means something for new rows only.
    pub write: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub account_id: i64,
    pub account: String,
    pub sha: String,
    /// This file is loaded already — there is nothing to write.
    pub already_imported: bool,
    /// Row `i` is row `i` of the statement.
    pub rows: Vec<PlannedRow>,
}

impl Plan {
    pub fn count(&self, status: RowStatus) -> usize {
        self.rows.iter().filter(|r| r.status == status).count()
    }

    /// The new rows that will be written.
    pub fn to_write(&self) -> usize {
        self.rows.iter().filter(|r| r.status == RowStatus::New && r.write).count()
    }

    /// The rows with an accepted hint of this kind that will be written.
    pub fn accepted(&self, transfer: bool) -> usize {
        self.rows
            .iter()
            .filter(|r| r.status == RowStatus::New && r.write && r.accept_hint)
            .filter(|r| matches!(r.hint, Some(Hint::Transfer { .. })) == transfer && r.hint.is_some())
            .count()
    }
}

/// The account a statement asks for: the one named as the bank. Failing that —
/// let a person choose.
pub fn account_for(conn: &Connection, statement: &Statement) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "select name from account where lower(name) = lower(?1) and currency = ?2 and state = 'ACTIVE'",
            params![statement.bank.as_str(), statement.currency.as_str()],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn plan(conn: &Connection, account_name: &str, path: &Path, statement: &Statement, today: NaiveDate) -> Result<Plan> {
    let Account { id: account_id, currency } = store::find_account(conn, account_name)?;
    if currency != statement.currency {
        anyhow::bail!(crate::tr!(
            "the statement is in {}, the account {account_name:?} in {currency} — invariant 4 forbids mixing",
            "выписка в {}, а счёт {account_name:?} в {currency} — инвариант 4 не разрешает смешивать",
            statement.currency
        ));
    }
    let sha = store::sha256_of(path)?;
    let already_imported: bool =
        conn.query_row("select exists (select 1 from import_file where sha256 = ?1)", [&sha], |r| r.get(0))?;

    let typical = typical_amount(conn, account_id)?;
    let mut used: Vec<i64> = Vec::new();
    let mut rows = Vec::new();
    for row in &statement.rows {
        let status = if !row.posted {
            RowStatus::Pending
        } else if store::seen_elsewhere(conn, &store::signature(account_id, row), None)? {
            RowStatus::Duplicate
        } else {
            RowStatus::New
        };
        let effect = row.amount.minor();
        let mut hint = None;
        let mut flags = Vec::new();
        // Moves inside the account and refunds have no other leg and no debt
        // to look for.
        let refund = effect > 0 && crate::counterparty::refund_of(&row.description).is_some();
        if status == RowStatus::New && !refund && !super::moves_within_the_account(statement.bank, &row.description) {
            let on = row.occurred_on.to_string();
            let partner = crate::edit::partners(conn, account_id, currency.as_str(), effect, &on)?
                .into_iter()
                .find(|id| !used.contains(id));
            if let Some(id) = partner {
                used.push(id);
                if let Some(with) = query::operation_by_id(conn, id)? {
                    hint = Some(Hint::Transfer { with });
                }
            }
            let counterparty = crate::counterparty::find(conn, &row.description)?;
            if hint.is_none() {
                if let Some(cp) = counterparty {
                    hint = debt_hint(conn, cp, currency.as_str(), effect)?;
                }
            }
            if row.occurred_on > today {
                flags.push(Flag::Future);
            }
            if is_large(conn, typical, counterparty, effect)? {
                flags.push(Flag::Large);
            }
            if hint.is_none() && looks_like_person(&row.description) {
                flags.push(Flag::Person);
            }
            let by_hand: bool = conn.query_row(
                "select exists (select 1 from operation
                  where account_id = ?1 and effect_minor = ?2 and state = 'POSTED' and import_row_id is null
                    and abs(julianday(date(occurred_at)) - julianday(date(?3))) <= 1)",
                params![account_id, effect, on],
                |r| r.get(0),
            )?;
            if by_hand {
                flags.push(Flag::ByHand);
            }
        }
        rows.push(PlannedRow {
            line_no: row.line_no,
            occurred_on: row.occurred_on,
            description: row.description.clone(),
            who: match crate::counterparty::find(conn, &row.description)? {
                Some(id) => conn.query_row("select name from counterparty where id = ?1", [id], |r| r.get(0)).ok(),
                None => None,
            },
            amount: row.amount,
            status,
            accept_hint: hint.is_some(),
            write: status == RowStatus::New && !flags.contains(&Flag::Future) && !flags.contains(&Flag::ByHand),
            hint,
            flags,
        });
    }
    Ok(Plan { account_id, account: account_name.to_string(), sha, already_imported, rows })
}

/// The debt a row asks for: this counterparty's debt, or a debt this
/// counterparty has already paid on. The second is not for beauty: the
/// Northwind debt in the real ledger has the counterparty «Northwind» while its
/// payments have «NORTHWIND» from the statement, and the first rule would miss them.
fn debt_hint(conn: &Connection, counterparty: i64, currency: &str, effect: i64) -> Result<Option<Hint>> {
    let found: Option<(i64, String, String)> = conn
        .query_row(
            "select d.id, c.name, d.direction
               from debt d
               join counterparty c on c.id = d.counterparty_id
               join debt_remaining r on r.debt_id = d.id
              where d.currency = ?2
                and (d.counterparty_id = ?1
                     or exists (select 1 from debt_movement m join operation o on o.id = m.operation_id
                                 where m.debt_id = d.id and o.counterparty_id = ?1))
                and (not ((d.direction = 'I_OWE' and ?3 < 0) or (d.direction = 'OWED_TO_ME' and ?3 > 0))
                     or r.remaining_minor >= abs(?3))
              order by r.remaining_minor > 0 desc, d.opened_on desc
              limit 1",
            params![counterparty, currency, effect],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(found.map(|(debt_id, counterparty, direction)| Hint::Debt {
        debt_id,
        counterparty,
        kind: crate::debt::kind_for(&direction, effect),
    }))
}

/// A usual amount for the account: the median of the absolute value over the last year.
fn typical_amount(conn: &Connection, account_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "select abs(effect_minor) from operation
          where account_id = ?1 and state = 'POSTED' and effect_minor <> 0
            and date(occurred_at) >= date('now', '-365 days')
          order by abs(effect_minor)",
    )?;
    let amounts = stmt.query_map([account_id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(amounts.get(amounts.len() / 2).copied())
}

// «Large» is one of two things. By the account: twenty times the median and not
// less than 500, and the counterparty has seen no such sums; rent that arrives
// every month is not marked. By the counterparty: it has three payments or more
// over the year, three quarters of them within 10% of the median, and this one
// three times it and not less than 20 — Netflix charging a year ahead is not
// large for the account but is for itself. Without «the same amount» the rule
// marked 74 ordinary purchases in Billa and Lidl on the real history; with it, none.
fn is_large(conn: &Connection, typical: Option<i64>, counterparty: Option<i64>, effect: i64) -> Result<bool> {
    let amount = effect.abs();
    if let Some(cp) = counterparty {
        let mut stmt = conn.prepare(
            "select abs(effect_minor) from operation
              where counterparty_id = ?1 and state = 'POSTED' and effect_minor <> 0
                and date(occurred_at) >= date('now', '-365 days')
              order by abs(effect_minor)",
        )?;
        let own = stmt.query_map([cp], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if own.len() >= 3 {
            let usual = own[own.len() / 2];
            let steady = own.iter().filter(|a| (*a - usual).abs() * 10 <= usual).count() * 4 >= own.len() * 3;
            if steady && amount >= 2_000 && amount > usual * 3 {
                return Ok(true);
            }
        }
    }
    let Some(typical) = typical else { return Ok(false) };
    if amount < (typical * 20).max(50_000) {
        return Ok(false);
    }
    let Some(cp) = counterparty else { return Ok(true) };
    let seen: bool = conn.query_row(
        "select exists (select 1 from operation where counterparty_id = ?1 and state = 'POSTED'
                         and abs(effect_minor) * 2 >= ?2)",
        params![cp, amount],
        |r| r.get(0),
    )?;
    Ok(!seen)
}

/// A transfer to a person in Revolut: «Transfer to/from NAME». iCard's internal
/// transfers also start with «Transfer to», but an account number follows, not a name.
fn looks_like_person(description: &str) -> bool {
    let lower = description.to_lowercase();
    ["transfer to ", "transfer from "].iter().any(|prefix| {
        lower
            .strip_prefix(prefix)
            .and_then(|rest| rest.chars().next())
            .is_some_and(|c| c.is_alphabetic())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, import};

    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind,opening_minor) values (1,'iCard','EUR','CARD',100000), (2,'Revolut','EUR','BANK',0);
             insert into counterparty (id,name) values (1,'Northwind'), (2,'NORTHWIND');
             insert into counterparty_alias (alias,counterparty_id) values ('northwind bulgaria eood',2);
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'expense','2026-10-16T00:00:00Z',1000,'EUR',-1000,'Revolut** Dublin',null),
               (2,1,'debt_movement','2026-09-03T00:00:00Z',72000,'EUR',-72000,'NORTHWIND BULGARIA EOOD',2);
             insert into debt (id,counterparty_id,direction,currency,opened_on) values (1,1,'I_OWE','EUR','2026-01-30');
             insert into debt_movement (debt_id,operation_id,kind,amount_minor,occurred_on) values
               (1,null,'ISSUE',900000,'2026-01-30'), (1,2,'REPAYMENT',72000,'2026-09-03');",
        )
        .unwrap();
        conn
    }

    /// A counterparty paid the same every month: three times that is large
    /// for it, though nothing for the account. A shop paid anything is not.
    #[test]
    fn three_times_a_regular_payment_is_large() {
        let conn = ledger();
        conn.execute_batch(
            "insert into counterparty (id,name) values (9,'Netflix');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
             select 1,'expense',datetime('now', -n || ' days'),1299,'EUR',-1299,'Netflix',9
               from (select 10 n union all select 40 union all select 70);",
        )
        .unwrap();
        let typical = typical_amount(&conn, 1).unwrap();
        assert!(is_large(&conn, typical, Some(9), -1299 * 12).unwrap(), "a year at once");
        assert!(!is_large(&conn, typical, Some(9), -1499).unwrap(), "a price rise is not");
        assert!(!is_large(&conn, typical, None, -1299 * 12).unwrap(), "nobody to compare with, and small for the account");
        conn.execute_batch(
            "insert into counterparty (id,name) values (10,'Billa');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
             select 1,'expense',datetime('now', -n || ' days'),a,'EUR',-a,'Billa',10
               from (select 5 n, 300 a union all select 6, 900 union all select 7, 640 union all select 8, 2100);",
        )
        .unwrap();
        assert!(!is_large(&conn, typical, Some(10), -3000).unwrap(), "groceries vary");
    }

    /// A Revolut statement: a top-up from iCard, a transfer to a friend, a pending row.
    const REVOLUT: &str = "\
EUR Statement
Revolut Bank UAB
 Account (Current Account) €0.00 €0.00 €10.00 €10.00
Date Description Money out Money in Balance
Oct 16, 2026 Apple Pay deposit by *9012 €10.00 €10.00
Oct 17, 2026 Transfer to Mila Stoeva €5.00 €5.00
Oct 17, 2026 Transfer from Mila Stoeva €5.00 €10.00
 Start date Description Money out Money in
Oct 18, 2026 Hosting €6.60
";

    fn file(name: &str, text: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("potok-review-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_top_up_finds_its_other_leg_and_a_friend_is_flagged() {
        let conn = ledger();
        let statement = import::parse(REVOLUT).unwrap();
        let path = file("revolut.txt", REVOLUT);
        assert_eq!(account_for(&conn, &statement).unwrap().as_deref(), Some("Revolut"));
        let plan = plan(&conn, "Revolut", &path, &statement, day("2026-10-18")).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(matches!(&plan.rows[0].hint, Some(Hint::Transfer { with }) if with.id == 1));
        assert!(plan.rows[0].accept_hint && plan.rows[0].write);
        assert_eq!(plan.rows[1].flags, [Flag::Person]);
        assert_eq!(plan.rows[3].status, RowStatus::Pending);
        assert!(!plan.rows[3].write);
        assert_eq!(plan.to_write(), 3);
        assert_eq!(plan.accepted(true), 1);
    }

    #[test]
    fn a_repayment_is_matched_to_the_debt_through_its_past_payments() {
        let conn = ledger();
        let hint = debt_hint(&conn, 2, "EUR", -72000).unwrap();
        assert_eq!(hint, Some(Hint::Debt { debt_id: 1, counterparty: "Northwind".into(), kind: "REPAYMENT" }));
        let more = debt_hint(&conn, 2, "EUR", 10000).unwrap();
        assert!(matches!(more, Some(Hint::Debt { kind: "ISSUE", .. })), "it came from the one owed — borrowed more");
        assert_eq!(debt_hint(&conn, 2, "USD", -72000).unwrap(), None, "another currency is another debt");
    }

    #[test]
    fn a_date_after_today_is_not_written_by_default() {
        let conn = ledger();
        let statement = import::parse(REVOLUT).unwrap();
        let path = file("future.txt", REVOLUT);
        let plan = plan(&conn, "Revolut", &path, &statement, day("2026-10-16")).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(plan.rows[1].flags[0], Flag::Future);
        assert!(!plan.rows[1].write);
    }

    /// Write by the plan: the top-up as a transfer from iCard, a rejected row
    /// not written; the rollback gives iCard its spend back and removes the rest.
    #[test]
    fn the_plan_is_written_as_decided_and_rolled_back_whole() {
        let mut conn = ledger();
        let statement = import::parse(REVOLUT).unwrap();
        let path = file("apply.txt", REVOLUT);
        let archive = std::env::temp_dir().join(format!("potok-review-archive-{}", std::process::id()));
        let mut plan = plan(&conn, "Revolut", &path, &statement, day("2026-10-18")).unwrap();
        plan.rows[2].write = false; // «Transfer from Mila» — do not write
        let outcome = store::apply(&mut conn, &path, &statement, &plan, &archive).unwrap();
        assert_eq!((outcome.operations_created, outcome.transfers_paired, outcome.declined), (2, 1, 1));
        let pair: Option<i64> = conn.query_row("select pair_id from operation where id = 1", [], |r| r.get(0)).unwrap();
        assert!(pair.is_some(), "the leg on iCard is tied");
        assert!(crate::verify::run(&conn).unwrap().ok());

        let file_id: i64 = conn.query_row("select max(id) from import_file", [], |r| r.get(0)).unwrap();
        let rolled = store::rollback(&mut conn, file_id).unwrap();
        assert_eq!((rolled.operations, rolled.partners_unpaired), (2, 1));
        let (role, pair): (String, Option<i64>) =
            conn.query_row("select role, pair_id from operation where id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((role.as_str(), pair), ("expense", None));
        assert!(crate::verify::run(&conn).unwrap().ok());

        // after the rollback the same file loads again
        let again = plan_again(&conn, &path, &statement);
        assert!(!again.already_imported);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&archive);
    }

    fn plan_again(conn: &Connection, path: &Path, statement: &Statement) -> Plan {
        plan(conn, "Revolut", path, statement, day("2026-10-18")).unwrap()
    }

    #[test]
    fn a_debt_payment_is_written_as_a_movement_and_the_debt_moves() {
        let mut conn = ledger();
        // an iCard statement with one Northwind refund
        let statement = Statement {
            bank: import::Bank::Icard,
            currency: crate::Currency::new("EUR").unwrap(),
            rows: vec![import::ParsedRow {
                line_no: 1,
                occurred_on: day("2026-10-03"),
                description: "NORTHWIND BULGARIA EOOD".into(),
                amount: Money::new(-72000, crate::Currency::new("EUR").unwrap()),
                running_balance: None,
                posted: true,
                raw: "raw".into(),
            }],
        };
        let path = file("icard.txt", "icard october");
        let archive = std::env::temp_dir().join(format!("potok-review-archive2-{}", std::process::id()));
        let before: i64 = conn.query_row("select remaining_minor from debt_remaining", [], |r| r.get(0)).unwrap();
        let plan = plan(&conn, "iCard", &path, &statement, day("2026-10-18")).unwrap();
        assert!(matches!(plan.rows[0].hint, Some(Hint::Debt { kind: "REPAYMENT", .. })));
        let outcome = store::apply(&mut conn, &path, &statement, &plan, &archive).unwrap();
        assert_eq!(outcome.debt_movements, 1);
        let after: i64 = conn.query_row("select remaining_minor from debt_remaining", [], |r| r.get(0)).unwrap();
        assert_eq!(before - after, 72000);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&archive);
    }

    #[test]
    fn the_saldo_migration_is_never_rolled_back() {
        let mut conn = ledger();
        conn.execute(
            "insert into import_file (sha256,bank,filename,archived_at) values ('x','saldo-migration','dump','—')",
            [],
        )
        .unwrap();
        assert!(store::rollback(&mut conn, 1).is_err());
    }

    /// Coffee entered by hand arrives in a statement later — not written twice.
    #[test]
    fn a_row_already_entered_by_hand_is_not_written_by_default() {
        let conn = ledger();
        conn.execute_batch(
            "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description)
             values (2,'expense','2026-10-17T00:00:00Z',500,'EUR',-500,'Mila')",
        )
        .unwrap();
        let statement = import::parse(REVOLUT).unwrap();
        let path = file("byhand.txt", REVOLUT);
        let plan = plan(&conn, "Revolut", &path, &statement, day("2026-10-18")).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(plan.rows[1].flags.contains(&Flag::ByHand), "{:?}", plan.rows[1]);
        assert!(!plan.rows[1].write);
    }

    #[test]
    fn people_are_told_apart_from_account_numbers() {
        assert!(looks_like_person("Transfer to Mila Stoeva"));
        assert!(looks_like_person("Transfer from LUCA ROSSI Reference: x"));
        assert!(!looks_like_person("Transfer to 91017809780 / EUR from 20177318399"));
        assert!(!looks_like_person("Apple Pay deposit"));
    }
}
