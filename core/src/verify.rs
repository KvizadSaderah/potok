//! `potok verify` — the §3 invariants checked against the database.
//!
//! Only what the schema does not hold itself is checked here. Invariants 1 and
//! 2 (an account's balance and a debt's remainder) hold by construction: they
//! are not stored but computed by views, so nothing can disagree — that was the
//! point. Invariants 4, 5 and 6 are held by a trigger, a `check` and a filter.
//!
//! What is left is what only a query expresses: a transfer's symmetry, a debt's
//! non-negativity, the pairing of a reversal, an import row's uniqueness.
use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The invariant's number from §3 of the specification.
    pub invariant: u8,
    pub what: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub checked: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.findings.is_empty()
    }
}

pub fn run(conn: &Connection) -> Result<Report> {
    let mut report = Report::default();

    check(
        conn,
        &mut report,
        3,
        crate::tr!("a transfer has exactly two legs", "перевод — ровно две ноги"),
        &[crate::tr!("transfer leg without its partner", "нога перевода без встречной")],
        // A leg whose partner does not point back, or points elsewhere.
        //
        // `a.pair_id <> a.id` here is no ornament: a leg paired with itself
        // passes the symmetry check — it is its own partner. A test caught that,
        // and the condition was added, because one operation cannot be a
        // transfer by definition.
        "select a.id, ?1 from operation a
           where (a.pair_id is not null
                  and (a.pair_id = a.id
                       or not exists (select 1 from operation b
                                       where b.id = a.pair_id and b.pair_id = a.id)))
              or (a.role = 'transfer' and a.pair_id is null)",
    )?;

    check(
        conn,
        &mut report,
        3,
        crate::tr!("transfer legs are opposite and equal", "ноги перевода противоположны и равны"),
        &[crate::tr!("transfer legs do not cancel out", "ноги перевода не гасят друг друга")],
        "select a.id, ?1 from operation a
           join operation b on b.id = a.pair_id
          where a.id < b.id
            and (a.account_id = b.account_id
                 or a.role <> b.role
                 or a.role not in ('transfer', 'passthrough')
                 or (a.currency = b.currency and a.effect_minor + b.effect_minor <> 0)
                 or (a.currency <> b.currency and (a.effect_minor >= 0) = (b.effect_minor >= 0)))",
    )?;

    check(
        conn,
        &mut report,
        4,
        crate::tr!("operation currency equals account currency", "валюта операции равна валюте счёта"),
        &[crate::tr!("currency ", "валюта "), crate::tr!(" on an account in ", " при счёте в ")],
        "select o.id, ?1 || o.currency || ?2 || a.currency
           from operation o join account a on a.id = o.account_id
          where o.currency <> a.currency",
    )?;

    check(
        conn,
        &mut report,
        2,
        crate::tr!("a debt remainder is not negative", "остаток долга не отрицателен"),
        &[crate::tr!("the remainder went below zero: ", "остаток ушёл ниже нуля: ")],
        "select debt_id, ?1 || printf('-%d.%02d', abs(remaining_minor) / 100, abs(remaining_minor) % 100)
           from debt_remaining where remaining_minor < 0",
    )?;

    check(
        conn,
        &mut report,
        8,
        crate::tr!("a transfer is removed whole, not one leg", "перевод убран целиком, а не одной ногой"),
        &[crate::tr!("a transfer leg is removed and the other is not", "нога перевода убрана, а вторая осталась")],
        // A removed operation is a mark, not a counter-entry (invariant 8).
        // There is one way to break that: remove one leg of a transfer and leave
        // the other — then the money left one account and arrived nowhere.
        "select o.id, ?1
           from operation o join operation b on b.id = o.pair_id
          where o.id < b.id and o.state <> b.state",
    )?;

    check(
        conn,
        &mut report,
        7,
        crate::tr!("one import row, one operation", "одна строка импорта — одна операция"),
        &[crate::tr!("more than one operation points at the import row", "на строку импорта ссылается больше одной операции")],
        "select import_row_id, ?1
           from operation where import_row_id is not null
          group by import_row_id having count(*) > 1",
    )?;

    check(
        conn,
        &mut report,
        1,
        crate::tr!("split parts add up to the operation", "части разбивки дают сумму операции"),
        &[crate::tr!("split parts do not match the operation", "разбивка не сходится с операцией")],
        "select o.id, ?1 from operation o
          where exists (select 1 from operation_split s where s.operation_id = o.id)
            and (o.role not in ('income', 'expense')
                 or (select sum(s.amount_minor) from operation_split s where s.operation_id = o.id) <> o.amount_minor)",
    )?;

    check(
        conn,
        &mut report,
        2,
        crate::tr!("debt movement matches its operation", "движение долга совпадает со своей операцией"),
        &[crate::tr!("debt movement disagrees with its operation", "движение долга расходится с операцией")],
        "select m.id, ?1 from debt_movement m
           join debt d on d.id = m.debt_id
           join operation o on o.id = m.operation_id
          where m.amount_minor <> o.amount_minor
             or d.currency <> o.currency
             or o.role <> 'debt_movement'",
    )?;

    Ok(report)
}

fn check(
    conn: &Connection,
    report: &mut Report,
    invariant: u8,
    label: String,
    words: &[String],
    sql: &str,
) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(words), |row| {
        let id: i64 = row.get(0)?;
        let what: String = row.get(1)?;
        Ok((id, what))
    })?;
    for row in rows {
        let (id, what) = row?;
        report.findings.push(Finding { invariant, what: format!("#{id}: {what}") });
    }
    report.checked.push(label);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn with_account() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into account (name,currency,kind) values ('B','EUR','BANK');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn an_empty_ledger_has_nothing_to_report() {
        let report = run(&db::open_memory().unwrap()).unwrap();
        assert!(report.ok(), "{:?}", report.findings);
        assert_eq!(report.checked.len(), 8, "there must be eight checks");
    }

    #[test]
    fn broken_splits_and_mismatched_debt_movements_are_reported() {
        let conn = with_account();
        conn.execute_batch(
            "insert into category (id,name) values (1,'Food');
             insert into counterparty (id,name) values (1,'Pat');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,1,'expense','2026-09-01T00:00:00Z',5000,'EUR',-5000),
                        (2,1,'debt_movement','2026-09-01T00:00:00Z',3000,'EUR',-3000);
             insert into operation_split (operation_id,category_id,amount_minor) values (1,1,4000);
             insert into debt (id,counterparty_id,direction,currency,opened_on) values (1,1,'OWED_TO_ME','EUR','2026-09-01');
             insert into debt_movement (id,debt_id,operation_id,kind,amount_minor,occurred_on)
                 values (1,1,2,'ISSUE',9999,'2026-09-01');",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert!(report.findings.iter().any(|f| f.invariant == 1 && f.what.contains("#1")), "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.invariant == 2 && f.what.contains("#1")), "{:?}", report.findings);
        assert!(
            conn.execute("update operation set currency = 'USD' where id = 1", []).is_err(),
            "the trigger holds the currency on an update too"
        );
    }

    /// Invariant 3: a transfer's leg with no counter-leg is a mistake, not an operation.
    #[test]
    fn a_transfer_leg_without_its_partner_is_reported() {
        let conn = with_account();
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,1,'transfer','2026-09-01T00:00:00Z',5000,'EUR',-5000);
             update operation set pair_id = 1 where id = 1;",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert!(!report.ok());
        assert!(report.findings.iter().any(|f| f.invariant == 3), "{:?}", report.findings);
    }

    /// Invariant 3: two legs must cancel each other. A pair that does not cancel
    /// means money appeared or vanished.
    #[test]
    fn two_legs_that_do_not_cancel_are_reported() {
        let conn = with_account();
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,1,'transfer','2026-09-01T00:00:00Z',5000,'EUR',-5000),
                        (2,2,'transfer','2026-09-01T00:00:00Z',4000,'EUR', 4000);
             update operation set pair_id = 2 where id = 1;
             update operation set pair_id = 1 where id = 2;",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert!(report.findings.iter().any(|f| f.invariant == 3), "{:?}", report.findings);
    }

    /// A properly paired transfer must not turn up among the findings — else the
    /// check makes noise and people stop reading it.
    #[test]
    fn a_well_formed_transfer_is_silent() {
        let conn = with_account();
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,1,'transfer','2026-09-01T00:00:00Z',5000,'EUR',-5000),
                        (2,2,'transfer','2026-09-01T00:00:00Z',5000,'EUR', 5000);
             update operation set pair_id = 2 where id = 1;
             update operation set pair_id = 1 where id = 2;",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert!(report.ok(), "{:?}", report.findings);
    }

    /// Invariant 2: a debt's remainder cannot go below zero — more was given
    /// back than was taken.
    #[test]
    fn a_debt_repaid_past_zero_is_reported() {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into counterparty (name) values ('Someone');
             insert into debt (counterparty_id,direction,currency,opened_on)
                 values (1,'I_OWE','EUR','2026-09-01');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'ISSUE',10000,'2026-09-01');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'REPAYMENT',15000,'2026-09-20');",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert!(report.findings.iter().any(|f| f.invariant == 2), "{:?}", report.findings);
    }

    /// Invariant 8: a removed operation is a mark, and that is no breach. A
    /// breach is a transfer removed by one leg.
    #[test]
    fn a_removed_duplicate_is_fine_and_half_a_removed_transfer_is_not() {
        let conn = with_account();
        conn.execute_batch(
            "insert into operation (account_id,role,state,occurred_at,amount_minor,currency,effect_minor)
                 values (1,'expense','REVERSED','2026-09-01T00:00:00Z',5000,'EUR',-5000);",
        )
        .unwrap();
        assert!(run(&conn).unwrap().ok(), "a removed duplicate with no counter-entry is by design");
        conn.execute_batch(
            "insert into operation (id,account_id,role,state,occurred_at,amount_minor,currency,effect_minor)
                 values (10,1,'transfer','REVERSED','2026-09-02T00:00:00Z',100,'EUR',-100),
                        (11,2,'transfer','POSTED','2026-09-02T00:00:00Z',100,'EUR',100);
             update operation set pair_id = 11 where id = 10;
             update operation set pair_id = 10 where id = 11;",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert!(report.findings.iter().any(|f| f.invariant == 8), "{:?}", report.findings);
    }

    #[test]
    fn unpaired_transfer_or_same_account_transfer_or_wrong_debt_role_is_reported() {
        let conn = with_account();
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,1,'transfer','2026-09-01T00:00:00Z',5000,'EUR',-5000),
                        (2,1,'transfer','2026-09-02T00:00:00Z',1000,'EUR',-1000),
                        (3,1,'transfer','2026-09-02T00:00:00Z',1000,'EUR', 1000);
             update operation set pair_id = 3 where id = 2;
             update operation set pair_id = 2 where id = 3;",
        )
        .unwrap();
        let report = run(&conn).unwrap();
        assert_eq!(report.findings.iter().filter(|f| f.invariant == 3).count(), 2, "{:?}", report.findings);
    }
}
