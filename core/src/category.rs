//! Categories by hand: the list with what is in each, rename, merge, delete.
//!
//! A category is assigned on a counterparty (§7a), on one operation as its
//! own, or on a part of a split operation. Every change here moves all three.
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::journal::{self, Undo};
use crate::{tr, MoneyBag};

#[derive(Debug, Clone)]
pub struct CategoryLine {
    pub id: i64,
    pub name: String,
    /// Counterparties carrying it.
    pub counterparties: usize,
    /// Operations counted in it: through their counterparty, their own, or a
    /// split part.
    pub operations: usize,
    /// Spent in it from `from` to `to`.
    pub spent: MoneyBag,
    /// Come in under it — a salary category has nothing spent.
    pub received: MoneyBag,
}

/// All categories, the most spent first, then by name.
pub fn list(conn: &Connection, from: &str, to: &str) -> Result<Vec<CategoryLine>> {
    let mut stmt = conn.prepare(
        "select c.id, c.name,
                (select count(*) from counterparty p where p.category_id = c.id),
                (select count(*) from operation o left join counterparty p on p.id = o.counterparty_id
                  where o.state = 'POSTED' and coalesce(o.category_id, p.category_id) = c.id
                    and not exists (select 1 from operation_split s where s.operation_id = o.id))
                + (select count(*) from operation_split s join operation o on o.id = s.operation_id
                    where s.category_id = c.id and o.state = 'POSTED')
           from category c order by c.name",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let flows = crate::query::flows(conn, from, to)?;
    let mut out: Vec<CategoryLine> = rows
        .into_iter()
        .map(|(id, name, counterparties, operations)| CategoryLine {
            spent: flows.iter().find(|f| f.label == name).map(|f| f.out.clone()).unwrap_or_default(),
            received: flows.iter().find(|f| f.label == name).map(|f| f.inn.clone()).unwrap_or_default(),
            id,
            name,
            counterparties: counterparties as usize,
            operations: operations as usize,
        })
        .collect();
    // In the flows' order — the most spent first, in the base currency; the
    // rest by name. Summing minor units across currencies is not an order.
    out.sort_by_key(|c| {
        let at = flows.iter().position(|f| f.label == c.name && !f.out.is_empty()).unwrap_or(usize::MAX);
        (at, c.name.to_lowercase())
    });
    Ok(out)
}

fn id_of(conn: &Connection, name: &str) -> Result<i64> {
    conn.query_row("select id from category where name = ?1", [name], |r| r.get(0))
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(tr!("no category «{name}»", "нет категории «{name}»")))
}

pub fn rename(conn: &Connection, from: &str, to: &str) -> Result<()> {
    let to = to.trim();
    crate::db::atomic(conn, || {
        let id = id_of(conn, from)?;
        if to.is_empty() || to == from {
            bail!(tr!("the new name is empty or the same", "новое имя пустое или то же самое"));
        }
        if id_of(conn, to).is_ok() {
            bail!(tr!(
                "«{to}» exists — merge into it instead",
                "«{to}» уже есть — тогда свести в неё"
            ));
        }
        conn.execute("update category set name = ?1 where id = ?2", params![to, id])?;
        journal::log(
            conn,
            &tr!("category «{from}» renamed to «{to}»", "категория «{from}» переименована в «{to}»"),
            Some(Undo::RenameCategory(to.to_string(), from.to_string())),
        )?;
        Ok(())
    })
}

/// Move everything from one category to another and drop the first.
pub fn merge(conn: &mut Connection, from: &str, into: &str) -> Result<usize> {
    let (a, b) = (id_of(conn, from)?, id_of(conn, into)?);
    if a == b {
        bail!(tr!("that is the same category", "это одна и та же категория"));
    }
    let transaction = conn.transaction()?;
    let mut moved = 0;
    for table in ["operation", "counterparty"] {
        moved += transaction.execute(&format!("update {table} set category_id = ?1 where category_id = ?2"), [b, a])?;
    }
    // If an operation was split into both `from` and `into`, combine their
    // amounts before moving the rest so `(operation_id, category_id)` stays unique.
    moved += transaction.execute(
        "update operation_split
            set amount_minor = amount_minor + (
                select coalesce(sum(s.amount_minor), 0) from operation_split s
                 where s.operation_id = operation_split.operation_id and s.category_id = ?2
            )
          where category_id = ?1
            and exists (
                select 1 from operation_split s
                 where s.operation_id = operation_split.operation_id and s.category_id = ?2
            )",
        [b, a],
    )?;
    transaction.execute(
        "delete from operation_split
          where category_id = ?2
            and exists (
                select 1 from operation_split s
                 where s.operation_id = operation_split.operation_id and s.category_id = ?1
            )",
        [b, a],
    )?;
    moved += transaction.execute("update operation_split set category_id = ?1 where category_id = ?2", [b, a])?;
    transaction.execute("update budget set category_id = ?1 where category_id = ?2 and not exists (select 1 from budget x where x.category_id = ?1)", [b, a])?;
    transaction.execute("delete from budget where category_id = ?1", [a])?;
    transaction.execute("delete from category where id = ?1", [a])?;
    journal::log(
        &transaction,
        &tr!("category «{from}» merged into «{into}»: {moved} places", "категория «{from}» сведена в «{into}»: мест {moved}"),
        None,
    )?;
    transaction.commit()?;
    Ok(moved)
}

/// Delete a category: whatever carried it carries none. Refuses if any split
/// operation uses it — deleting one part of a split would either corrupt the
/// split sum or silently destroy the other categories' parts.
pub fn delete(conn: &mut Connection, name: &str) -> Result<()> {
    let id = id_of(conn, name)?;
    let transaction = conn.transaction()?;
    let split_count: i64 = transaction.query_row(
        "select count(distinct operation_id) from operation_split where category_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    if split_count > 0 {
        bail!(tr!(
            "«{name}» is used in {split_count} split operations — merge it into another category or unsplit them first",
            "«{name}» участвует в разбивке {split_count} операций — сведите её в другую категорию или снимите разбивку"
        ));
    }
    transaction.execute("update operation set category_id = null where category_id = ?1", [id])?;
    transaction.execute("update counterparty set category_id = null where category_id = ?1", [id])?;
    transaction.execute("delete from budget where category_id = ?1", [id])?;
    transaction.execute("delete from category where id = ?1", [id])?;
    journal::log(&transaction, &tr!("category «{name}» deleted", "категория «{name}» удалена"), None)?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> Connection {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'Cash','EUR','CASH');
             insert into category (id,name) values (1,'Food'), (2,'Groceries'), (3,'Home');
             insert into counterparty (id,name,category_id) values (1,'Kaufland',2), (2,'Lidl',1);
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id,category_id) values
               (1,1,'expense','2026-09-10T00:00:00Z',1000,'EUR',-1000,1,null),
               (2,1,'expense','2026-09-11T00:00:00Z',500,'EUR',-500,2,null),
               (3,1,'expense','2026-09-12T00:00:00Z',300,'EUR',-300,2,3);",
        )
        .unwrap();
        conn
    }

    fn names(conn: &Connection) -> Vec<(String, usize)> {
        list(conn, "2026-09-01", "2026-09-30").unwrap().into_iter().map(|c| (c.name, c.operations)).collect()
    }

    #[test]
    fn categories_are_listed_renamed_merged_and_deleted() {
        let mut conn = ledger();
        assert_eq!(names(&conn), [("Groceries".into(), 1), ("Food".into(), 1), ("Home".into(), 1)]);
        rename(&conn, "Groceries", "Supermarket").unwrap();
        assert!(rename(&conn, "Supermarket", "Food").is_err(), "taken — merge instead");
        merge(&mut conn, "Supermarket", "Food").unwrap();
        assert_eq!(names(&conn), [("Food".into(), 2), ("Home".into(), 1)]);
        delete(&mut conn, "Home").unwrap();
        assert_eq!(names(&conn), [("Food".into(), 3)], "#3 falls back to Lidl's Food");
        let entry = journal::entries(&conn, 10).unwrap().into_iter().find(|e| e.what.contains("renamed")).unwrap();
        assert!(entry.can_undo);

        // A split operation: a category cannot be deleted out of a split, while
        // folding two categories inside one split adds their parts without loss.
        crate::edit::split(&mut conn, 1, &[("Food".into(), 600), ("Household".into(), 400)]).unwrap();
        assert!(delete(&mut conn, "Household").is_err(), "нельзя молча снести чужую разбивку");
        merge(&mut conn, "Household", "Food").unwrap();
        assert_eq!(crate::edit::split_of(&conn, 1).unwrap(), [("Food".into(), 1000)]);
    }
}
