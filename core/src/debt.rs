//! Debts by hand: open one, tie an operation to it, write it off.
//!
//! Until 2026-10-18 debts arrived only by the move from saldo, and a payment
//! could be untied but not tied. A friend sent 300 — the operation landed as
//! income, and the debt appeared nowhere. The import now hints at a payment on
//! a debt that already exists; here is everything else.
//!
//! A debt's remainder is still not stored (invariant 2): only the movements are
//! written here, and `debt_remaining` computes it.
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// The direction of a debt movement from the operation's sign. I owe: out — paid
/// back, in — borrowed more. Owed to me: out — lent more, in — paid back.
pub fn kind_for(direction: &str, effect_minor: i64) -> &'static str {
    match (direction, effect_minor < 0) {
        ("I_OWE", true) | ("OWED_TO_ME", false) => "REPAYMENT",
        _ => "ISSUE",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDebt {
    /// Who — becomes the debt's counterparty.
    pub who: String,
    /// `I_OWE` or `OWED_TO_ME`.
    pub direction: String,
    pub currency: String,
    pub opened_on: chrono::NaiveDate,
    pub due_on: Option<chrono::NaiveDate>,
    /// How much is owed already on the day it opens, with no operation in the
    /// ledger — an old debt, in cash, before the accounting began. Zero — it starts with one.
    pub opening_minor: i64,
    pub note: Option<String>,
}

/// Opens a debt. With `with_operation` that operation becomes its first lending
/// (a friend sent 300 — and that is the debt).
pub fn open(conn: &mut Connection, new: &NewDebt, with_operation: Option<i64>) -> Result<i64> {
    if !matches!(new.direction.as_str(), "I_OWE" | "OWED_TO_ME") {
        bail!(crate::tr!("direction is I_OWE or OWED_TO_ME, not «{}»", "кто кому — I_OWE или OWED_TO_ME, а не «{}»", new.direction));
    }
    crate::Currency::new(&new.currency)?;
    let who = new.who.trim();
    if who.is_empty() {
        bail!(crate::tr!("who the debt is with is empty", "с кем долг — пусто"));
    }
    if new.opening_minor < 0 {
        bail!(crate::tr!("a debt amount has no sign — the direction says who owes whom", "сумма долга — без знака: кто кому должен, говорит направление"));
    }
    if new.opening_minor == 0 && with_operation.is_none() {
        bail!(crate::tr!(
            "a debt with no amount and no operation is empty — give an amount or open it from an operation",
            "долг без суммы и без операции — пустой; назови сумму или открой его с операции"
        ));
    }
    let transaction = conn.transaction()?;
    let counterparty = crate::counterparty::resolve_or_create(&transaction, who)?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("«{who}» will not do as a name", "«{who}» не годится в имя")))?;
    let note = new.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    transaction.execute(
        "insert into debt (counterparty_id, direction, currency, opened_on, due_on, note)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            counterparty,
            new.direction,
            new.currency,
            new.opened_on.to_string(),
            new.due_on.map(|d| d.to_string()),
            note
        ],
    )?;
    let id = transaction.last_insert_rowid();
    if new.opening_minor > 0 {
        transaction.execute(
            "insert into debt_movement (debt_id, operation_id, kind, amount_minor, occurred_on, note)
             values (?1, null, 'ISSUE', ?2, ?3, ?4)",
            params![id, new.opening_minor, new.opened_on.to_string(), crate::tr!("as of the opening day", "на день открытия")],
        )?;
    }
    if let Some(operation) = with_operation {
        link_in(&transaction, id, operation, Some("ISSUE"))?;
    }
    let what = match with_operation {
        Some(op) => crate::tr!("from operation {}", "с операции {}", crate::journal::describe(&transaction, op)),
        None => crate::tr!("for {} {}", "на {} {}", crate::money::format_minor(new.opening_minor), new.currency),
    };
    let side = if new.direction == "I_OWE" { crate::trs!("I owe", "я должен") } else { crate::trs!("owed to me", "мне должны") };
    crate::journal::log(
        &transaction,
        &crate::tr!("debt opened «{who}» ({side}) {what}", "открыт долг «{who}» ({side}) {what}"),
        Some(crate::journal::Undo::DeleteDebt(id)),
    )?;
    transaction.commit()?;
    Ok(id)
}

/// Ties an operation to a debt. Returns the kind of movement: a lending or a
/// repayment, by the operation's sign and the debt's direction.
pub fn link(conn: &mut Connection, debt_id: i64, operation_id: i64) -> Result<&'static str> {
    let transaction = conn.transaction()?;
    let kind = link_in(&transaction, debt_id, operation_id, None)?;
    let movement = transaction.last_insert_rowid();
    let what = crate::journal::describe(&transaction, operation_id);
    crate::journal::log(
        &transaction,
        &if kind == "ISSUE" {
            crate::tr!("linked to debt #{debt_id} as an issue: {what}", "привязана к долгу #{debt_id} как выдача: {what}")
        } else {
            crate::tr!("linked to debt #{debt_id} as a repayment: {what}", "привязана к долгу #{debt_id} как возврат: {what}")
        },
        Some(crate::journal::Undo::Unlink(movement)),
    )?;
    transaction.commit()?;
    Ok(kind)
}

fn link_in(transaction: &Connection, debt_id: i64, operation_id: i64, kind: Option<&'static str>) -> Result<&'static str> {
    let (direction, currency, remaining): (String, String, i64) = transaction
        .query_row(
            "select d.direction, d.currency, r.remaining_minor
               from debt d join debt_remaining r on r.debt_id = d.id where d.id = ?1",
            [debt_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no debt #{debt_id}", "нет долга #{debt_id}")))?;
    let (state, role, op_currency, effect, on): (String, String, String, i64, String) = transaction
        .query_row(
            "select state, role, currency, effect_minor, date(occurred_at) from operation where id = ?1",
            [operation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{operation_id}", "нет операции #{operation_id}")))?;
    if state != "POSTED" {
        bail!(crate::tr!("the operation is removed — it cannot be linked to a debt", "операция убрана — к долгу её не привязать"));
    }
    match role.as_str() {
        "income" | "expense" => {}
        "transfer" => bail!(crate::tr!("this is a transfer between own accounts — it cannot be a debt", "это перевод между своими счетами — долгом он быть не может")),
        "debt_movement" => bail!(crate::tr!("the operation is already linked to a debt — unlink it first", "операция уже привязана к долгу — сначала отвязать")),
        other => bail!(crate::tr!("an operation with role {other} cannot be linked to a debt", "операцию с ролью {other} к долгу не привязать")),
    }
    if op_currency != currency {
        bail!(crate::tr!("the debt is in {currency}, the operation in {op_currency}", "долг в {currency}, а операция в {op_currency}"));
    }
    let has_split: bool = transaction.query_row(
        "select exists (select 1 from operation_split where operation_id = ?1)",
        [operation_id],
        |r| r.get(0),
    )?;
    if has_split {
        bail!(crate::tr!(
            "a split operation cannot be linked to a debt — clear its split first",
            "разбитую операцию к долгу не привязать — сначала снять разбивку"
        ));
    }
    let kind = kind.unwrap_or_else(|| kind_for(&direction, effect));
    if kind == "REPAYMENT" && effect.abs() > remaining {
        bail!(crate::tr!(
            "a repayment of {} is more than the {} left — wrong debt, or the debt has to grow first",
            "возврат {} больше остатка {} — это не тот долг, или долг надо сначала увеличить",
            crate::money::format_minor(effect.abs()),
            crate::money::format_minor(remaining)
        ));
    }
    transaction.execute("update operation set role = 'debt_movement' where id = ?1", [operation_id])?;
    transaction.execute(
        "insert into debt_movement (debt_id, operation_id, kind, amount_minor, occurred_on, note)
         values (?1, ?2, ?3, ?4, ?5, ?6)",
        params![debt_id, operation_id, kind, effect.abs(), on, crate::tr!("by hand", "руками")],
    )?;
    Ok(kind)
}

/// Writes the remainder off: forgiven, by them or by me. A movement with no
/// operation — there was no money. Returns what was written off.
pub fn write_off(conn: &Connection, debt_id: i64, on: chrono::NaiveDate) -> Result<i64> {
    crate::db::atomic(conn, || {
        let remaining: i64 = conn
            .query_row("select remaining_minor from debt_remaining where debt_id = ?1", [debt_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!(crate::tr!("no debt #{debt_id}", "нет долга #{debt_id}")))?;
        if remaining <= 0 {
            bail!(crate::tr!("the debt is already settled — nothing to write off", "долг уже закрыт — списывать нечего"));
        }
        conn.execute(
            "insert into debt_movement (debt_id, operation_id, kind, amount_minor, occurred_on, note)
             values (?1, null, 'WRITE_OFF', ?2, ?3, ?4)",
            params![debt_id, remaining, on.to_string(), crate::tr!("written off", "списано")],
        )?;
        crate::journal::log(
            conn,
            &crate::tr!(
                "wrote off the {} left on debt #{debt_id}",
                "списан остаток {} по долгу #{debt_id}",
                crate::money::format_minor(remaining)
            ),
            Some(crate::journal::Undo::DeleteMovement(conn.last_insert_rowid())),
        )?;
        Ok(remaining)
    })
}

/// What can be changed about a debt once it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebtFields {
    pub direction: String,
    pub due_on: Option<String>,
    pub note: Option<String>,
}

pub fn fields(conn: &Connection, debt_id: i64) -> Result<DebtFields> {
    conn.query_row("select direction, due_on, note from debt where id = ?1", [debt_id], |r| {
        Ok(DebtFields { direction: r.get(0)?, due_on: r.get(1)?, note: r.get(2)? })
    })
    .optional()?
    .ok_or_else(|| anyhow::anyhow!(crate::tr!("no debt #{debt_id}", "нет долга #{debt_id}")))
}

/// Change the direction, the due date or the note. Movements are not touched:
/// turning «I owe» into «owed to me» keeps their kinds, which is right when
/// the direction was simply chosen wrong at opening.
pub fn update(conn: &Connection, debt_id: i64, new: &DebtFields) -> Result<()> {
    if !matches!(new.direction.as_str(), "I_OWE" | "OWED_TO_ME") {
        bail!(crate::tr!("direction is I_OWE or OWED_TO_ME", "направление — I_OWE или OWED_TO_ME"));
    }
    crate::db::atomic(conn, || {
        let before = fields(conn, debt_id)?;
        let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        conn.execute(
            "update debt set direction = ?1, due_on = ?2, note = ?3 where id = ?4",
            params![new.direction, clean(&new.due_on), clean(&new.note), debt_id],
        )?;
        crate::journal::log(
            conn,
            &crate::tr!("debt #{debt_id} edited", "долг #{debt_id} изменён"),
            Some(crate::journal::Undo::EditDebt(debt_id, before.direction, before.due_on, before.note)),
        )?;
        Ok(())
    })
}

/// Deletes a debt whole — only as an undo of its opening. The operations tied
/// to it become an ordinary spend or income.
pub fn delete(conn: &mut Connection, debt_id: i64) -> Result<()> {
    let transaction = conn.transaction()?;
    let who: String = transaction
        .query_row(
            "select p.name from debt d join counterparty p on p.id = d.counterparty_id where d.id = ?1",
            [debt_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no debt #{debt_id}", "нет долга #{debt_id}")))?;
    transaction.execute(
        "update operation set role = case when effect_minor < 0 then 'expense' else 'income' end
          where role = 'debt_movement'
            and id in (select operation_id from debt_movement where debt_id = ?1)
            and id not in (select operation_id from debt_movement where debt_id <> ?1 and operation_id is not null)",
        [debt_id],
    )?;
    transaction.execute("delete from debt where id = ?1", [debt_id])?;
    crate::journal::log(&transaction, &crate::tr!("debt deleted «{who}»", "удалён долг «{who}»"), None)?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, query};

    fn day(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// A friend sent 300 to Revolut, then I gave 100 back.
    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'Revolut','EUR','BANK'), (2,'Tinkoff','RUB','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'income', '2026-10-01T00:00:00Z',30000,'EUR', 30000,'Transfer from Mila Stoeva'),
               (2,1,'expense','2026-10-10T00:00:00Z',10000,'EUR',-10000,'Transfer to Mila Stoeva'),
               (3,2,'expense','2026-10-10T00:00:00Z',10000,'RUB',-10000,'Olga'),
               (4,1,'transfer','2026-10-11T00:00:00Z',500,'EUR',-500,'to self'),
               (5,2,'transfer','2026-10-11T00:00:00Z',50000,'RUB',50000,'from self');
             update operation set pair_id = 5 where id = 4;
             update operation set pair_id = 4 where id = 5;",
        )
        .unwrap();
        conn
    }

    fn new(direction: &str, opening: i64) -> NewDebt {
        NewDebt {
            who: "Mila Stoeva".into(),
            direction: direction.into(),
            currency: "EUR".into(),
            opened_on: day("2026-10-01"),
            due_on: None,
            opening_minor: opening,
            note: None,
        }
    }

    fn remaining(conn: &Connection, id: i64) -> i64 {
        conn.query_row("select remaining_minor from debt_remaining where debt_id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn a_loan_starts_from_its_operation_and_is_paid_back() {
        let mut conn = ledger();
        let id = open(&mut conn, &new("I_OWE", 0), Some(1)).unwrap();
        assert_eq!(remaining(&conn, id), 30000);
        assert_eq!(link(&mut conn, id, 2).unwrap(), "REPAYMENT");
        assert_eq!(remaining(&conn, id), 20000);
        let roles: Vec<String> = query::operations(&conn, Some(1), false).unwrap().into_iter().map(|o| o.role).collect();
        assert_eq!(roles.iter().filter(|r| *r == "debt_movement").count(), 2);
        assert!(crate::verify::run(&conn).unwrap().ok());

        assert_eq!(write_off(&conn, id, day("2026-10-18")).unwrap(), 20000);
        assert_eq!(remaining(&conn, id), 0);
        assert!(write_off(&conn, id, day("2026-10-18")).is_err(), "a closed one cannot be written off");
    }

    #[test]
    fn an_old_debt_opens_with_an_amount_and_no_operation() {
        let mut conn = ledger();
        let id = open(&mut conn, &new("OWED_TO_ME", 50000), None).unwrap();
        assert_eq!(remaining(&conn, id), 50000);
        assert_eq!(link(&mut conn, id, 1).unwrap(), "REPAYMENT", "it came from the one who owes me — paid back");
        assert_eq!(remaining(&conn, id), 20000);
    }

    #[test]
    fn what_cannot_be_part_of_a_debt_is_refused() {
        let mut conn = ledger();
        assert!(open(&mut conn, &new("I_OWE", 0), None).is_err(), "an empty debt");
        assert!(open(&mut conn, &new("SIDEWAYS", 100), None).is_err());
        assert!(open(&mut conn, &NewDebt { currency: "EURO".into(), ..new("I_OWE", 100) }, None).is_err(), "not a currency code");
        assert!(open(&mut conn, &NewDebt { who: " ".into(), ..new("I_OWE", 100) }, None).is_err());
        let id = open(&mut conn, &new("I_OWE", 5000), None).unwrap();
        assert!(link(&mut conn, id, 2).is_err(), "a repayment of 100 exceeds the remainder of 50");
        assert!(link(&mut conn, id, 3).is_err(), "another currency");
        assert!(link(&mut conn, id, 4).is_err(), "a transfer between one's own");
        let big = open(&mut conn, &new("I_OWE", 50000), None).unwrap();
        link(&mut conn, big, 2).unwrap();
        assert!(link(&mut conn, id, 2).is_err(), "already tied");
        assert!(link(&mut conn, 99, 1).is_err());
    }

    #[test]
    fn a_debt_is_edited_and_the_edit_undone() {
        let mut conn = ledger();
        let id = open(&mut conn, &new("I_OWE", 5000), None).unwrap();
        update(&conn, id, &DebtFields { direction: "OWED_TO_ME".into(), due_on: Some("2026-12-31".into()), note: Some(" ok ".into()) }).unwrap();
        let now = fields(&conn, id).unwrap();
        assert_eq!((now.direction.as_str(), now.due_on.as_deref(), now.note.as_deref()), ("OWED_TO_ME", Some("2026-12-31"), Some("ok")));
        let entry = crate::journal::entries(&conn, 1).unwrap().remove(0);
        crate::journal::undo(&mut conn, entry.id).unwrap();
        assert_eq!(fields(&conn, id).unwrap().direction, "I_OWE");
    }

    #[test]
    fn the_kind_follows_the_direction_and_the_sign() {
        assert_eq!(kind_for("I_OWE", -1), "REPAYMENT");
        assert_eq!(kind_for("I_OWE", 1), "ISSUE");
        assert_eq!(kind_for("OWED_TO_ME", -1), "ISSUE");
        assert_eq!(kind_for("OWED_TO_ME", 1), "REPAYMENT");
    }
}
