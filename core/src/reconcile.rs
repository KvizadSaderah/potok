//! Reconciling with the bank and finding duplicates.
//!
//! Came about after 2026-10-18: the debit iCard showed −1 324.68, which does
//! not happen, and nobody noticed — iCard prints no balance in its statement,
//! so the import had nothing to check against. The cause was nine duplicates
//! from loading one statement twice in saldo; they were found by hand, row by
//! row against the PDF. The same thing here — by a key.
//!
//! Both things only show. A person fixes it, with the same edits from `edit`,
//! and a person decides: two identical fees on one day are sometimes real.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::query::{self, OperationLine};
use crate::{Currency, Money};

// ── Duplicates ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    /// The operation numbers comma-separated, ascending — the «not a duplicate» key.
    pub key: String,
    pub operations: Vec<OperationLine>,
}

impl DuplicateGroup {
    /// How much is surplus if one is kept: one's amount times the number of the
    /// others.
    pub fn extra(&self) -> Money {
        let one = self.operations[0].amount;
        Money::new(one.minor() * (self.operations.len() as i64 - 1), one.currency())
    }
}

/// Similar operations: one account, one day, one amount with the same sign, one
/// counterparty (and without one, the same description). The posted ones,
/// except the groups rejected as «not a duplicate». The new on top.
///
/// The role is deliberately not in the key: in the real case one copy of the
/// Revolut top-up was a transfer and the other a spend, and the role would miss them.
///
/// Every copy from one statement, on rows of its own, is not shown: the bank
/// printed the payment twice, so it was made twice — two tickets, two fees.
/// A double import is two files, or a statement and a hand-typed copy. The
/// saldo migration is not a statement: its rows are saldo's, doubles included.
pub fn duplicates(conn: &Connection, account_id: Option<i64>) -> Result<Vec<DuplicateGroup>> {
    let mut stmt = conn.prepare(
        "select group_concat(id, ',') from (
            select o.id, o.account_id, date(o.occurred_at) d, o.effect_minor,
                   coalesce(cast(o.counterparty_id as text), lower(trim(o.description))) who,
                   case when f.bank <> 'saldo-migration' then r.file_id end file
              from operation o
              left join import_row  r on r.id = o.import_row_id
              left join import_file f on f.id = r.file_id
             where o.state = 'POSTED' and (?1 is null or o.account_id = ?1)
             order by o.id)
          group by account_id, d, effect_minor, who
         having count(*) > 1
            and not (count(file) = count(*) and count(distinct file) = 1)
         order by d desc, min(id) desc",
    )?;
    let keys = stmt.query_map([account_id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut groups = Vec::new();
    for key in keys {
        let dismissed: bool = conn.query_row(
            "select exists (select 1 from duplicate_dismissed where operation_ids = ?1)",
            [&key],
            |r| r.get(0),
        )?;
        if dismissed {
            continue;
        }
        let mut operations = Vec::new();
        for id in key.split(',') {
            let id: i64 = id.parse().context("operation number in a group")?;
            if let Some(line) = query::operation_by_id(conn, id)? {
                operations.push(line);
            }
        }
        groups.push(DuplicateGroup { key, operations });
    }
    Ok(groups)
}

/// «This is not a duplicate»: do not show this group again.
pub fn dismiss_duplicates(conn: &Connection, key: &str) -> Result<()> {
    crate::db::atomic(conn, || {
        conn.execute("insert or ignore into duplicate_dismissed (operation_ids) values (?1)", [key])?;
        crate::journal::log(conn, &crate::tr!("«not a duplicate»: {key}", "«не дубль»: {key}"), Some(crate::journal::Undo::Undismiss(key.to_string())))?;
        Ok(())
    })
}

// ── Reconciling ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Reconciliation {
    pub account_id: i64,
    pub account: String,
    pub on_date: String,
    pub bank: Money,
    /// The balance by the operations at the end of that same day — recomputed now.
    pub ledger: Money,
    /// Minus is the bank here: plus means we have more than there really is.
    pub difference: Money,
    /// The surplus in this account's unrejected duplicates up to the date.
    pub duplicates_extra: Money,
    /// This account's operations up to the date whose sum equals the gap — the
    /// first thing to look at if the gap is not from duplicates.
    pub same_amount: Vec<OperationLine>,
}

impl Reconciliation {
    pub fn matches(&self) -> bool {
        self.difference.minor() == 0
    }

    /// What the balance would be if every similar operation were a duplicate:
    /// compare with the bank and see whether they explain the gap.
    pub fn without_duplicates(&self) -> Money {
        Money::new(self.ledger.minor() - self.duplicates_extra.minor(), self.ledger.currency())
    }
}

fn account_of(conn: &Connection, account_id: i64) -> Result<(String, Currency, i64)> {
    let (name, currency, opening): (String, String, i64) = conn
        .query_row(
            "select name, currency, opening_minor from account where id = ?1",
            [account_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .with_context(|| crate::tr!("no account #{account_id}", "нет счёта #{account_id}"))?;
    Ok((name, Currency::new(&currency)?, opening))
}

/// The account's balance by posted operations at the end of day `on`, inclusive.
pub fn balance_on(conn: &Connection, account_id: i64, on: &str) -> Result<Money> {
    let (_, currency, opening) = account_of(conn, account_id)?;
    let sum: i64 = conn.query_row(
        "select coalesce(sum(effect_minor), 0) from operation
          where account_id = ?1 and state = 'POSTED' and date(occurred_at) <= date(?2)",
        params![account_id, on],
        |r| r.get(0),
    )?;
    Ok(Money::new(opening + sum, currency))
}

/// Writes down that the bank showed `bank` at the end of day `on`, and checks at once.
pub fn reconcile(conn: &Connection, account_id: i64, on: &str, bank_minor: i64) -> Result<Reconciliation> {
    if chrono::NaiveDate::parse_from_str(on, "%Y-%m-%d").is_err() {
        bail!(crate::tr!("a reconciliation date is YYYY-MM-DD, not «{on}»", "дата сверки — ГГГГ-ММ-ДД, а не «{on}»"));
    }
    crate::db::atomic(conn, || {
        let (name, _, _) = account_of(conn, account_id)?;
        conn.execute(
            "insert into reconciliation (account_id, on_date, bank_minor) values (?1, ?2, ?3)",
            params![account_id, on, bank_minor],
        )?;
        crate::journal::log(
            conn,
            &crate::tr!(
                "reconciled {name} on {on}: the bank says {}",
                "сверка {name} на {on}: в банке {}",
                crate::money::format_minor(bank_minor)
            ),
            Some(crate::journal::Undo::DeleteReconciliation(conn.last_insert_rowid())),
        )?;
        compare(conn, account_id, on, bank_minor)
    })
}

/// The account's last reconciliation, recomputed by the operations as they now are.
pub fn last(conn: &Connection, account_id: i64) -> Result<Option<Reconciliation>> {
    let found: Option<(String, i64)> = conn
        .query_row(
            "select on_date, bank_minor from reconciliation where account_id = ?1
              order by on_date desc, id desc limit 1",
            [account_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match found {
        Some((on, bank)) => Ok(Some(compare(conn, account_id, &on, bank)?)),
        None => Ok(None),
    }
}

fn compare(conn: &Connection, account_id: i64, on: &str, bank_minor: i64) -> Result<Reconciliation> {
    let (account, currency, _) = account_of(conn, account_id)?;
    let ledger = balance_on(conn, account_id, on)?;
    let difference = ledger.minor() - bank_minor;

    let duplicates_extra = duplicates(conn, Some(account_id))?
        .iter()
        .filter(|group| group.operations[0].occurred_on.as_str() <= on)
        .map(|group| group.extra().minor())
        .sum::<i64>();

    let mut same_amount = Vec::new();
    if difference != 0 {
        let mut stmt = conn.prepare(
            "select id from operation
              where account_id = ?1 and state = 'POSTED' and date(occurred_at) <= date(?2)
                and abs(effect_minor) = ?3
              order by occurred_at desc limit 10",
        )?;
        let ids = stmt
            .query_map(params![account_id, on, difference.abs()], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            if let Some(line) = query::operation_by_id(conn, id)? {
                same_amount.push(line);
            }
        }
    }

    Ok(Reconciliation {
        account_id,
        account,
        on_date: on.to_string(),
        bank: Money::new(bank_minor, currency),
        ledger,
        difference: Money::new(difference, currency),
        duplicates_extra: Money::new(duplicates_extra, currency),
        same_amount,
    })
}

/// An amount as a person types it: `507.12`, `507,1`, `-1 324.68`, `507`.
///
/// Leans on `Money::parse` but rejects thousands grouped by a point or a comma
/// (`1.234`): in a bank statement that is one thousand two hundred thirty-four,
/// while in a field typed by hand it is a slip in the cents.
pub fn parse_input(text: &str) -> Result<i64> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace() && *c != '\u{a0}').collect();
    let clean = clean.replace('\u{2212}', "-").replace(',', ".");
    let body = clean
        .strip_prefix('-')
        .or_else(|| clean.strip_prefix('+'))
        .unwrap_or(clean.as_str());
    let valid_shape = !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit() || c == '.')
        && body.matches('.').count() <= 1
        && body.split_once('.').is_none_or(|(w, f)| !w.is_empty() && f.len() <= 2);
    if !valid_shape {
        bail!(crate::tr!("not an amount: «{text}» — something like 507.12", "не похоже на сумму: «{text}» — нужно вроде 507.12"));
    }
    let money = Money::parse(&clean, Currency::new("XXX").expect("XXX is a valid code"))
        .map_err(|_| anyhow::anyhow!(crate::tr!("not an amount: «{text}» — something like 507.12", "не похоже на сумму: «{text}» — нужно вроде 507.12")))?;
    Ok(money.minor())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, edit};

    /// The real case in miniature: topping up Revolut from iCard written twice
    /// — once as a transfer, once as a spend — and two real fees of 0.50 on one
    /// day.
    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',50000), ('Revolut','EUR','BANK',0);
             insert into counterparty (id,name) values (1,'Revolut'), (2,'iCard fee');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'transfer','2026-09-02T12:00:00Z',10000,'EUR',-10000,'Revolut** Dublin',1),
               (2,2,'transfer','2026-09-02T12:00:00Z',10000,'EUR', 10000,'Top-up',null),
               (3,1,'expense', '2026-09-02T12:00:00Z',10000,'EUR',-10000,'Revolut** Dublin, -100.00 EUR',1),
               (4,1,'expense', '2026-09-03T12:00:00Z',50,'EUR',-50,'Fee SEPA',2),
               (5,1,'expense', '2026-09-03T12:00:00Z',50,'EUR',-50,'Fee SEPA',2),
               (6,1,'expense', '2026-09-05T12:00:00Z',2000,'EUR',-2000,'Shop',null);
             update operation set pair_id = 2 where id = 1;
             update operation set pair_id = 1 where id = 2;",
        )
        .unwrap();
        conn
    }

    fn keys(groups: &[DuplicateGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.key.as_str()).collect()
    }

    #[test]
    fn copies_are_found_across_roles_and_only_on_one_account() {
        let conn = ledger();
        let groups = duplicates(&conn, None).unwrap();
        assert_eq!(keys(&groups), ["4,5", "1,3"], "the new on top; the leg on Revolut is not in the group");
        assert_eq!(groups[1].extra().minor(), -10000);
        assert!(duplicates(&conn, Some(2)).unwrap().is_empty());
    }

    /// Two fees on two rows of one statement were both charged; the same fee
    /// from two statements, or from saldo's rows, is still a question.
    #[test]
    fn copies_printed_by_one_statement_are_not_duplicates() {
        let conn = ledger();
        conn.execute_batch(
            "insert into import_file (id,sha256,bank,filename,archived_at) values
               (1,'a','icard','sept.pdf','x'), (2,'b','icard','sept-again.pdf','y'), (3,'c','saldo-migration','dump.sql','z');
             insert into import_row (id,file_id,line_no,raw,signature) values
               (1,1,1,'fee','s1'), (2,1,2,'fee','s2');
             update operation set import_row_id = 1 where id = 4;
             update operation set import_row_id = 2 where id = 5;",
        )
        .unwrap();
        assert_eq!(keys(&duplicates(&conn, None).unwrap()), ["1,3"], "the bank printed both fees");
        conn.execute_batch("update import_row set file_id = 2 where id = 2;").unwrap();
        assert_eq!(keys(&duplicates(&conn, None).unwrap()), ["4,5", "1,3"], "two files — a double import");
        conn.execute_batch(
            "update import_row set file_id = 3 where id in (1,2);",
        )
        .unwrap();
        assert_eq!(keys(&duplicates(&conn, None).unwrap()), ["4,5", "1,3"], "saldo's rows prove nothing");
    }

    #[test]
    fn a_dismissed_group_stays_hidden_until_it_changes() {
        let conn = ledger();
        dismiss_duplicates(&conn, "4,5").unwrap();
        assert_eq!(keys(&duplicates(&conn, None).unwrap()), ["1,3"]);
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
             values (7,1,'expense','2026-09-03T12:00:00Z',50,'EUR',-50,'Fee SEPA',2);",
        )
        .unwrap();
        assert_eq!(keys(&duplicates(&conn, None).unwrap()), ["4,5,7", "1,3"], "the third makes a new group");
    }

    /// Reconciling: the gap shows, a duplicate explains it, and it goes when removed.
    #[test]
    fn a_difference_is_explained_by_a_duplicate_and_goes_away_with_it() {
        let mut conn = ledger();
        // at the bank 500 − 100 − 0.50 − 0.50 − 20 = 379.00 on 5 September
        let first = reconcile(&conn, 1, "2026-09-05", 37900).unwrap();
        assert_eq!(first.ledger.minor(), 27900);
        assert_eq!(first.difference.minor(), -10000, "we are 100 short of the bank");
        assert_eq!(first.duplicates_extra.minor(), -10050);
        assert_eq!(first.without_duplicates().minor(), 37950, "the duplicate explains 100, the fee does not");
        assert_eq!(first.same_amount.iter().map(|l| l.id).collect::<Vec<_>>(), [3, 1]);

        edit::remove_operation(&mut conn, 3).unwrap();
        let now = last(&conn, 1).unwrap().unwrap();
        assert!(now.matches(), "{now:?}");
    }

    #[test]
    fn balance_is_taken_at_the_end_of_the_day() {
        let conn = ledger();
        assert_eq!(balance_on(&conn, 1, "2026-09-01").unwrap().minor(), 50000);
        assert_eq!(balance_on(&conn, 1, "2026-09-02").unwrap().minor(), 30000);
    }

    #[test]
    fn a_bad_date_or_account_is_refused() {
        let conn = ledger();
        assert!(reconcile(&conn, 1, "16.10.2026", 0).is_err());
        assert!(reconcile(&conn, 99, "2026-10-16", 0).is_err());
        assert!(last(&conn, 1).unwrap().is_none(), "the refusal wrote nothing");
    }

    #[test]
    fn typed_amounts_read_the_way_people_type_them() {
        assert_eq!(parse_input("507.12").unwrap(), 50712);
        assert_eq!(parse_input("507,1").unwrap(), 50710);
        assert_eq!(parse_input("507").unwrap(), 50700);
        assert_eq!(parse_input("-1 324.68").unwrap(), -132468);
        assert_eq!(parse_input("−5").unwrap(), -500);
        for bad in ["", "abc", "1.234", "5.0.1", "-"] {
            assert!(parse_input(bad).is_err(), "{bad}");
        }
    }
}
