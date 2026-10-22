//! The journal of edits: what was changed by hand, when, and how to undo it.
//!
//! Every edit in the core — remove, bring back, tie, break, attach, correct,
//! open a debt, load a statement — writes a row here in the same transaction as
//! the edit itself. So the journal is complete wherever the edit came from: the
//! interface, the command line, something that does not exist yet.
//!
//! Undoing is not «put the table's rows back as they were» but the opposite
//! action through the same core functions: remove again, tie again. They check
//! what they checked at the edit and write to the journal themselves — so an
//! undo can be undone too. If something has changed since and the opposite
//! action no longer makes sense, it refuses with a reason rather than spoiling it.
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

/// How to undo an edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undo {
    Restore(i64),
    Remove(i64),
    Pair(i64, i64),
    Unpair(i64),
    OwnCategory(i64, Option<String>),
    /// A correction: remove the new one, bring the old one back.
    Replace { old: i64, new: i64 },
    Opening(i64, i64),
    DeleteDebt(i64),
    Unlink(i64),
    DeleteMovement(i64),
    /// Put a movement taken off a debt back as it was.
    Relink { debt: i64, operation: Option<i64>, kind: String, amount: i64, on: String, note: Option<String>, role: Option<String> },
    Undismiss(String),
    DeleteReconciliation(i64),
    CounterpartyCategory(String, Option<String>, Vec<i64>),
    /// Give a spelling back to whoever had it (`None` — nobody had it).
    Alias(String, Option<i64>),
    /// Put an operation's previous note back (`None` — there was none).
    OperationNote(i64, Option<String>),
    /// Put a counterparty's previous note back (`None` — there was none).
    Note(String, Option<String>),
    /// Give these operations their own category back — the one they were moved
    /// out of under the counterparty's rule.
    OwnCategories(Vec<i64>, String),
    Unimport(i64),
    /// Put the previous role back: a spend, income, «do not count» or transit.
    Role(i64, String),
    /// Rename it back: (the current name, the previous one).
    Rename(String, String),
    /// Give the spelling and the operations back to the previous counterparty.
    Respell { alias: String, counterparty: Option<i64>, alias_was: Option<i64>, operations: Vec<i64> },
    DeleteAccount(i64),
    RenameAccount(i64, String),
    /// Archive (`true`) or bring back (`false`).
    Archive(i64, bool),
    /// Put a debt's direction, due date and note back.
    EditDebt(i64, String, Option<String>, Option<String>),
    /// Put an operation's split back as it was (empty — no split).
    Split(i64, Vec<(String, i64)>),
    /// Rename a category back: (current name, previous).
    RenameCategory(String, String),
    /// Put a category's budget back (`None` — there was none).
    Budget(String, Option<i64>),
    /// Put a month's start back: (`YYYY-MM`, the corrected start or none).
    MonthStart(String, Option<String>),
}

const SEP: char = '\u{1f}';

impl Undo {
    fn encode(&self) -> String {
        let opt = |o: &Option<String>| o.clone().unwrap_or_default();
        let parts: Vec<String> = match self {
            Undo::Restore(id) => vec!["restore".into(), id.to_string()],
            Undo::Remove(id) => vec!["remove".into(), id.to_string()],
            Undo::Pair(a, b) => vec!["pair".into(), a.to_string(), b.to_string()],
            Undo::Unpair(a) => vec!["unpair".into(), a.to_string()],
            Undo::OwnCategory(id, c) => vec!["own-category".into(), id.to_string(), opt(c)],
            Undo::Replace { old, new } => vec!["replace".into(), old.to_string(), new.to_string()],
            Undo::Opening(id, minor) => vec!["opening".into(), id.to_string(), minor.to_string()],
            Undo::DeleteDebt(id) => vec!["delete-debt".into(), id.to_string()],
            Undo::Unlink(id) => vec!["unlink".into(), id.to_string()],
            Undo::DeleteMovement(id) => vec!["delete-movement".into(), id.to_string()],
            Undo::Relink { debt, operation, kind, amount, on, note, role } => vec![
                "relink".into(),
                debt.to_string(),
                operation.map(|o| o.to_string()).unwrap_or_default(),
                kind.clone(),
                amount.to_string(),
                on.clone(),
                opt(note),
                opt(role),
            ],
            Undo::Undismiss(key) => vec!["undismiss".into(), key.clone()],
            Undo::DeleteReconciliation(id) => vec!["delete-reconciliation".into(), id.to_string()],
            Undo::CounterpartyCategory(name, c, ops) => vec![
                "counterparty-category".into(),
                name.clone(),
                opt(c),
                ops.iter().map(|o| o.to_string()).collect::<Vec<_>>().join(","),
            ],
            Undo::OwnCategories(ops, category) => vec![
                "own-categories".into(),
                category.clone(),
                ops.iter().map(|o| o.to_string()).collect::<Vec<_>>().join(","),
            ],
            Undo::Alias(alias, was) => vec!["alias".into(), alias.clone(), was.map(|w| w.to_string()).unwrap_or_default()],
            Undo::OperationNote(id, was) => vec!["operation-note".into(), id.to_string(), opt(was)],
            Undo::Note(name, was) => vec!["note".into(), name.clone(), opt(was)],
            Undo::Unimport(id) => vec!["unimport".into(), id.to_string()],
            Undo::Role(id, role) => vec!["role".into(), id.to_string(), role.clone()],
            Undo::Rename(now, was) => vec!["rename".into(), now.clone(), was.clone()],
            Undo::DeleteAccount(id) => vec!["delete-account".into(), id.to_string()],
            Undo::RenameAccount(id, name) => vec!["rename-account".into(), id.to_string(), name.clone()],
            Undo::Archive(id, on) => vec!["archive".into(), id.to_string(), (*on as i64).to_string()],
            Undo::MonthStart(month, start) => vec!["month-start".into(), month.clone(), opt(start)],
            Undo::Budget(category, minor) => {
                vec!["budget".into(), category.clone(), minor.map(|m| m.to_string()).unwrap_or_default()]
            }
            Undo::RenameCategory(now, was) => vec!["rename-category".into(), now.clone(), was.clone()],
            Undo::Split(id, parts) => {
                let mut v = vec!["split".into(), id.to_string()];
                v.extend(parts.iter().map(|(c, a)| format!("{a}={c}")));
                v
            }
            Undo::EditDebt(id, direction, due, note) => {
                vec!["edit-debt".into(), id.to_string(), direction.clone(), opt(due), opt(note)]
            }
            Undo::Respell { alias, counterparty, alias_was, operations } => vec![
                "respell".into(),
                alias.clone(),
                counterparty.map(|c| c.to_string()).unwrap_or_default(),
                alias_was.map(|c| c.to_string()).unwrap_or_default(),
                operations.iter().map(|o| o.to_string()).collect::<Vec<_>>().join(","),
            ],
        };
        parts.join(&SEP.to_string())
    }

    fn decode(text: &str) -> Result<Undo> {
        let parts: Vec<&str> = text.split(SEP).collect();
        let n = |i: usize| -> Result<i64> {
            parts.get(i).context("short undo record")?.parse::<i64>().context("number in undo record")
        };
        let s = |i: usize| parts.get(i).map(|p| p.to_string()).unwrap_or_default();
        let o = |i: usize| parts.get(i).filter(|p| !p.is_empty()).map(|p| p.to_string());
        Ok(match parts[0] {
            "restore" => Undo::Restore(n(1)?),
            "remove" => Undo::Remove(n(1)?),
            "pair" => Undo::Pair(n(1)?, n(2)?),
            "unpair" => Undo::Unpair(n(1)?),
            "own-category" => Undo::OwnCategory(n(1)?, o(2)),
            "replace" => Undo::Replace { old: n(1)?, new: n(2)? },
            "opening" => Undo::Opening(n(1)?, n(2)?),
            "delete-debt" => Undo::DeleteDebt(n(1)?),
            "unlink" => Undo::Unlink(n(1)?),
            "delete-movement" => Undo::DeleteMovement(n(1)?),
            "relink" => Undo::Relink {
                debt: n(1)?,
                operation: o(2).map(|v| v.parse()).transpose()?,
                kind: s(3),
                amount: n(4)?,
                on: s(5),
                note: o(6),
                role: o(7),
            },
            "undismiss" => Undo::Undismiss(s(1)),
            "delete-reconciliation" => Undo::DeleteReconciliation(n(1)?),
            "counterparty-category" => Undo::CounterpartyCategory(
                s(1),
                o(2),
                s(3).split(',').filter(|v| !v.is_empty()).map(str::parse).collect::<Result<_, _>>()?,
            ),
            "own-categories" => Undo::OwnCategories(
                s(2).split(',').filter(|v| !v.is_empty()).map(str::parse).collect::<Result<_, _>>()?,
                s(1),
            ),
            "alias" => Undo::Alias(s(1), o(2).map(|v| v.parse()).transpose()?),
            "operation-note" => Undo::OperationNote(n(1)?, o(2)),
            "note" => Undo::Note(s(1), o(2)),
            "unimport" => Undo::Unimport(n(1)?),
            "role" => Undo::Role(n(1)?, s(2)),
            "rename" => Undo::Rename(s(1), s(2)),
            "delete-account" => Undo::DeleteAccount(n(1)?),
            "rename-account" => Undo::RenameAccount(n(1)?, s(2)),
            "archive" => Undo::Archive(n(1)?, n(2)? != 0),
            "edit-debt" => Undo::EditDebt(n(1)?, s(2), o(3), o(4)),
            "rename-category" => Undo::RenameCategory(s(1), s(2)),
            "budget" => Undo::Budget(s(1), o(2).map(|v| v.parse()).transpose()?),
            "month-start" => Undo::MonthStart(s(1), o(2)),
            "split" => Undo::Split(
                n(1)?,
                parts[2..]
                    .iter()
                    .map(|p| {
                        let (a, c) = p.split_once('=').context("split part")?;
                        Ok((c.to_string(), a.parse::<i64>()?))
                    })
                    .collect::<Result<_>>()?,
            ),
            "respell" => Undo::Respell {
                alias: s(1),
                counterparty: o(2).map(|v| v.parse()).transpose()?,
                alias_was: o(3).map(|v| v.parse()).transpose()?,
                operations: s(4).split(',').filter(|v| !v.is_empty()).map(str::parse).collect::<Result<_, _>>()?,
            },
            other => bail!("unknown undo «{other}»"),
        })
    }
}

/// Writes an edit down. Called inside the edit's own transaction.
pub(crate) fn log(conn: &Connection, what: &str, undo: Option<Undo>) -> Result<()> {
    conn.execute(
        "insert into change_log (what, undo) values (?1, ?2)",
        params![what, undo.map(|u| u.encode())],
    )?;
    Ok(())
}

/// An operation in words — for a journal row: number, date, account, amount, who.
pub(crate) fn describe(conn: &Connection, id: i64) -> String {
    match crate::query::operation_by_id(conn, id) {
        Ok(Some(op)) => format!(
            "#{} {} {} {} {}",
            op.id,
            op.occurred_on,
            op.account,
            op.amount,
            op.counterparty.as_deref().filter(|n| !n.is_empty()).unwrap_or(&op.description)
        ),
        _ => format!("#{id}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i64,
    pub at: String,
    pub what: String,
    pub can_undo: bool,
    pub undone_at: Option<String>,
}

/// The edits, the new on top.
/// The newest entry's number, 0 for none: what has changed since a moment is
/// whatever came after its number.
pub fn last_id(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("select coalesce(max(id), 0) from change_log", [], |r| r.get(0))?)
}

pub fn entries(conn: &Connection, limit: usize) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        // UTC is stored, local is shown: «when I did this» is read off one's
        // own clock.
        "select id, datetime(at, 'localtime'), what, undo is not null, datetime(undone_at, 'localtime')
           from change_log order by id desc limit ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(Entry { id: r.get(0)?, at: r.get(1)?, what: r.get(2)?, can_undo: r.get(3)?, undone_at: r.get(4)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Undoes an edit. Returns what was done.
pub fn undo(conn: &mut Connection, id: i64) -> Result<String> {
    let (what, undo, undone): (String, Option<String>, Option<String>) = conn
        .query_row("select what, undo, undone_at from change_log where id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?
        .with_context(|| crate::tr!("no change #{id}", "нет правки #{id}"))?;
    if undone.is_some() {
        bail!(crate::tr!("change #{id} is already undone", "правка #{id} уже отменена"));
    }
    let Some(undo) = undo else {
        bail!(crate::tr!("«{what}» cannot be undone — by design", "«{what}» не отменяется — так устроено"))
    };
    match Undo::decode(&undo)? {
        Undo::Restore(op) => {
            crate::edit::restore_operation(conn, op)?;
        }
        Undo::Remove(op) => {
            crate::edit::remove_operation(conn, op)?;
        }
        Undo::Pair(a, b) => crate::edit::pair_transfer(conn, a, b)?,
        Undo::Unpair(a) => crate::edit::unpair_transfer(conn, a)?,
        Undo::OwnCategory(op, category) => crate::edit::set_own_category(conn, op, category.as_deref())?,
        Undo::Replace { old, new } => {
            let transaction = conn.transaction()?;
            let (new_state, new_role, new_pair): (String, String, Option<i64>) = transaction
                .query_row("select state, role, pair_id from operation where id = ?1", [new], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?
                .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{new}", "нет операции #{new}")))?;
            if new_state != "POSTED" {
                bail!(crate::tr!("the operation is already removed", "операция уже убрана"));
            }
            if !matches!(new_role.as_str(), "income" | "expense") || new_pair.is_some() {
                bail!(crate::tr!(
                    "the new operation was linked to a transfer or debt — unlink it first",
                    "новая операция была привязана к переводу или долгу — сначала отвязать"
                ));
            }
            let old_state: String = transaction
                .query_row("select state from operation where id = ?1", [old], |r| r.get(0))
                .optional()?
                .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{old}", "нет операции #{old}")))?;
            if old_state != "REVERSED" {
                bail!(crate::tr!("the operation is not removed — nothing to restore", "операция не убрана — возвращать нечего"));
            }
            transaction.execute("update operation set state = 'REVERSED' where id = ?1", [new])?;
            transaction.execute("update operation set state = 'POSTED' where id = ?1", [old])?;
            let (was, now) = (describe(&transaction, new), describe(&transaction, old));
            log(
                &transaction,
                &crate::tr!("fixed {was} → {now}", "исправлена {was} → {now}"),
                Some(Undo::Replace { old: new, new: old }),
            )?;
            transaction.commit()?;
        }
        Undo::Opening(account, minor) => {
            crate::edit::set_opening(conn, account, minor)?;
        }
        Undo::DeleteDebt(debt) => crate::debt::delete(conn, debt)?,
        Undo::Unlink(movement) => {
            crate::edit::unlink_movement(conn, movement)?;
        }
        Undo::DeleteMovement(movement) => {
            let transaction = conn.transaction()?;
            let gone = transaction.execute("delete from debt_movement where id = ?1 and operation_id is null", [movement])?;
            if gone == 0 {
                bail!(crate::tr!("that debt movement is gone", "этого движения долга уже нет"));
            }
            log(&transaction, &crate::tr!("write-off undone (movement #{movement})", "отменено списание по долгу (движение #{movement})"), None)?;
            transaction.commit()?;
        }
        Undo::Relink { debt, operation, kind, amount, on, note, role } => {
            let transaction = conn.transaction()?;
            if let Some(op) = operation {
                let (state, pair): (String, Option<i64>) = transaction
                    .query_row("select state, pair_id from operation where id = ?1", [op], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?
                    .with_context(|| crate::tr!("the operation is gone", "операции уже нет"))?;
                if state != "POSTED" {
                    bail!(crate::tr!("the operation is removed — it cannot go back to the debt", "the operation is removed — it cannot go back to the debt"));
                }
                if pair.is_some() {
                    bail!(crate::tr!("the operation is now a transfer leg — split the pair first", "операция стала ногой перевода — сначала разорвать пару"));
                }
                let has_split: bool = transaction.query_row(
                    "select exists (select 1 from operation_split where operation_id = ?1)",
                    [op],
                    |r| r.get(0),
                )?;
                if has_split {
                    bail!(crate::tr!("the operation is now split — clear its split first", "операция теперь разбита — сначала снять разбивку"));
                }
                if let Some(role) = &role {
                    transaction.execute("update operation set role = ?1 where id = ?2", params![role, op])?;
                }
            }
            if kind == "REPAYMENT" {
                let remaining: i64 = transaction
                    .query_row("select remaining_minor from debt_remaining where debt_id = ?1", [debt], |r| r.get(0))
                    .optional()?
                    .with_context(|| crate::tr!("the debt is gone", "долга уже нет"))?;
                if amount > remaining {
                    bail!(crate::tr!(
                        "a repayment of {} is more than the {} left on debt #{debt}",
                        "возврат {} больше остатка {} по долгу #{debt}",
                        crate::money::format_minor(amount),
                        crate::money::format_minor(remaining)
                    ));
                }
            }
            transaction.execute(
                "insert into debt_movement (debt_id, operation_id, kind, amount_minor, occurred_on, note)
                 values (?1, ?2, ?3, ?4, ?5, ?6)",
                params![debt, operation, kind, amount, on, note],
            )?;
            let movement = transaction.last_insert_rowid();
            let who = operation.map(|op| describe(&transaction, op)).unwrap_or_else(|| crate::tr!("a movement with no operation", "движение без операции"));
            log(&transaction, &crate::tr!("linked to the debt again: {who}", "снова привязано к долгу: {who}"), Some(Undo::Unlink(movement)))?;
            transaction.commit()?;
        }
        Undo::Undismiss(key) => {
            crate::db::atomic(conn, || {
                conn.execute("delete from duplicate_dismissed where operation_ids = ?1", [&key])?;
                log(conn, &crate::tr!("show look-alikes {key} again", "снова показывать похожие {key}"), None)?;
                Ok(())
            })?;
        }
        Undo::DeleteReconciliation(rec) => {
            crate::db::atomic(conn, || {
                conn.execute("delete from reconciliation where id = ?1", [rec])?;
                log(conn, &crate::tr!("reconciliation #{rec} deleted", "удалена сверка #{rec}"), None)?;
                Ok(())
            })?;
        }
        Undo::CounterpartyCategory(name, category, operations) => {
            let transaction = conn.transaction()?;
            let current_category_id: Option<i64> = transaction
                .query_row("select category_id from counterparty where name = ?1", [&name], |r| r.get(0))
                .optional()?
                .flatten();
            let category_id: Option<i64> = match &category {
                Some(c) => transaction.query_row("select id from category where name = ?1", [c], |r| r.get(0)).optional()?,
                None => None,
            };
            transaction.execute("update counterparty set category_id = ?1 where name = ?2", params![category_id, name])?;
            if let Some(restored_cat) = current_category_id {
                for op in operations {
                    transaction.execute(
                        "update operation set category_id = ?1 where id = ?2 and category_id is null",
                        params![restored_cat, op],
                    )?;
                }
            }
            log(
                &transaction,
                &crate::tr!(
                    "category «{name}» is {} again",
                    "категория «{name}» снова {}",
                    category.unwrap_or_else(|| crate::tr!("unset", "не задана"))
                ),
                None,
            )?;
            transaction.commit()?;
        }
        Undo::OwnCategories(operations, category) => {
            let transaction = conn.transaction()?;
            let category_id: Option<i64> =
                transaction.query_row("select id from category where name = ?1", [&category], |r| r.get(0)).optional()?;
            let category_id = category_id.with_context(|| {
                crate::tr!("no category «{category}» any more", "категории «{category}» больше нет")
            })?;
            let mut back = 0usize;
            for op in &operations {
                back += transaction.execute(
                    "update operation set category_id = ?1 where id = ?2 and category_id is null",
                    params![category_id, op],
                )?;
            }
            log(&transaction, &crate::tr!("{back} operations are «{category}» again", "{back} операций снова «{category}»"), None)?;
            transaction.commit()?;
        }
        Undo::Alias(alias, was) => {
            crate::db::atomic(conn, || {
                match was {
                    Some(owner) => {
                        conn.execute("insert or replace into counterparty_alias (alias, counterparty_id) values (?1, ?2)", params![&alias, owner])?;
                    }
                    None => {
                        conn.execute("delete from counterparty_alias where alias = ?1", params![&alias])?;
                    }
                }
                log(conn, &crate::tr!("spelling «{alias}» put back", "написание «{alias}» возвращено"), None)?;
                Ok(())
            })?;
        }
        Undo::OperationNote(id, was) => {
            conn.execute("update operation set note = ?1 where id = ?2", params![was, id])?;
            log(conn, &crate::tr!("the note on #{id} is back", "заметка на #{id} возвращена"), None)?;
        }
        Undo::Note(name, was) => {
            crate::db::atomic(conn, || {
                conn.execute("update counterparty set note = ?1 where name = ?2", params![was, &name])?;
                log(conn, &crate::tr!("note on «{name}» put back", "заметка на «{name}» возвращена"), None)?;
                Ok(())
            })?;
        }
        Undo::Unimport(file) => {
            crate::import::store::rollback(conn, file)?;
        }
        Undo::Role(op, role) => {
            let counting = match role.as_str() {
                "excluded" => crate::edit::Counting::Excluded,
                "passthrough" => crate::edit::Counting::Passthrough,
                "refund" => crate::edit::Counting::Refund,
                _ => crate::edit::Counting::Counted,
            };
            crate::edit::set_counting(conn, op, counting)?;
        }
        Undo::Rename(now, was) => crate::counterparty::rename(conn, &now, &was)?,
        Undo::DeleteAccount(id) => crate::account::delete_empty(conn, id)?,
        Undo::RenameAccount(id, name) => crate::account::rename(conn, id, &name)?,
        Undo::Archive(id, on) => crate::account::set_archived(conn, id, on)?,
        Undo::Split(op, parts) => crate::edit::split(conn, op, &parts)?,
        Undo::RenameCategory(now, was) => crate::category::rename(conn, &now, &was)?,
        Undo::Budget(category, minor) => crate::budget::set(conn, &category, minor)?,
        Undo::MonthStart(month, start) => {
            let first = chrono::NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")?;
            let start = start.map(|s| chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")).transpose()?;
            crate::period::set_start(conn, (chrono::Datelike::year(&first), chrono::Datelike::month(&first)), start)?
        }
        Undo::EditDebt(id, direction, due_on, note) => {
            crate::debt::update(conn, id, &crate::debt::DebtFields { direction, due_on, note })?
        }
        Undo::Respell { alias, counterparty, alias_was, operations } => {
            let transaction = conn.transaction()?;
            for op in &operations {
                transaction.execute("update operation set counterparty_id = ?1 where id = ?2", params![counterparty, op])?;
            }
            match alias_was {
                Some(cp) => transaction.execute(
                    "insert or replace into counterparty_alias (alias, counterparty_id) values (?1, ?2)",
                    params![alias, cp],
                )?,
                None => transaction.execute("delete from counterparty_alias where alias = ?1", [&alias])?,
            };
            log(&transaction, &crate::tr!("spelling «{alias}» restored: {} operations", "написание «{alias}» возвращено: операций {}", operations.len()), None)?;
            transaction.commit()?;
        }
    }
    conn.execute("update change_log set undone_at = datetime('now') where id = ?1", [id])?;
    Ok(crate::tr!("undone: {what}", "отменено: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, debt, edit, reconcile};

    /// iCard and Revolut; a debt of 2200 with a 720 refund on it (#1); a
    /// transfer of 50 (#4↔#5); a spend of 12.50 (#6).
    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind,opening_minor) values (1,'iCard','EUR','CARD',100000), (2,'Revolut','EUR','BANK',0);
             insert into counterparty (id,name) values (1,'Northwind'), (2,'Kaufland');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'debt_movement','2026-09-03T00:00:00Z',72000,'EUR',-72000,'NORTHWIND',1),
               (4,1,'transfer','2026-09-04T00:00:00Z',5000,'EUR',-5000,'to revolut',null),
               (5,2,'transfer','2026-09-04T00:00:00Z',5000,'EUR',5000,'from icard',null),
               (6,1,'expense','2026-09-05T00:00:00Z',1250,'EUR',-1250,'Kaufland',2);
             update operation set pair_id = 5 where id = 4;
             update operation set pair_id = 4 where id = 5;
             insert into debt (id,counterparty_id,direction,currency,opened_on) values (1,1,'I_OWE','EUR','2026-01-30');
             insert into debt_movement (id,debt_id,operation_id,kind,amount_minor,occurred_on) values
               (1,1,null,'ISSUE',220000,'2026-01-30'), (2,1,1,'REPAYMENT',72000,'2026-09-03');",
        )
        .unwrap();
        conn
    }

    fn last(conn: &Connection) -> Entry {
        entries(conn, 1).unwrap().remove(0)
    }

    fn snapshot(conn: &Connection) -> String {
        conn.query_row(
            // What counts: an operation removed by a mark and a counterparty
            // created on the way with no category affect nothing.
            "select (select group_concat(id || role || coalesce(pair_id,'') || coalesce(category_id,''), ';') from operation where state = 'POSTED')
                 || '|' || (select group_concat(debt_id || kind || amount_minor || coalesce(operation_id,''), ';') from debt_movement)
                 || '|' || (select group_concat(id || opening_minor, ';') from account)
                 || '|' || (select count(*) from debt)
                 || '|' || coalesce((select group_concat(name || category_id, ';') from counterparty where category_id is not null), '')
                 || '|' || (select count(*) from duplicate_dismissed) || (select count(*) from reconciliation)",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Every edit is written, and undoing it returns the ledger to exactly what
    /// was there before it.
    #[test]
    fn every_edit_is_logged_and_undone_back_to_where_it_was() {
        let mut conn = ledger();
        type Edit = Box<dyn Fn(&mut Connection)>;
        let mut edits: Vec<(&str, Edit)> = vec![
            ("removed", Box::new(|c| { edit::remove_operation(c, 6).unwrap(); })),
            ("unlinked", Box::new(|c| { edit::unlink_movement(c, 2).unwrap(); })),
            ("transfer split", Box::new(|c| edit::unpair_transfer(c, 4).unwrap())),
            ("own category", Box::new(|c| edit::set_own_category(c, 6, Some("Food")).unwrap())),
            ("entered by hand", Box::new(|c| {
                edit::add_operation(c, &edit::NewOperation {
                    account_id: 1,
                    occurred_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                    effect_minor: -500,
                    who: "A bakery".into(),
                    category: None,
                }).unwrap();
            })),
            ("fixed", Box::new(|c| {
                edit::replace_operation(c, 6, &edit::NewOperation {
                    account_id: 1,
                    occurred_on: chrono::NaiveDate::from_ymd_opt(2026, 9, 5).unwrap(),
                    effect_minor: -125,
                    who: "Kaufland".into(),
                    category: None,
                }).unwrap();
            })),
            ("opening balance", Box::new(|c| { edit::set_opening(c, 1, 0).unwrap(); })),
            ("debt opened", Box::new(|c| {
                debt::open(c, &debt::NewDebt {
                    who: "Mila".into(),
                    direction: "OWED_TO_ME".into(),
                    currency: "EUR".into(),
                    opened_on: chrono::NaiveDate::from_ymd_opt(2026, 9, 5).unwrap(),
                    due_on: None,
                    opening_minor: 0,
                    note: None,
                }, Some(6)).unwrap();
            })),
            ("wrote off", Box::new(|c| { debt::write_off(c, 1, chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()).unwrap(); })),
            ("«not a duplicate»", Box::new(|c| reconcile::dismiss_duplicates(c, "1,2").unwrap())),
            ("reconciled", Box::new(|c| { reconcile::reconcile(c, 1, "2026-10-01", 1).unwrap(); })),
            ("category «Kaufland»", Box::new(|c| { crate::counterparty::set_category(c, "Kaufland", "Food").unwrap(); })),
            ("not counted", Box::new(|c| edit::set_counting(c, 6, edit::Counting::Excluded).unwrap())),
            ("renamed", Box::new(|c| crate::counterparty::rename(c, "Kaufland", "Kaufland corner").unwrap())),
            ("spelling", Box::new(|c| { crate::counterparty::reassign(c, 6, "Lidl").unwrap(); })),
            ("transit", Box::new(|c| edit::set_counting(c, 6, edit::Counting::Passthrough).unwrap())),
        ];
        for (label, apply) in edits.drain(..) {
            let before = snapshot(&conn);
            apply(&mut conn);
            let entry = last(&conn);
            assert!(entry.what.contains(label), "{label}: {}", entry.what);
            assert!(entry.can_undo, "{label}");
            undo(&mut conn, entry.id).unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(snapshot(&conn), before, "{label}: undo did not restore what was there");
            assert!(crate::verify::run(&conn).unwrap().ok(), "{label}");
        }
    }

    #[test]
    fn an_undo_is_done_once_and_can_itself_be_undone() {
        let mut conn = ledger();
        edit::remove_operation(&mut conn, 6).unwrap();
        let removal = last(&conn).id;
        undo(&mut conn, removal).unwrap();
        assert!(undo(&mut conn, removal).is_err(), "it does not undo twice");
        let restored = last(&conn);
        assert!(restored.what.starts_with("restored"), "{}", restored.what);
        undo(&mut conn, restored.id).unwrap();
        let state: String = conn.query_row("select state from operation where id = 6", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "REVERSED", "undoing the undo removes it again");
    }

    /// An opposite action that no longer makes sense refuses rather than
    /// spoiling the ledger.
    #[test]
    fn a_stale_undo_is_refused() {
        let mut conn = ledger();
        edit::unlink_movement(&mut conn, 2).unwrap();
        let unlink = last(&conn).id;
        edit::remove_operation(&mut conn, 1).unwrap();
        assert!(undo(&mut conn, unlink).is_err(), "the operation is removed — it cannot go back to the debt");
        assert!(entries(&conn, 10).unwrap().iter().find(|e| e.id == unlink).unwrap().undone_at.is_none());
    }

    fn counterparty_of(conn: &Connection, op: i64) -> Option<String> {
        conn.query_row(
            "select p.name from operation o left join counterparty p on p.id = o.counterparty_id where o.id = ?1",
            [op],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// A name given by hand survives a rebuild of the names; undo puts the
    /// previous one back.
    #[test]
    fn a_name_given_by_hand_survives_refresh_and_is_undone() {
        let mut conn = ledger();
        crate::counterparty::rename(&conn, "Kaufland", "Kaufland by the house").unwrap();
        crate::counterparty::refresh_names(&mut conn).unwrap();
        assert_eq!(counterparty_of(&conn, 6).as_deref(), Some("Kaufland by the house"));
        let taken = counterparty_of(&conn, 1).unwrap();
        assert!(crate::counterparty::rename(&conn, "Kaufland by the house", &taken).is_err(), "taken — fold them");
        // last in the journal stands the rebuild of names — the rename is undone
        let rename = entries(&conn, 10).unwrap().into_iter().find(|e| e.what.contains("renamed to")).unwrap().id;
        undo(&mut conn, rename).unwrap();
        assert_eq!(counterparty_of(&conn, 6).as_deref(), Some("Kaufland"));
    }

    /// «This is another counterparty»: every operation with the same spelling
    /// moves, and the next import knows it as the new one; undo puts it all back.
    #[test]
    fn a_spelling_moves_to_another_counterparty_with_its_operations() {
        let mut conn = ledger();
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (7,1,'expense','2026-09-06T00:00:00Z',300,'EUR',-300,'Kaufland',2),
               (8,1,'expense','2026-09-07T00:00:00Z',300,'EUR',-300,'Kaufland Mall',2);
             insert into counterparty_alias (alias,counterparty_id) values ('kaufland',2);",
        )
        .unwrap();
        let moved = crate::counterparty::reassign(&mut conn, 6, "Kaufland Sofia").unwrap();
        assert_eq!(moved, 2, "#6 and #7 share the spelling; #8 has another");
        assert_eq!(counterparty_of(&conn, 7).as_deref(), Some("Kaufland Sofia"));
        assert_eq!(counterparty_of(&conn, 8).as_deref(), Some("Kaufland"));
        let found = crate::counterparty::find(&conn, "KAUFLAND").unwrap();
        let sofia: i64 = conn.query_row("select id from counterparty where name = 'Kaufland Sofia'", [], |r| r.get(0)).unwrap();
        assert_eq!(found, Some(sofia), "a future import knows the spelling the new way");
        assert!(crate::counterparty::reassign(&mut conn, 6, "kaufland sofia").is_err(), "already with it");

        let respell = last(&conn).id;
        undo(&mut conn, respell).unwrap();
        assert_eq!(counterparty_of(&conn, 7).as_deref(), Some("Kaufland"));
        assert_eq!(crate::counterparty::find(&conn, "KAUFLAND").unwrap(), Some(2));
    }

    #[test]
    fn a_merge_is_logged_but_not_undoable() {
        let mut conn = ledger();
        crate::counterparty::merge(&mut conn, "Kaufland", "Northwind").unwrap();
        let entry = last(&conn);
        assert!(!entry.can_undo);
        assert!(undo(&mut conn, entry.id).is_err());
    }

    /// Giving a counterparty a category strips the matching own category off its
    /// operations, and undoing the edit gives it back without loss.
    #[test]
    fn undoing_counterparty_category_restores_cleared_operation_categories() {
        let mut conn = ledger();
        edit::set_own_category(&conn, 6, Some("Food")).unwrap();
        crate::counterparty::set_category(&conn, "Kaufland", "Food").unwrap();
        let own_during: Option<i64> = conn.query_row("select category_id from operation where id = 6", [], |r| r.get(0)).unwrap();
        assert_eq!(own_during, None, "the matching own category is stripped");

        let entry = last(&conn);
        undo(&mut conn, entry.id).unwrap();
        let own_after: Option<String> = conn
            .query_row(
                "select c.name from operation o left join category c on c.id = o.category_id where o.id = 6",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(own_after.as_deref(), Some("Food"), "undo gave the operation its own category back");
    }

    #[test]
    fn every_undo_survives_being_written_down() {
        let all = [
            Undo::Restore(1),
            Undo::Remove(2),
            Undo::Pair(3, 4),
            Undo::Unpair(5),
            Undo::OwnCategory(6, Some("Food and that is all".into())),
            Undo::OwnCategory(6, None),
            Undo::Replace { old: 7, new: 8 },
            Undo::Opening(9, -100),
            Undo::DeleteDebt(10),
            Undo::Unlink(11),
            Undo::DeleteMovement(12),
            Undo::Relink {
                debt: 1,
                operation: None,
                kind: "ISSUE".into(),
                amount: 5,
                on: "2026-10-18".into(),
                note: Some("x".into()),
                role: None,
            },
            Undo::Undismiss("1,2".into()),
            Undo::DeleteReconciliation(13),
            Undo::CounterpartyCategory("Kaufland".into(), Some("Food".into()), vec![1, 6]),
            Undo::CounterpartyCategory("Kaufland".into(), None, vec![]),
            Undo::Unimport(14),
            Undo::Role(15, "excluded".into()),
            Undo::Rename("A b".into(), "C".into()),
            Undo::DeleteAccount(4),
            Undo::RenameAccount(4, "Wise EUR".into()),
            Undo::Archive(4, true),
            Undo::EditDebt(1, "I_OWE".into(), Some("2026-12-31".into()), None),
            Undo::Split(6, vec![("Food = and ; more".into(), 300), ("Home".into(), 950)]),
            Undo::Split(6, vec![]),
            Undo::RenameCategory("Food".into(), "Groceries".into()),
            Undo::Budget("Food".into(), Some(20000)),
            Undo::Budget("Food".into(), None),
            Undo::MonthStart("2026-10".into(), Some("2026-10-29".into())),
            Undo::MonthStart("2026-10".into(), None),
            Undo::Respell { alias: "kaufland".into(), counterparty: Some(2), alias_was: None, operations: vec![1, 2] },
            Undo::Respell { alias: "x".into(), counterparty: None, alias_was: Some(3), operations: vec![] },
        ];
        for undo in all {
            assert_eq!(Undo::decode(&undo.encode()).unwrap(), undo);
        }
    }
}
