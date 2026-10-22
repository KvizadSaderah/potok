//! Edits by hand: remove a duplicate, bring it back, untie from a debt, a transfer or not, an own category.
//!
//! Came about when the real ledger turned up things that had to be fixed and
//! nothing to fix them with: the 720 refund on the Northwind debt, written in
//! saldo twice out of two competing imports, and a utilities payment that
//! restoring the history in saldo tied to the same debt as a «refund». Neither
//! the amount nor the date nor the account of a posted operation changes here
//! (invariant 8): a mistaken operation is removed by a mark and brought back.
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    /// One, or two if it is a transfer's leg: legs are removed together only.
    pub operations: usize,
    /// How many debt movements stopped counting along with it.
    pub debt_movements: usize,
}

/// Removes an operation as mistaken — a duplicate. A mark, not a deletion: it
/// stays visible, stops counting in the balance, in the flows and in the debt,
/// `restore_operation`.
pub fn remove_operation(conn: &mut Connection, id: i64) -> Result<Removed> {
    let transaction = conn.transaction()?;
    let (state, pair): (String, Option<i64>) = transaction
        .query_row("select state, pair_id from operation where id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{id}", "нет операции #{id}")))?;
    if state != "POSTED" {
        bail!(crate::tr!("the operation is already removed", "операция уже убрана"));
    }
    let ids: Vec<i64> = std::iter::once(id).chain(pair).collect();
    let mut debt_movements = 0usize;
    for op in &ids {
        transaction.execute("update operation set state = 'REVERSED' where id = ?1", [op])?;
        let negative_debt: bool = transaction.query_row(
            "select exists (
                select 1 from debt_movement m
                  join debt_remaining r on r.debt_id = m.debt_id
                 where m.operation_id = ?1 and r.remaining_minor < 0
            )",
            [op],
            |r| r.get(0),
        )?;
        if negative_debt {
            bail!(crate::tr!(
                "removing this loan issue would leave the debt below zero — unlink its repayments first",
                "если убрать эту выдачу, остаток долга уйдёт ниже нуля — сначала отвязать его возвраты"
            ));
        }
        debt_movements += transaction.query_row(
            "select count(*) from debt_movement where operation_id = ?1",
            [op],
            |r| r.get::<_, i64>(0),
        )? as usize;
    }
    let what = crate::journal::describe(&transaction, id);
    crate::journal::log(
        &transaction,
        &if ids.len() > 1 {
            crate::tr!("removed {what} with its other leg", "убрана {what} вместе со второй ногой")
        } else {
            crate::tr!("removed {what}", "убрана {what}")
        },
        Some(crate::journal::Undo::Restore(id)),
    )?;
    transaction.commit()?;
    Ok(Removed { operations: ids.len(), debt_movements })
}

/// Brings a removed one back. The debt movements that referred to it count
/// again on their own — a debt's remainder is a computation, not a number.
pub fn restore_operation(conn: &mut Connection, id: i64) -> Result<usize> {
    let transaction = conn.transaction()?;
    let (state, pair): (String, Option<i64>) = transaction
        .query_row("select state, pair_id from operation where id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{id}", "нет операции #{id}")))?;
    if state != "REVERSED" {
        bail!(crate::tr!("the operation is not removed — nothing to restore", "операция не убрана — возвращать нечего"));
    }
    let ids: Vec<i64> = std::iter::once(id).chain(pair).collect();
    for op in &ids {
        transaction.execute("update operation set state = 'POSTED' where id = ?1", [op])?;
    }
    let what = crate::journal::describe(&transaction, id);
    crate::journal::log(&transaction, &crate::tr!("restored {what}", "возвращена {what}"), Some(crate::journal::Undo::Remove(id)))?;
    transaction.commit()?;
    Ok(ids.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unlinked {
    /// The operation was a «debt movement» and became an ordinary spend or
    /// income — otherwise it would not appear in the flows, where it belongs.
    pub role_now: Option<String>,
}

/// «This is not a debt payment»: takes a movement off a debt. The debt's
/// remainder recomputes itself. The operation stays — the money did leave the
/// account — and if it counted as a debt movement, becomes a spend or income.
pub fn unlink_movement(conn: &mut Connection, movement_id: i64) -> Result<Unlinked> {
    let transaction = conn.transaction()?;
    #[allow(clippy::type_complexity)]
    let (debt, operation, kind, amount, on, note): (i64, Option<i64>, String, i64, String, Option<String>) = transaction
        .query_row(
            "select debt_id, operation_id, kind, amount_minor, occurred_on, note from debt_movement where id = ?1",
            [movement_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no debt movement #{movement_id}", "нет движения #{movement_id}")))?;
    let role_before: Option<String> = match operation {
        Some(op) => transaction.query_row("select role from operation where id = ?1", [op], |r| r.get(0)).optional()?,
        None => None,
    };
    transaction.execute("delete from debt_movement where id = ?1", [movement_id])?;
    let mut role_now = None;
    if let Some(op) = operation {
        let still_linked: i64 = transaction.query_row(
            "select count(*) from debt_movement where operation_id = ?1",
            [op],
            |r| r.get(0),
        )?;
        let (role, effect): (String, i64) = transaction.query_row(
            "select role, effect_minor from operation where id = ?1",
            [op],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if role == "debt_movement" && still_linked == 0 {
            let new_role = if effect < 0 { "expense" } else { "income" };
            transaction.execute("update operation set role = ?1 where id = ?2", params![new_role, op])?;
            role_now = Some(new_role.to_string());
        }
    }
    let who: String = transaction.query_row(
        "select p.name from debt d join counterparty p on p.id = d.counterparty_id where d.id = ?1",
        [debt],
        |r| r.get(0),
    )?;
    let what = operation.map(|op| crate::journal::describe(&transaction, op)).unwrap_or_else(|| crate::tr!("a movement with no operation", "движение без операции"));
    crate::journal::log(
        &transaction,
        &crate::tr!("unlinked from the debt «{who}»: {what}", "отвязано от долга «{who}»: {what}"),
        Some(crate::journal::Undo::Relink { debt, operation, kind, amount, on, note, role: role_before }),
    )?;
    transaction.commit()?;
    Ok(Unlinked { role_now })
}

/// The role of an operation that stopped being a transfer's leg: by its sign.
fn plain_role(effect: i64) -> &'static str {
    if effect < 0 { "expense" } else { "income" }
}

/// «This is not a transfer»: breaks the pair. Both legs become an ordinary
/// spend and income — each by its own sign — and appear in the flows.
pub fn unpair_transfer(conn: &mut Connection, id: i64) -> Result<()> {
    let transaction = conn.transaction()?;
    // A transit has a pair too — a transfer told «this passed through me».
    // It has to be broken the same way: otherwise a pair that cannot be broken
    // stays in the ledger for good (and one such arrived from saldo).
    let pair: Option<i64> = transaction
        .query_row(
            "select pair_id from operation where id = ?1 and role in ('transfer', 'passthrough')",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| {
            anyhow::anyhow!(crate::tr!("operation #{id} is not a transfer", "операция #{id} не перевод"))
        })?;
    for op in std::iter::once(id).chain(pair) {
        let effect: i64 = transaction.query_row("select effect_minor from operation where id = ?1", [op], |r| r.get(0))?;
        transaction.execute(
            "update operation set role = ?1, pair_id = null where id = ?2",
            params![plain_role(effect), op],
        )?;
    }
    if let Some(other) = pair {
        let what = crate::journal::describe(&transaction, id);
        crate::journal::log(&transaction, &crate::tr!("transfer split {what}", "разорван перевод {what}"), Some(crate::journal::Undo::Pair(id, other)))?;
    }
    transaction.commit()?;
    Ok(())
}

/// What the second leg of a transfer may be: on another account in the same
/// currency, the same amount with the opposite sign, posted, paired with
/// nobody yet, no further than three days off. Nearest by date first.
// ponytail: transfers across currencies (EUR → RUB) do not land here — the
// legs differ in amount, and guessing the rate was not taken on. If needed,
// search by date and account, and show both amounts.
pub fn transfer_candidates(conn: &Connection, id: i64) -> Result<Vec<crate::query::OperationLine>> {
    let (account, currency, effect, on): (i64, String, i64, String) = conn.query_row(
        "select account_id, currency, effect_minor, date(occurred_at) from operation where id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let mut ids = partners(conn, account, &currency, effect, &on)?;
    // After one's own currency — the others: a EUR → RUB transfer arrives as a
    // different amount, and only the date and direction tell it. A person picks.
    let mut stmt = conn.prepare(
        "select id from operation
          where account_id <> ?1 and currency <> ?2 and effect_minor * ?3 < 0
            and state = 'POSTED' and pair_id is null and role in ('income','expense')
            and abs(julianday(date(occurred_at)) - julianday(date(?4))) <= ?5
          order by abs(julianday(date(occurred_at)) - julianday(date(?4))), id
          limit 10",
    )?;
    let days = crate::settings::transfer_days(conn)?;
    ids.extend(
        stmt.query_map(params![account, currency, effect, on, days], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    );
    let mut lines = Vec::new();
    for other in ids {
        if let Some(line) = crate::query::operation_by_id(conn, other)? {
            lines.push(line);
        }
    }
    Ok(lines)
}

/// The numbers of the operations that may be the second leg of a transfer for
/// the movement `effect` on account `account` on day `on`. Shared by the edit
/// from the menu and by parsing a statement before writing, where there is none.
pub(crate) fn partners(conn: &Connection, account: i64, currency: &str, effect: i64, on: &str) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "select id from operation
          where account_id <> ?1 and currency = ?2 and effect_minor = -?3
            and state = 'POSTED' and pair_id is null and role in ('income','expense')
            and abs(julianday(date(occurred_at)) - julianday(date(?4))) <= ?5
          order by abs(julianday(date(occurred_at)) - julianday(date(?4))), id
          limit 10",
    )?;
    let days = crate::settings::transfer_days(conn)?;
    let ids = stmt
        .query_map(params![account, currency, effect, on, days], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// «This is a transfer between my own»: pairs two operations. The same
/// conditions as in `transfer_candidates` are checked here once more — they
/// hold invariant 3, and a button must not be the only guard.
/// An own note on an operation: what the bank did not write. Empty clears it.
/// Editable always: a note moves no money, and invariant 8 does not touch it.
pub fn set_note(conn: &Connection, id: i64, note: &str) -> Result<()> {
    crate::db::atomic(conn, || {
        let was: Option<String> =
            conn.query_row("select note from operation where id = ?1", [id], |r| r.get(0)).optional()?.flatten();
        let note = note.trim();
        let value = (!note.is_empty()).then(|| note.to_string());
        let changed = conn.execute("update operation set note = ?1 where id = ?2", params![value, id])?;
        if changed == 0 {
            bail!(crate::tr!("no operation #{id}", "нет операции #{id}"));
        }
        let what = crate::journal::describe(conn, id);
        crate::journal::log(
            conn,
            &match &value {
                Some(note) => crate::tr!("note on {what}: {note}", "заметка на {what}: {note}"),
                None => crate::tr!("note on {what} removed", "заметка с {what} снята"),
            },
            Some(crate::journal::Undo::OperationNote(id, was)),
        )?;
        Ok(())
    })
}

pub fn pair_transfer(conn: &mut Connection, a: i64, b: i64) -> Result<()> {
    let transaction = conn.transaction()?;
    pair_in(&transaction, a, b)?;
    let (x, y) = (crate::journal::describe(&transaction, a), crate::journal::describe(&transaction, b));
    crate::journal::log(&transaction, &crate::tr!("transfer paired {x} ↔ {y}", "связан перевод {x} ↔ {y}"), Some(crate::journal::Undo::Unpair(a)))?;
    transaction.commit()?;
    Ok(())
}

/// The same inside somebody else's transaction — an import that writes a leg
/// and ties it at once.
pub(crate) fn pair_in(transaction: &Connection, a: i64, b: i64) -> Result<()> {
    let has_split: bool = transaction.query_row(
        "select exists (select 1 from operation_split where operation_id in (?1, ?2))",
        [a, b],
        |r| r.get(0),
    )?;
    if has_split {
        bail!(crate::tr!(
            "a split operation cannot be a transfer — clear its split first",
            "разбитая операция не может быть переводом — сначала снять разбивку"
        ));
    }
    let ok: bool = transaction.query_row(
        "select count(*) = 1 from operation x join operation y on y.id = ?2
          where x.id = ?1 and x.id <> y.id
            and x.account_id <> y.account_id
            -- In one currency the legs are equal and opposite. Across currencies
            -- (EUR → RUB) the amounts differ — the bank's rate — and only the
            -- opposite sign is required.
            and ((x.currency = y.currency and x.effect_minor = -y.effect_minor)
                 or (x.currency <> y.currency and x.effect_minor * y.effect_minor < 0))
            and x.state = 'POSTED' and y.state = 'POSTED'
            and x.pair_id is null and y.pair_id is null
            and x.role in ('income','expense') and y.role in ('income','expense')",
        [a, b],
        |r| r.get(0),
    )?;
    if !ok {
        bail!(crate::tr!("these two operations cannot be one transfer", "эти две операции не могут быть одним переводом"));
    }
    transaction.execute("update operation set role = 'transfer', pair_id = ?2 where id = ?1", [a, b])?;
    transaction.execute("update operation set role = 'transfer', pair_id = ?2 where id = ?1", [b, a])?;
    Ok(())
}

/// A transfer between one's own accounts entered by hand: both legs at once.
/// In one currency `received` equals `sent`; across currencies — what arrived.
pub fn add_transfer(
    conn: &mut Connection,
    from: i64,
    to: i64,
    sent_minor: i64,
    received_minor: i64,
    on: chrono::NaiveDate,
    note: &str,
) -> Result<(i64, i64)> {
    if from == to {
        bail!(crate::tr!("a transfer goes between two different accounts", "перевод — между двумя разными счетами"));
    }
    if sent_minor <= 0 || received_minor <= 0 {
        bail!(crate::tr!("transfer amounts are positive", "суммы перевода — положительные"));
    }
    let currency = |id: i64| -> Result<String> {
        conn.query_row("select currency from account where id = ?1", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!(crate::tr!("no account #{id}", "нет счёта #{id}")))
    };
    if currency(from)? == currency(to)? && sent_minor != received_minor {
        bail!(crate::tr!(
            "in one currency what is sent is what arrives",
            "в одной валюте ушло столько же, сколько пришло"
        ));
    }
    let who = if note.trim().is_empty() { crate::tr!("transfer", "перевод") } else { note.trim().to_string() };
    let transaction = conn.transaction()?;
    let new = |account: i64, effect: i64| NewOperation { account_id: account, occurred_on: on, effect_minor: effect, who: who.clone(), category: None };
    let a = insert(&transaction, &new(from, -sent_minor))?;
    let b = insert(&transaction, &new(to, received_minor))?;
    pair_in(&transaction, a, b)?;
    let (x, y) = (crate::journal::describe(&transaction, a), crate::journal::describe(&transaction, b));
    crate::journal::log(
        &transaction,
        &crate::tr!("transfer entered by hand {x} → {y}", "перевод введён руками {x} → {y}"),
        Some(crate::journal::Undo::Remove(a)),
    )?;
    transaction.commit()?;
    Ok((a, b))
}

/// Parts of an operation, as the person types them: `Food 30; Household 12.50`
/// — and at most one part with no amount, which takes what is left.
pub fn parse_split(text: &str, total_minor: i64) -> Result<Vec<(String, i64)>> {
    let mut parts: Vec<(String, Option<i64>)> = Vec::new();
    for piece in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, amount) = match piece.rsplit_once(' ') {
            Some((name, last)) => match crate::reconcile::parse_input(last) {
                Ok(minor) => (name.trim().to_string(), Some(minor.abs())),
                Err(_) => (piece.to_string(), None),
            },
            None => (piece.to_string(), None),
        };
        parts.push((name, amount));
    }
    let given: i64 = parts.iter().filter_map(|(_, a)| *a).sum();
    let open = parts.iter().filter(|(_, a)| a.is_none()).count();
    if open > 1 {
        bail!(crate::tr!("only one part may be without an amount", "без суммы может быть только одна часть"));
    }
    let left = total_minor - given;
    let out: Vec<(String, i64)> = parts.into_iter().map(|(name, a)| (name, a.unwrap_or(left))).collect();
    let sum: i64 = out.iter().map(|(_, a)| *a).sum();
    if sum != total_minor || out.iter().any(|(_, a)| *a <= 0) {
        bail!(crate::tr!(
            "the parts add up to {}, the operation is {}",
            "части дают {}, а операция — {}",
            crate::money::format_minor(sum),
            crate::money::format_minor(total_minor)
        ));
    }
    Ok(out)
}

/// Split an operation by category; no parts — undo the split. The parts must
/// add up to the operation exactly.
pub fn split(conn: &mut Connection, id: i64, parts: &[(String, i64)]) -> Result<()> {
    let transaction = conn.transaction()?;
    let (state, role, amount): (String, String, i64) = transaction
        .query_row("select state, role, amount_minor from operation where id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{id}", "нет операции #{id}")))?;
    if state != "POSTED" || !matches!(role.as_str(), "income" | "expense") {
        bail!(crate::tr!(
            "only a posted expense or income is split",
            "разбить можно только проведённый расход или доход"
        ));
    }
    if !parts.is_empty() && parts.iter().map(|(_, a)| *a).sum::<i64>() != amount {
        bail!(crate::tr!("the parts do not add up to the operation", "части не дают сумму операции"));
    }
    let before = split_of(&transaction, id)?;
    transaction.execute("delete from operation_split where operation_id = ?1", [id])?;
    for (category, minor) in parts {
        let category = category.trim();
        if category.is_empty() {
            bail!(crate::tr!("a part has no category", "у части нет категории"));
        }
        if *minor <= 0 {
            bail!(crate::tr!("each split part must be positive", "каждая часть разбивки должна быть больше нуля"));
        }
        transaction.execute("insert or ignore into category (name) values (?1)", [category])?;
        let category_id: i64 = transaction.query_row("select id from category where name = ?1", [category], |r| r.get(0))?;
        let merged = transaction.execute(
            "update operation_split set amount_minor = amount_minor + ?3
              where operation_id = ?1 and category_id = ?2",
            params![id, category_id, minor],
        )?;
        if merged == 0 {
            transaction.execute(
                "insert into operation_split (operation_id, category_id, amount_minor)
                 values (?1, ?2, ?3)",
                params![id, category_id, minor],
            )?;
        }
    }
    let what = crate::journal::describe(&transaction, id);
    let said = if parts.is_empty() {
        crate::tr!("split undone {what}", "разбивка снята {what}")
    } else {
        let list: Vec<String> = parts.iter().map(|(c, a)| format!("{c} {}", crate::money::format_minor(*a))).collect();
        crate::tr!("split {what}: {}", "разбита {what}: {}", list.join("; "))
    };
    crate::journal::log(&transaction, &said, Some(crate::journal::Undo::Split(id, before)))?;
    transaction.commit()?;
    Ok(())
}

/// The parts of a split operation, in the order written.
pub fn split_of(conn: &Connection, id: i64) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "select c.name, s.amount_minor from operation_split s join category c on c.id = s.category_id
          where s.operation_id = ?1 order by s.id",
    )?;
    let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// How an operation counts in the flows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counting {
    /// A spend or income — by the sign. Goes into the flows.
    Counted,
    /// Not counted: the money moved, but it is neither a spend nor earnings —
    /// one's own cash, a bank's mistake, a deposit returned. In the balance,
    /// not in the flows.
    Excluded,
    /// Transit: it passed through me — paid for a friend, and he paid back. Not
    /// in the flows either; a separate word, so that later the why is visible.
    Passthrough,
    /// Refund: money that came back from a purchase. It takes the spending
    /// off its category instead of counting as income — returned shoes cost
    /// nothing, they did not cost 100 and earn 100. Stored as an expense with
    /// a plus sign; the schema needs no new role for it.
    Refund,
}

/// How an operation counts, in one word: its role, or «refund» for an
/// expense with a plus sign.
pub fn kind(role: &str, effect_minor: i64) -> &str {
    if role == "expense" && effect_minor > 0 {
        "refund"
    } else {
        role
    }
}

/// Changes how an operation counts. The balance is untouched: the money on the
/// account is the same, only whether it enters the flows changes.
///
/// A transfer and a debt movement are not changed this way — they have their
/// own edit (break the pair, untie from the debt) and a second side to change.
pub fn set_counting(conn: &mut Connection, id: i64, counting: Counting) -> Result<()> {
    let transaction = conn.transaction()?;
    let (state, role, effect): (String, String, i64) = transaction
        .query_row("select state, role, effect_minor from operation where id = ?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{id}", "нет операции #{id}")))?;
    if state != "POSTED" {
        bail!(crate::tr!("the operation is removed — restore it first", "операция убрана — сначала вернуть"));
    }
    match role.as_str() {
        "income" | "expense" | "excluded" | "passthrough" => {}
        "transfer" => bail!(crate::tr!("this is a transfer — split the pair first", "это перевод — сначала разорвать пару")),
        "debt_movement" => bail!(crate::tr!("this is a debt payment — unlink it from the debt first", "это платёж по долгу — сначала отвязать от долга")),
        other => bail!(crate::tr!("an operation with role {other} cannot be changed this way", "операцию с ролью {other} так не поменять")),
    }
    let new_role = match counting {
        // A row from saldo, «excluded» with a zero effect, remembers no sign:
        // went out or came in is unknown, and guessing here is not allowed.
        // The schema permits a zero effect for «not counted» only (§4).
        Counting::Counted | Counting::Passthrough if effect == 0 => {
            bail!(crate::tr!(
                "the row has no sign — unknown whether money left or arrived; it can only be «not counted»",
                "у строки нет знака — непонятно, ушло или пришло; она может быть только «не считается»"
            ))
        }
        Counting::Counted => plain_role(effect),
        Counting::Excluded => "excluded",
        Counting::Passthrough => "passthrough",
        Counting::Refund if effect <= 0 => bail!(crate::tr!(
            "a refund is money that came back — this one went out",
            "возврат — это деньги, которые пришли обратно, а эти ушли"
        )),
        Counting::Refund => "expense",
    };
    let was = kind(&role, effect).to_string();
    if kind(new_role, effect) == was {
        bail!(crate::tr!("the operation already counts this way", "операция уже так и считается"));
    }
    if !matches!(new_role, "income" | "expense") {
        let has_split: bool = transaction.query_row(
            "select exists (select 1 from operation_split where operation_id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if has_split {
            bail!(crate::tr!(
                "a split operation cannot be «{}» — clear its split first",
                "разбитая операция не может быть «{}» — сначала снять разбивку",
                if new_role == "excluded" {
                    crate::trs!("not counted", "не считается")
                } else {
                    crate::trs!("transit", "транзит")
                }
            ));
        }
    }
    transaction.execute("update operation set role = ?1 where id = ?2", params![new_role, id])?;
    let what = crate::journal::describe(&transaction, id);
    let said = |r: &str| match r {
        "excluded" => crate::trs!("not counted", "не считается"),
        "passthrough" => crate::trs!("transit", "транзит"),
        "income" => crate::trs!("income", "доход"),
        "refund" => crate::trs!("refund", "возврат"),
        _ => crate::trs!("expense", "расход"),
    };
    crate::journal::log(
        &transaction,
        &format!("{what}: {} → {}", said(&was), said(kind(new_role, effect))),
        Some(crate::journal::Undo::Role(id, was)),
    )?;
    transaction.commit()?;
    Ok(())
}

/// An operation entered by hand: cash, or the right amount instead of a wrong one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOperation {
    pub account_id: i64,
    pub occurred_on: chrono::NaiveDate,
    /// Signed: minus went out, plus came in.
    pub effect_minor: i64,
    /// Who or what for — becomes the description and the counterparty.
    pub who: String,
    /// The operation's own category. Without it — the counterparty's.
    pub category: Option<String>,
}

/// Enters an operation by hand. A spend or income — by the sign; it is made a
/// transfer or a debt afterwards by the same menu as an imported one.
pub fn add_operation(conn: &mut Connection, new: &NewOperation) -> Result<i64> {
    let transaction = conn.transaction()?;
    let id = insert(&transaction, new)?;
    let what = crate::journal::describe(&transaction, id);
    crate::journal::log(&transaction, &crate::tr!("entered by hand {what}", "введена руками {what}"), Some(crate::journal::Undo::Remove(id)))?;
    transaction.commit()?;
    Ok(id)
}

/// «Correct it»: remove the wrong operation (by a mark, invariant 8) and enter
/// the right one — in one action, so as to end with neither two nor none.
///
/// A transfer's leg and a debt movement cannot be corrected this way: they
/// have a second side that must change too. Break or untie it first.
pub fn replace_operation(conn: &mut Connection, old: i64, new: &NewOperation) -> Result<i64> {
    let transaction = conn.transaction()?;
    let (state, role): (String, String) = transaction
        .query_row("select state, role from operation where id = ?1", [old], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{old}", "нет операции #{old}")))?;
    if state != "POSTED" {
        bail!(crate::tr!("the operation is already removed", "операция уже убрана"));
    }
    match role.as_str() {
        "income" | "expense" => {}
        "transfer" => bail!(crate::tr!("this is a transfer — split the pair before fixing it", "это перевод — сначала разорвать пару, потом исправлять")),
        "debt_movement" => bail!(crate::tr!("this is a debt payment — unlink it before fixing it", "это платёж по долгу — сначала отвязать от долга, потом исправлять")),
        other => bail!(crate::tr!("an operation with role {other} cannot be fixed this way", "операцию с ролью {other} так не исправить")),
    }
    transaction.execute("update operation set state = 'REVERSED' where id = ?1", [old])?;
    let id = insert(&transaction, new)?;
    let (was, now) = (crate::journal::describe(&transaction, old), crate::journal::describe(&transaction, id));
    crate::journal::log(
        &transaction,
        &crate::tr!("fixed {was} → {now}", "исправлена {was} → {now}"),
        Some(crate::journal::Undo::Replace { old, new: id }),
    )?;
    transaction.commit()?;
    Ok(id)
}

fn insert(transaction: &Connection, new: &NewOperation) -> Result<i64> {
    if new.effect_minor == 0 {
        bail!(crate::tr!("the amount is zero — there is no such operation", "сумма нулевая — такой операции не бывает"));
    }
    let who = new.who.trim();
    if who.is_empty() {
        bail!(crate::tr!("who or what for is empty — without it the operation cannot be found later", "кто или за что — пусто; без этого операцию потом не найти"));
    }
    let currency: String = transaction
        .query_row("select currency from account where id = ?1", [new.account_id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no account #{}", "нет счёта #{}", new.account_id)))?;
    let counterparty = crate::counterparty::resolve_or_create(transaction, who)?;
    let role = if new.effect_minor < 0 { "expense" } else { "income" };
    transaction.execute(
        "insert into operation (account_id, role, state, occurred_at, amount_minor, currency,
                                effect_minor, description, counterparty_id)
         values (?1, ?2, 'POSTED', ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            new.account_id,
            role,
            format!("{}T00:00:00Z", new.occurred_on),
            new.effect_minor.abs(),
            currency,
            new.effect_minor,
            who,
            counterparty,
        ],
    )?;
    let id = transaction.last_insert_rowid();
    if let Some(category) = new.category.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        own_category_in(transaction, id, Some(category))?;
    }
    Ok(id)
}

/// An account's opening balance — what was there before the ledger's first
/// operation. Returns the previous one.
pub fn set_opening(conn: &Connection, account_id: i64, minor: i64) -> Result<i64> {
    crate::db::atomic(conn, || {
        let before: i64 = conn
            .query_row("select opening_minor from account where id = ?1", [account_id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!(crate::tr!("no account #{account_id}", "нет счёта #{account_id}")))?;
        conn.execute("update account set opening_minor = ?1 where id = ?2", params![minor, account_id])?;
        let name: String = conn.query_row("select name from account where id = ?1", [account_id], |r| r.get(0))?;
        crate::journal::log(
            conn,
            &crate::tr!(
                "opening balance {name}: {} → {}",
                "начальный остаток {name}: {} → {}",
                crate::money::format_minor(before),
                crate::money::format_minor(minor)
            ),
            Some(crate::journal::Undo::Opening(account_id, before)),
        )?;
        Ok(before)
    })
}

/// A date as a person types it: `2026-10-18`, `18.10.2026`, `18.10` (this
/// year), `today`, `yesterday`.
pub fn parse_date(text: &str, today: chrono::NaiveDate) -> Result<chrono::NaiveDate> {
    use chrono::{Datelike, NaiveDate};
    let text = text.trim();
    match text {
        "" | "today" | "сегодня" => return Ok(today),
        "yesterday" | "вчера" => return Ok(today - chrono::Duration::days(1)),
        _ => {}
    }
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(day);
    }
    if let Ok(day) = NaiveDate::parse_from_str(text, "%d.%m.%Y") {
        return Ok(day);
    }
    if let Ok(day) = NaiveDate::parse_from_str(&format!("{text}.{}", today.year()), "%d.%m.%Y") {
        return Ok(day);
    }
    bail!(crate::tr!(
        "not a date: «{text}» — use 2026-10-18, 18.10.2026 or 18.10",
        "не похоже на дату: «{text}» — нужно 2026-10-18, 18.10.2026 или 18.10"
    ))
}

/// An own category on one operation — an exception to the counterparty's rule.
/// `None` clears it, and the operation takes the counterparty's again.
pub fn set_own_category(conn: &Connection, operation_id: i64, category: Option<&str>) -> Result<()> {
    crate::db::atomic(conn, || {
        let before = own_category_in(conn, operation_id, category)?;
        let what = crate::journal::describe(conn, operation_id);
        crate::journal::log(
            conn,
            &crate::tr!(
                "own category {what}: {} → {}",
                "своя категория {what}: {} → {}",
                before.as_deref().unwrap_or("—"),
                category.unwrap_or("—")
            ),
            Some(crate::journal::Undo::OwnCategory(operation_id, before)),
        )?;
        Ok(())
    })
}

/// Sets or clears an own category without writing to the journal: for edits
/// that write a line of their own. Returns the previous one.
fn own_category_in(conn: &Connection, operation_id: i64, category: Option<&str>) -> Result<Option<String>> {
    let before: Option<String> = conn
        .query_row(
            "select c.name from operation o left join category c on c.id = o.category_id where o.id = ?1",
            [operation_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let category_id = match category {
        None => None,
        Some(name) => Some(
            match conn
                .query_row("select id from category where name = ?1", [name], |r| r.get::<_, i64>(0))
                .optional()?
            {
                Some(id) => id,
                None => {
                    conn.execute("insert into category (name) values (?1)", [name])?;
                    conn.last_insert_rowid()
                }
            },
        ),
    };
    let changed = conn.execute(
        "update operation set category_id = ?1 where id = ?2",
        params![category_id, operation_id],
    )?;
    if changed == 0 {
        bail!(crate::tr!("no operation #{operation_id}", "нет операции #{operation_id}"));
    }
    Ok(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, query};

    /// The real case in miniature: a debt of 2200, the 720 refund written
    /// twice, and a utilities payment of 23.93 tied to the debt.
    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',100000), ('Revolut','EUR','BANK',0);
             insert into counterparty (name) values ('Northwind');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'debt_movement','2026-09-03T00:00:00Z',72000,'EUR',-72000,'NORTHWIND return'),
               (2,1,'debt_movement','2026-09-03T00:00:00Z',72000,'EUR',-72000,'NORTHWIND return'),
               (3,2,'debt_movement','2026-06-16T00:00:00Z',2393,'EUR',-2393,'Epay To: Epay Utilities'),
               (4,1,'transfer','2026-09-04T00:00:00Z',5000,'EUR',-5000,'to revolut'),
               (5,2,'transfer','2026-09-04T00:00:00Z',5000,'EUR',5000,'from icard');
             update operation set pair_id = 5 where id = 4;
             update operation set pair_id = 4 where id = 5;
             insert into debt (counterparty_id,direction,currency,opened_on) values (1,'I_OWE','EUR','2026-01-30');
             insert into debt_movement (id,debt_id,operation_id,kind,amount_minor,occurred_on) values
               (1,1,null,'ISSUE',220000,'2026-01-30'),
               (2,1,1,'REPAYMENT',72000,'2026-09-03'),
               (3,1,2,'REPAYMENT',72000,'2026-09-03'),
               (4,1,3,'REPAYMENT',2393,'2026-06-16');",
        )
        .unwrap();
        conn
    }

    fn role(conn: &Connection, id: i64) -> (String, Option<i64>) {
        conn.query_row("select role, pair_id from operation where id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
    }

    /// Break and pair again: the roles and pairs come back, the check is quiet.
    /// A pair told «transit» has to be breakable too: one such arrived from
    /// saldo and kept `verify` red.
    #[test]
    fn a_transit_pair_can_be_split_too() {
        let mut conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'iCard','EUR','CARD'), (2,'R','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,pair_id) values
               (1,1,'passthrough','2026-07-13T00:00:00Z',172,'EUR',-172,'one',2),
               (2,2,'passthrough','2026-07-15T00:00:00Z',172,'EUR', 172,'two',1);",
        )
        .unwrap();
        unpair_transfer(&mut conn, 1).unwrap();
        let role = |conn: &Connection, id: i64| -> (String, Option<i64>) {
            conn.query_row("select role, pair_id from operation where id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
        };
        assert_eq!(role(&conn, 1), ("expense".into(), None));
        assert_eq!(role(&conn, 2), ("income".into(), None));
        assert!(crate::verify::run(&conn).unwrap().ok(), "after the break the invariants hold");
    }

    #[test]
    fn a_transfer_is_unpaired_and_paired_back() {
        let mut conn = ledger();
        unpair_transfer(&mut conn, 4).unwrap();
        assert_eq!(role(&conn, 4), ("expense".into(), None));
        assert_eq!(role(&conn, 5), ("income".into(), None));

        let found: Vec<i64> = transfer_candidates(&conn, 4).unwrap().iter().map(|l| l.id).collect();
        assert_eq!(found, vec![5]);
        pair_transfer(&mut conn, 4, 5).unwrap();
        assert_eq!(role(&conn, 4), ("transfer".into(), Some(5)));
        assert_eq!(role(&conn, 5), ("transfer".into(), Some(4)));
        assert!(crate::verify::run(&conn).unwrap().ok());
    }

    /// The wrong amount, the same account, with itself, already paired — refused.
    #[test]
    fn what_cannot_be_a_transfer_is_refused() {
        let mut conn = ledger();
        assert!(pair_transfer(&mut conn, 1, 3).is_err(), "the amounts differ");
        assert!(pair_transfer(&mut conn, 1, 2).is_err(), "one account");
        assert!(pair_transfer(&mut conn, 4, 4).is_err(), "with itself");
        assert!(pair_transfer(&mut conn, 4, 5).is_err(), "already paired");
        assert!(unpair_transfer(&mut conn, 1).is_err(), "not a transfer");
    }

    fn cash(conn: &Connection) -> i64 {
        conn.execute("insert into account (name,currency,kind) values ('Cash','EUR','CASH')", []).unwrap();
        conn.last_insert_rowid()
    }

    fn day(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// Cash by hand: the balance moved, the counterparty and own category are there.
    #[test]
    fn a_cash_purchase_is_entered_by_hand() {
        let mut conn = ledger();
        let account = cash(&conn);
        let id = add_operation(
            &mut conn,
            &NewOperation {
                account_id: account,
                occurred_on: day("2026-10-18"),
                effect_minor: -1250,
                who: "A bakery".into(),
                category: Some("Food".into()),
            },
        )
        .unwrap();
        assert_eq!(balance(&conn, "Cash"), -1250);
        let line = query::operation_by_id(&conn, id).unwrap().unwrap();
        assert_eq!((line.role.as_str(), line.counterparty.as_deref(), line.category.as_deref()),
                   ("expense", Some("A bakery"), Some("Food")));
        assert!(crate::verify::run(&conn).unwrap().ok());
    }

    /// Correct the amount: the old removed, the new entered, the balance the new one's.
    #[test]
    fn a_wrong_amount_is_replaced_in_one_step() {
        let mut conn = ledger();
        let account = cash(&conn);
        let new = |minor| NewOperation {
            account_id: account,
            occurred_on: day("2026-10-18"),
            effect_minor: minor,
            who: "A bakery".into(),
            category: None,
        };
        let wrong = add_operation(&mut conn, &new(-12500)).unwrap();
        let right = replace_operation(&mut conn, wrong, &new(-1250)).unwrap();
        assert_ne!(wrong, right);
        assert_eq!(balance(&conn, "Cash"), -1250);
        assert!(query::operation_by_id(&conn, wrong).unwrap().unwrap().removed);
        assert!(replace_operation(&mut conn, wrong, &new(-1)).is_err(), "a removed one cannot be corrected twice");
    }

    #[test]
    fn transfers_debts_and_nonsense_are_refused() {
        let mut conn = ledger();
        let account = cash(&conn);
        let new = |minor, who: &str| NewOperation {
            account_id: account,
            occurred_on: day("2026-10-18"),
            effect_minor: minor,
            who: who.into(),
            category: None,
        };
        assert!(replace_operation(&mut conn, 4, &new(-100, "x")).is_err(), "a transfer leg");
        assert!(replace_operation(&mut conn, 1, &new(-100, "x")).is_err(), "a debt payment");
        assert!(add_operation(&mut conn, &new(0, "x")).is_err());
        assert!(add_operation(&mut conn, &new(-100, "  ")).is_err());
        assert!(add_operation(&mut conn, &NewOperation { account_id: 99, ..new(-100, "x") }).is_err());
    }

    #[test]
    fn the_opening_balance_moves_the_balance() {
        let conn = ledger();
        let before = set_opening(&conn, 1, 50000).unwrap();
        assert_eq!(before, 100000);
        assert_eq!(balance(&conn, "iCard"), 50000 - 72000 - 72000 - 5000);
    }

    #[test]
    fn dates_read_the_way_people_type_them() {
        let today = day("2026-10-18");
        assert_eq!(parse_date("", today).unwrap(), today);
        assert_eq!(parse_date("yesterday", today).unwrap(), day("2026-10-17"));
        assert_eq!(parse_date("2026-09-03", today).unwrap(), day("2026-09-03"));
        assert_eq!(parse_date("03.09.2026", today).unwrap(), day("2026-09-03"));
        assert_eq!(parse_date("3.9", today).unwrap(), day("2026-09-03"));
        assert!(parse_date("tomorrow", today).is_err());
        assert!(parse_date("31.02", today).is_err());
    }

    /// «Do not count» takes it out of the flows and leaves the balance alone;
    /// «count it again» brings it back as a spend by its sign.
    #[test]
    fn counting_moves_an_operation_out_of_flows_and_not_out_of_the_balance() {
        let mut conn = ledger();
        let account = cash(&conn);
        let id = add_operation(
            &mut conn,
            &NewOperation { account_id: account, occurred_on: day("2026-09-10"), effect_minor: -1000, who: "Deposit".into(), category: None },
        )
        .unwrap();
        let in_flows = |c: &Connection| query::flows(c, "2026-09-01", "2026-09-30").unwrap().len();
        let before = in_flows(&conn);
        set_counting(&mut conn, id, Counting::Excluded).unwrap();
        assert_eq!(in_flows(&conn), before - 1);
        assert_eq!(balance(&conn, "Cash"), -1000, "the money left — the balance is the same");
        set_counting(&mut conn, id, Counting::Passthrough).unwrap();
        assert_eq!(query::operation_by_id(&conn, id).unwrap().unwrap().role, "passthrough");
        set_counting(&mut conn, id, Counting::Counted).unwrap();
        assert_eq!(query::operation_by_id(&conn, id).unwrap().unwrap().role, "expense");
        assert!(set_counting(&mut conn, id, Counting::Counted).is_err(), "it already counts that way");
        assert!(set_counting(&mut conn, 4, Counting::Excluded).is_err(), "a transfer leg");
        assert!(set_counting(&mut conn, 1, Counting::Excluded).is_err(), "a debt payment");
    }

    #[test]
    fn a_row_without_a_sign_cannot_be_counted() {
        let mut conn = ledger();
        conn.execute_batch(
            "insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description)
             values (50,1,'excluded','2026-09-01T00:00:00Z',80000,'EUR',0,'from saldo with no sign')",
        )
        .unwrap();
        assert!(set_counting(&mut conn, 50, Counting::Counted).is_err());
        assert!(set_counting(&mut conn, 50, Counting::Passthrough).is_err());
    }

    /// EUR → RUB: the amounts differ, tying is allowed, a candidate is found;
    /// by hand — both legs at once.
    #[test]
    fn transfers_go_between_currencies_and_can_be_entered_by_hand() {
        let mut conn = ledger();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (9,'Tinkoff','RUB','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (20,2,'expense','2026-09-10T00:00:00Z',10000,'EUR',-10000,'to tinkoff'),
               (21,9,'income','2026-09-11T00:00:00Z',1000000,'RUB',1000000,'from revolut');",
        )
        .unwrap();
        let found: Vec<i64> = transfer_candidates(&conn, 20).unwrap().iter().map(|l| l.id).collect();
        assert!(found.contains(&21), "{found:?}");
        pair_transfer(&mut conn, 20, 21).unwrap();
        assert!(crate::verify::run(&conn).unwrap().ok());

        let (a, b) = add_transfer(&mut conn, 1, 2, 2500, 2500, day("2026-09-12"), "top-up").unwrap();
        assert_eq!(query::operation_by_id(&conn, a).unwrap().unwrap().pair_account.as_deref(), Some("Revolut"));
        assert_eq!(query::operation_by_id(&conn, b).unwrap().unwrap().role, "transfer");
        assert!(add_transfer(&mut conn, 1, 2, 2500, 2400, day("2026-09-12"), "").is_err(), "one currency, two sums");
        assert!(add_transfer(&mut conn, 1, 1, 100, 100, day("2026-09-12"), "").is_err(), "same account");
        assert!(crate::verify::run(&conn).unwrap().ok());
    }

    /// A receipt of 12.50: 10 on food, the rest on the household. The flows see
    /// the parts, drilling into «Food» finds the operation; undo clears the split.
    #[test]
    fn an_operation_is_split_by_category_in_the_flows() {
        let mut conn = ledger();
        let account = cash(&conn);
        let id = add_operation(
            &mut conn,
            &NewOperation { account_id: account, occurred_on: day("2026-09-10"), effect_minor: -1250, who: "Kaufland".into(), category: None },
        )
        .unwrap();
        let parts = parse_split("Food 10; Household", 1250).unwrap();
        assert_eq!(parts, [("Food".to_string(), 1000), ("Household".to_string(), 250)]);
        assert!(parse_split("Food 10; Home 5", 1250).is_err(), "does not add up");
        assert!(parse_split("Food; Home", 1250).is_err(), "two without an amount");
        split(&mut conn, id, &parts).unwrap();
        let flows = query::flows(&conn, "2026-09-01", "2026-09-30").unwrap();
        let food = flows.iter().find(|f| f.label == "Food").unwrap();
        assert_eq!(food.out.in_display_order(None)[0].minor(), 1000);
        assert!(flows.iter().all(|f| f.label != "Kaufland"), "the whole is not counted twice");
        let drilled = query::flow_operations(&conn, "Household", "2026-09-01", "2026-09-30").unwrap();
        assert_eq!(drilled.iter().map(|o| o.id).collect::<Vec<_>>(), [id]);
        let entry = crate::journal::entries(&conn, 1).unwrap().remove(0);
        crate::journal::undo(&mut conn, entry.id).unwrap();
        assert!(split_of(&conn, id).unwrap().is_empty());
    }

    fn remaining(conn: &Connection) -> i64 {
        conn.query_row("select remaining_minor from debt_remaining", [], |r| r.get(0)).unwrap()
    }

    fn balance(conn: &Connection, name: &str) -> i64 {
        query::balances(conn).unwrap().into_iter().find(|a| a.name == name).unwrap().balance.minor()
    }

    /// Remove a duplicate: the balance and the debt become what they would be without it.
    #[test]
    fn removing_a_duplicate_takes_it_out_of_the_balance_and_the_debt() {
        let mut conn = ledger();
        assert_eq!(remaining(&conn), 220000 - 72000 - 72000 - 2393);
        let before = balance(&conn, "iCard");
        let removed = remove_operation(&mut conn, 2).unwrap();
        assert_eq!(removed, Removed { operations: 1, debt_movements: 1 });
        assert_eq!(balance(&conn, "iCard"), before + 72000, "the duplicate no longer lowers the balance");
        assert_eq!(remaining(&conn), 220000 - 72000 - 2393, "and does not lower the debt");
        assert!(crate::verify::run(&conn).unwrap().ok());
    }

    /// What was removed shows in the journal — otherwise it could not be brought back.
    #[test]
    fn a_removed_operation_stays_visible_and_comes_back() {
        let mut conn = ledger();
        remove_operation(&mut conn, 2).unwrap();
        let journal = query::operations(&conn, None, true).unwrap();
        assert!(journal.iter().any(|o| o.id == 2 && o.removed));
        let counted = query::operations(&conn, None, false).unwrap();
        assert!(!counted.iter().any(|o| o.id == 2), "it is not in the counts");
        let before = remaining(&conn);
        assert_eq!(restore_operation(&mut conn, 2).unwrap(), 1);
        assert_eq!(remaining(&conn), before - 72000, "back — and paying the debt down again");
    }

    /// A transfer's legs are removed together only — otherwise the money would
    /// leave one account and arrive nowhere.
    #[test]
    fn a_transfer_leg_takes_its_partner_with_it() {
        let mut conn = ledger();
        assert_eq!(remove_operation(&mut conn, 4).unwrap().operations, 2);
        assert!(crate::verify::run(&conn).unwrap().ok());
        assert_eq!(restore_operation(&mut conn, 5).unwrap(), 2, "and they come back together");
    }

    #[test]
    fn removing_twice_or_restoring_what_was_not_removed_is_refused() {
        let mut conn = ledger();
        remove_operation(&mut conn, 2).unwrap();
        assert!(remove_operation(&mut conn, 2).is_err());
        assert!(restore_operation(&mut conn, 1).is_err());
    }

    /// «This is not a debt payment»: the movement is off, the debt recomputed,
    /// and the utilities payment became an ordinary spend that enters the flows.
    #[test]
    fn unlinking_a_wrong_repayment_gives_the_debt_back_and_the_spending_too() {
        let mut conn = ledger();
        let before = remaining(&conn);
        let unlinked = unlink_movement(&mut conn, 4).unwrap();
        assert_eq!(unlinked.role_now.as_deref(), Some("expense"));
        assert_eq!(remaining(&conn), before + 2393);
        let flows = query::flows(&conn, "2026-06-01", "2026-06-30").unwrap();
        assert_eq!(flows.len(), 1, "23.93 is back among the spends");
    }

    #[test]
    fn an_own_category_is_set_and_cleared() {
        let conn = ledger();
        set_own_category(&conn, 3, Some("Utilities")).unwrap();
        let category: Option<String> = conn
            .query_row(
                "select c.name from operation o join category c on c.id = o.category_id where o.id = 3",
                [],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(category.as_deref(), Some("Utilities"));
        set_own_category(&conn, 3, None).unwrap();
        let id: Option<i64> =
            conn.query_row("select category_id from operation where id = 3", [], |r| r.get(0)).unwrap();
        assert_eq!(id, None);
    }

    #[test]
    fn split_operations_refuse_role_changes_and_duplicate_split_categories_merge_cleanly() {
        let mut conn = ledger();
        unlink_movement(&mut conn, 4).unwrap();
        split(&mut conn, 3, &[("Food".into(), 1000), ("Food".into(), 1393)]).unwrap();
        assert_eq!(split_of(&conn, 3).unwrap(), vec![("Food".into(), 2393)], "duplicate split categories are combined");
        assert!(set_counting(&mut conn, 3, Counting::Excluded).is_err(), "split operation cannot become excluded");
        assert!(set_counting(&mut conn, 3, Counting::Passthrough).is_err(), "split operation cannot become transit");
        assert!(crate::debt::link(&mut conn, 1, 3).is_err(), "split operation cannot be linked to a debt");
    }

    #[test]
    fn removing_an_issue_that_would_leave_a_debt_negative_is_refused() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'Revolut','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'income', '2026-10-01T00:00:00Z',10000,'EUR', 10000,'Loan'),
               (2,1,'expense','2026-10-05T00:00:00Z',10000,'EUR',-10000,'Repay');",
        )
        .unwrap();
        let debt_id = crate::debt::open(
            &mut conn,
            &crate::debt::NewDebt {
                who: "Alex".into(),
                direction: "I_OWE".into(),
                currency: "EUR".into(),
                opened_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                due_on: None,
                opening_minor: 0,
                note: None,
            },
            Some(1),
        )
        .unwrap();
        crate::debt::link(&mut conn, debt_id, 2).unwrap();
        assert!(remove_operation(&mut conn, 1).is_err(), "removing the issue while repayment stands would make debt negative");
        assert!(crate::verify::run(&conn).unwrap().ok());
    }
}
