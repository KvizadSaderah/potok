//! Accounts by hand: create, rename, archive, bring back, delete an empty one.
//!
//! Archiving hides an account from the lists and the totals' rows; it does
//! not touch a single operation. Only an account at zero is archived: one
//! with money on it hidden from view is money forgotten.
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::journal::{self, Undo};
use crate::{tr, Currency};

pub const KINDS: [&str; 4] = ["BANK", "CARD", "CASH", "OTHER"];

fn name_of(conn: &Connection, id: i64) -> Result<String> {
    conn.query_row("select name from account where id = ?1", [id], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(tr!("no account #{id}", "нет счёта #{id}")))
}

pub fn create(conn: &Connection, name: &str, currency: &str, kind: &str, opening_minor: i64) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        bail!(tr!("the account name is empty", "имя счёта пустое"));
    }
    if !KINDS.contains(&kind) {
        bail!(tr!("account kind is one of {:?}", "вид счёта — один из {:?}", KINDS));
    }
    let currency = Currency::new(&currency.trim().to_uppercase())?;
    crate::db::atomic(conn, || {
        let id = crate::import::store::create_account(conn, name, currency, kind, opening_minor)?;
        journal::log(
            conn,
            &tr!("account «{name}» created ({currency})", "создан счёт «{name}» ({currency})"),
            Some(Undo::DeleteAccount(id)),
        )?;
        Ok(id)
    })
}

pub fn rename(conn: &Connection, id: i64, to: &str) -> Result<()> {
    let to = to.trim();
    crate::db::atomic(conn, || {
        let from = name_of(conn, id)?;
        if to.is_empty() || to == from {
            bail!(tr!("the new name is empty or the same", "новое имя пустое или то же самое"));
        }
        let taken: bool = conn.query_row("select exists (select 1 from account where name = ?1)", [to], |r| r.get(0))?;
        if taken {
            bail!(tr!("there is an account «{to}» already", "счёт «{to}» уже есть"));
        }
        conn.execute("update account set name = ?1 where id = ?2", params![to, id])?;
        journal::log(
            conn,
            &tr!("account «{from}» renamed to «{to}»", "счёт «{from}» переименован в «{to}»"),
            Some(Undo::RenameAccount(id, from)),
        )?;
        Ok(())
    })
}

/// Archive or bring back.
pub fn set_archived(conn: &Connection, id: i64, archived: bool) -> Result<()> {
    crate::db::atomic(conn, || {
        let name = name_of(conn, id)?;
        if archived {
            let balance: i64 =
                conn.query_row("select balance_minor from account_balance where account_id = ?1", [id], |r| r.get(0))?;
            if balance != 0 {
                bail!(tr!(
                    "«{name}» holds {} — only an account at zero is archived",
                    "на «{name}» {} — в архив уходит только счёт с нулём",
                    crate::money::format_minor(balance)
                ));
            }
        }
        let state = if archived { "ARCHIVED" } else { "ACTIVE" };
        conn.execute("update account set state = ?1 where id = ?2", params![state, id])?;
        let what = if archived {
            tr!("account «{name}» archived", "счёт «{name}» в архиве")
        } else {
            tr!("account «{name}» back from the archive", "счёт «{name}» возвращён из архива")
        };
        journal::log(conn, &what, Some(Undo::Archive(id, !archived)))?;
        Ok(())
    })
}

/// Delete an account with no operations — the undo of creating one.
pub fn delete_empty(conn: &Connection, id: i64) -> Result<()> {
    crate::db::atomic(conn, || {
        let name = name_of(conn, id)?;
        let used: bool = conn.query_row("select exists (select 1 from operation where account_id = ?1)", [id], |r| r.get(0))?;
        if used {
            bail!(tr!("«{name}» has operations — archive it instead", "у «{name}» есть операции — его можно только в архив"));
        }
        conn.execute("delete from reconciliation where account_id = ?1", [id])?;
        conn.execute("delete from account where id = ?1", [id])?;
        journal::log(conn, &tr!("account «{name}» deleted", "счёт «{name}» удалён"), None)?;
        Ok(())
    })
}

/// Archived accounts, by name.
pub fn archived(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare("select id, name from account where state = 'ARCHIVED' order by name")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, query};

    fn names(conn: &Connection) -> Vec<String> {
        query::balances(conn).unwrap().into_iter().map(|a| a.name).collect()
    }

    #[test]
    fn an_account_is_created_renamed_archived_and_brought_back() {
        let conn = db::open_memory().unwrap();
        let id = create(&conn, "Wise", "eur", "BANK", 0).unwrap();
        assert!(create(&conn, "Wise", "EUR", "BANK", 0).is_err(), "the name is taken");
        assert!(create(&conn, "X", "EURO", "BANK", 0).is_err(), "not a currency");
        rename(&conn, id, "Wise EUR").unwrap();
        assert_eq!(names(&conn), ["Wise EUR"]);
        set_archived(&conn, id, true).unwrap();
        assert!(names(&conn).is_empty());
        assert_eq!(archived(&conn).unwrap(), [(id, "Wise EUR".to_string())]);
        set_archived(&conn, id, false).unwrap();
        assert_eq!(names(&conn), ["Wise EUR"]);
    }

    #[test]
    fn money_is_not_archived_and_used_accounts_are_not_deleted() {
        let conn = db::open_memory().unwrap();
        let id = create(&conn, "Cash", "EUR", "CASH", 1000).unwrap();
        assert!(set_archived(&conn, id, true).is_err());
        conn.execute(
            "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor) values (?1,'expense','2026-10-01',1000,'EUR',-1000)",
            [id],
        )
        .unwrap();
        set_archived(&conn, id, true).unwrap();
        assert!(delete_empty(&conn, id).is_err());
    }
}
