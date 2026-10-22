//! Writing a parsed statement into the database.
//!
//! Three rules that matter here more than convenience.
//!
//! **All or nothing.** One transaction per import. Half a statement in the
//! ledger is worse than none: none is visible, half looks like the whole.
//!
//! **Idempotent by content.** The key is the file's `sha256`, not its name: a
//! name is what the browser gave the download and means nothing. The same file
//! a second time creates nothing and says so.
//!
//! **The original is kept.** The file is copied into an archive beside the
//! database before anything is written. That is what `saldo` lacks, and it
//! guards against its known hole: the dedup key depends on the description, so
//! improving the cleanup of descriptions re-opens the history to duplicates.
//! With the archive the parser can be improved and everything run again.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::{ParsedRow, Statement};
use crate::money::Currency;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub file_id: i64,
    /// This file was loaded already. Nothing is written, and it is not an error.
    pub already_imported: bool,
    pub rows_recorded: usize,
    pub operations_created: usize,
    /// Rows whose signature is already in the ledger from another file. That is
    /// what overlapping statement periods look like.
    pub duplicates_skipped: usize,
    /// Pending rows. They do not become operations: they are not money
    /// (invariant 6), and in the next statement they arrive completed — and
    /// load then. Writing them now means a duplicate in a month.
    pub pending_skipped: usize,
    /// New rows a person decided not to write.
    pub declined: usize,
    /// Written as a transfer's leg and tied to the second leg on another account.
    pub transfers_paired: usize,
    /// Written as a debt movement.
    pub debt_movements: usize,
    pub archived_at: PathBuf,
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| crate::tr!("cannot read {}", "не читается {}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// A row's signature — the key for finding duplicates **between** files.
///
/// Inside a file a row is identified by the pair «file, row number», which
/// depends on neither the description nor its cleanup. Between files there is
/// no such pair, so the content is compared: account, date, amount, payee key.
///
/// **The description enters the signature normalised, as a counterparty key**,
/// not as raw text. saldo stores an iCard description cleaned up, potok as it
/// is printed in the PDF, with `POS Payment *1234,` in front. By the raw text
/// one and the same purchase gave two signatures, and the move from saldo would
/// have doubled on the first import. By the key both give `kaufland`. This is
/// saldo's known weak spot — «improving the cleanup of descriptions re-opens
/// the history to duplicates» — moved one level down: the text cleanup can now
/// change without touching signatures; only a change of normalisation touches
/// them, and for that the archive of originals is there.
///
/// **The amount carries no sign**, as in saldo and for the same reason: a
/// direction is sometimes restored rather than read. In Revolut the sign comes
/// from the balance step; saldo's «excluded» rows have no sign at all — a zero effect.
pub fn signature(account_id: i64, row: &ParsedRow) -> String {
    signature_of(account_id, row.occurred_on, row.amount.minor(), &row.description)
}

/// The same for rows that came from no statement — the move from saldo.
pub fn signature_of(
    account_id: i64,
    occurred_on: chrono::NaiveDate,
    minor: i64,
    description: &str,
) -> String {
    format!(
        "{}|{}|{}|{}",
        account_id,
        occurred_on,
        minor.unsigned_abs(),
        crate::counterparty::normalize(description)
    )
}

pub(crate) struct Account {
    pub id: i64,
    pub currency: Currency,
}

/// The same signature has already become an operation from another file —
/// overlapping statement periods. `file_id` is this file, if already written:
/// rows of one file are not duplicates of each other, two 0.50 fees are real.
pub(crate) fn seen_elsewhere(conn: &Connection, signature: &str, file_id: Option<i64>) -> Result<bool> {
    Ok(conn.query_row(
        "select exists (select 1 from operation o join import_row r on r.id = o.import_row_id
                         where r.signature = ?1 and r.file_id <> ?2)",
        params![signature, file_id.unwrap_or(-1)],
        |r| r.get(0),
    )?)
}

pub(crate) fn find_account(conn: &Connection, name: &str) -> Result<Account> {
    let found: Option<(i64, String)> = conn
        .query_row(
            "select id, currency from account where name = ?1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((id, currency)) = found else {
        anyhow::bail!(crate::tr!(
            "no account {name:?}. Create it: potok account add {name:?} --currency EUR --kind BANK",
            "нет счёта {name:?}. Создать: potok account add {name:?} --currency EUR --kind BANK"
        ))
    };
    Ok(Account { id, currency: Currency::new(&currency)? })
}

/// Copies the original into the archive under a name from its own hash.
///
/// Done before the transaction: an orphaned file in the archive is harmless —
/// it is addressed by its content — while a write with no original takes away
/// the chance to run the parsing again.
fn archive(path: &Path, sha: &str, archive_dir: &Path) -> Result<PathBuf> {
    // Statements only, named by their hash: the folder is potok's.
    crate::db::private_dir(archive_dir, |_| true)
        .with_context(|| crate::tr!("cannot create the archive {}", "не создаётся архив {}", archive_dir.display()))?;
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let target = archive_dir.join(format!("{sha}.{extension}"));
    if !target.exists() {
        std::fs::copy(path, &target)
            .with_context(|| crate::tr!("cannot copy to {}", "не копируется в {}", target.display()))?;
    }
    crate::db::owner_only(&target)?;
    Ok(target)
}

/// Writes a statement as decided in `plan` (`review::plan`): new rows as
/// operations, accepted hints as a transfer or a debt movement.
///
/// The plan may have gone stale while a person looked at it: the second leg
/// was tied by hand in another window. Then the tying refuses and the whole
/// import rolls back — half a statement is worse than none.
pub fn apply(
    conn: &mut Connection,
    path: &Path,
    statement: &Statement,
    plan: &super::review::Plan,
    archive_dir: &Path,
) -> Result<Outcome> {
    use super::review::{Hint, RowStatus};
    anyhow::ensure!(
        plan.rows.len() == statement.rows.len(),
        crate::tr!("the plan is not for this statement", "план не от этой выписки")
    );
    let account = find_account(conn, &plan.account)?;
    let sha = sha256_of(path)?;
    anyhow::ensure!(
        sha == plan.sha,
        crate::tr!("the file changed after review — review it again", "файл изменился после разбора — разбери заново")
    );

    // The file is loaded already — do nothing and say so.
    if let Some(file_id) = conn
        .query_row("select id from import_file where sha256 = ?1", params![sha], |r| r.get::<_, i64>(0))
        .optional()?
    {
        let archived_at: String = conn.query_row(
            "select archived_at from import_file where id = ?1",
            params![file_id],
            |r| r.get(0),
        )?;
        return Ok(Outcome {
            file_id,
            already_imported: true,
            rows_recorded: 0,
            operations_created: 0,
            duplicates_skipped: 0,
            pending_skipped: 0,
            declined: 0,
            transfers_paired: 0,
            debt_movements: 0,
            archived_at: PathBuf::from(archived_at),
        });
    }

    let archived_at = archive(path, &sha, archive_dir)?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string();

    let transaction = conn.transaction()?;
    transaction.execute(
        "insert into import_file (sha256, bank, filename, archived_at, account_id)
         values (?1, ?2, ?3, ?4, ?5)",
        params![sha, statement.bank.as_str(), filename, archived_at.to_string_lossy(), account.id],
    )?;
    let file_id = transaction.last_insert_rowid();

    let mut outcome = Outcome {
        file_id,
        already_imported: false,
        rows_recorded: 0,
        operations_created: 0,
        duplicates_skipped: 0,
        pending_skipped: 0,
        declined: 0,
        transfers_paired: 0,
        debt_movements: 0,
        archived_at: archived_at.clone(),
    };

    for (row, planned) in statement.rows.iter().zip(&plan.rows) {
        let signature = signature(account.id, row);
        transaction.execute(
            "insert into import_row (file_id, line_no, raw, signature, pending)
             values (?1, ?2, ?3, ?4, ?5)",
            params![file_id, row.line_no as i64, row.raw, signature, !row.posted],
        )?;
        let row_id = transaction.last_insert_rowid();
        outcome.rows_recorded += 1;

        if !row.posted {
            outcome.pending_skipped += 1;
            continue;
        }

        // The same signature in another file, already turned into an operation,
        // is overlapping statement periods rather than a new spend.
        if seen_elsewhere(&transaction, &signature, Some(file_id))? {
            outcome.duplicates_skipped += 1;
            continue;
        }
        if planned.status != RowStatus::New || !planned.write {
            outcome.declined += 1;
            continue;
        }

        // iCard moving money between two balances of the same card prints
        // both halves — «Balance transfer» +500 and −500 — on one statement.
        // Counted as spending and income they put 500 into each side of the
        // month (30.09.2026); as transit the balance is the same and the
        // flows are what really came and went.
        let internal = super::moves_within_the_account(statement.bank, &row.description);
        // Money back from a purchase takes the spending down (an expense with
        // a plus), and belongs to the shop it came from when that shop is known.
        let refund = row.amount.minor() > 0 && crate::counterparty::refund_of(&row.description).is_some();
        let role = if internal {
            "passthrough"
        } else if row.amount.minor() < 0 || refund {
            "expense"
        } else {
            "income"
        };
        // A counterparty is recognised by the normalised description. No category
        // is set here: it lives on the counterparty and is read through
        // `coalesce`, so marking after the fact rewrites nothing.
        let counterparty_id = match refund.then(|| crate::counterparty::refund_counterparty(&transaction, &row.description)).transpose()?.flatten() {
            Some(shop) => Some(shop),
            None => crate::counterparty::resolve_or_create(&transaction, &row.description)?,
        };
        transaction.execute(
            "insert into operation
                (account_id, role, state, occurred_at, amount_minor, currency,
                 effect_minor, description, counterparty_id, import_row_id)
             values (?1, ?2, 'POSTED', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                account.id,
                role,
                format!("{}T00:00:00Z", row.occurred_on),
                row.amount.minor().abs(),
                account.currency.as_str(),
                row.amount.minor(),
                row.description,
                counterparty_id,
                row_id,
            ],
        )?;
        let operation_id = transaction.last_insert_rowid();
        outcome.operations_created += 1;

        match (&planned.hint, planned.accept_hint) {
            (Some(Hint::Transfer { with }), true) => {
                crate::edit::pair_in(&transaction, operation_id, with.id).map_err(|_| {
                    anyhow::anyhow!(crate::tr!(
                        "the other transfer leg #{} is taken already — review the statement again",
                        "вторая нога перевода #{} уже занята — разбери выписку заново",
                        with.id
                    ))
                })?;
                outcome.transfers_paired += 1;
            }
            (Some(Hint::Debt { debt_id, kind, .. }), true) => {
                if *kind == "REPAYMENT" {
                    let remaining: i64 = transaction
                        .query_row("select remaining_minor from debt_remaining where debt_id = ?1", [debt_id], |r| r.get(0))
                        .optional()?
                        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no debt #{debt_id}", "нет долга #{debt_id}")))?;
                    if row.amount.minor().abs() > remaining {
                        anyhow::bail!(crate::tr!(
                            "a repayment of {} is more than the {} left on debt #{debt_id}",
                            "возврат {} больше остатка {} по долгу #{debt_id}",
                            crate::money::format_minor(row.amount.minor().abs()),
                            crate::money::format_minor(remaining)
                        ));
                    }
                }
                transaction.execute("update operation set role = 'debt_movement' where id = ?1", [operation_id])?;
                transaction.execute(
                    "insert into debt_movement (debt_id, operation_id, kind, amount_minor, occurred_on, note)
                     values (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        debt_id,
                        operation_id,
                        kind,
                        row.amount.minor().abs(),
                        row.occurred_on.to_string(),
                        crate::tr!("import", "импорт")
                    ],
                )?;
                outcome.debt_movements += 1;
            }
            _ => {}
        }
    }

    crate::journal::log(
        &transaction,
        &crate::tr!(
            "statement «{filename}» imported to {}: {} operations, {} transfers, {} debt payments",
            "загружена выписка «{filename}» на {}: операций {}, переводов {}, по долгам {}",
            plan.account, outcome.operations_created, outcome.transfers_paired, outcome.debt_movements
        ),
        Some(crate::journal::Undo::Unimport(file_id)),
    )?;
    transaction.commit()?;
    Ok(outcome)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolledBack {
    pub filename: String,
    pub operations: usize,
    /// Operations on other accounts that stopped being a transfer's second leg.
    pub partners_unpaired: usize,
    pub debt_movements: usize,
}

/// Rolls a load back whole: its operations, their ties to debts and transfers,
/// the journal rows and the file itself. After that the same file can load again.
///
/// The operations are deleted rather than marked as removed (invariant 8 is
/// about something else): this is not correcting what is posted but undoing a
/// write just made — the original lies in the archive. The move from saldo does
/// not roll back this way: it is the whole ledger.
pub fn rollback(conn: &mut Connection, file_id: i64) -> Result<RolledBack> {
    let transaction = conn.transaction()?;
    let (bank, filename): (String, String) = transaction
        .query_row("select bank, filename from import_file where id = ?1", [file_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .with_context(|| crate::tr!("no import #{file_id}", "нет загрузки #{file_id}"))?;
    anyhow::ensure!(
        bank != "saldo-migration",
        crate::tr!(
            "the saldo migration is the whole ledger — it is not rolled back",
            "перенос из saldo — это весь леджер, его не откатывают"
        )
    );

    let mut stmt = transaction.prepare(
        "select o.id from operation o join import_row r on r.id = o.import_row_id where r.file_id = ?1",
    )?;
    let ids = stmt.query_map([file_id], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let mut partners_unpaired = 0;
    let mut debt_movements = 0;
    let mut affected_debts: Vec<i64> = Vec::new();
    for id in &ids {
        let mut d_stmt = transaction.prepare("select debt_id from debt_movement where operation_id = ?1")?;
        for d in d_stmt.query_map([id], |r| r.get::<_, i64>(0))? {
            affected_debts.push(d?);
        }
        drop(d_stmt);
        debt_movements += transaction.execute("delete from debt_movement where operation_id = ?1", [id])?;
        // The second leg on another account stays — its money is real — and
        // becomes an ordinary spend or income.
        partners_unpaired += transaction.execute(
            "update operation set pair_id = null,
                    role = case when effect_minor < 0 then 'expense' else 'income' end
              where pair_id = ?1 and coalesce(import_row_id, -1) not in (select id from import_row where file_id = ?2)",
            params![id, file_id],
        )?;
    }
    for debt_id in affected_debts {
        let neg: bool = transaction.query_row(
            "select exists (select 1 from debt_remaining where debt_id = ?1 and remaining_minor < 0)",
            [debt_id],
            |r| r.get(0),
        )?;
        if neg {
            anyhow::bail!(crate::tr!(
                "rolling back this statement would leave a debt below zero — unlink its later repayments first",
                "откат этой выписки уведёт остаток долга ниже нуля — сначала отвязать более поздние возвраты"
            ));
        }
    }
    for id in &ids {
        transaction.execute("update operation set pair_id = null where id = ?1", [id])?;
    }
    for id in &ids {
        transaction.execute("delete from operation where id = ?1", [id])?;
    }
    transaction.execute("delete from import_file where id = ?1", [file_id])?;
    crate::journal::log(&transaction, &crate::tr!("statement «{filename}» rolled back: {} operations", "откачена выписка «{filename}»: операций {}", ids.len()), None)?;
    transaction.commit()?;
    Ok(RolledBack { filename, operations: ids.len(), partners_unpaired, debt_movements })
}

/// Creating an account. Without one there is nowhere to import, hence it lives here.
pub fn create_account(
    conn: &Connection,
    name: &str,
    currency: Currency,
    kind: &str,
    opening_minor: i64,
) -> Result<i64> {
    conn.execute(
        "insert into account (name, currency, kind, opening_minor)
         values (?1, ?2, ?3, ?4)",
        params![name, currency.as_str(), kind, opening_minor],
    )
    .with_context(|| crate::tr!("account {name:?} was not created — perhaps it exists", "счёт {name:?} не создался — возможно, он уже есть"))?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, import, query};

    /// The same shape as in the parser's tests: opening 100.00, 30.00 out,
    /// 5.00 in, 75.00 at the end, plus one pending of 6.60.
    const SAMPLE: &str = "\
EUR Statement
Revolut Bank UAB
 Account (Current Account) €100.00 €30.00 €5.00 €75.00
Date Description Money out Money in Balance
Sep 1, 2026 Shop €20.00 €80.00
Sep 2, 2026 Refund €5.00 €85.00
Sep 3, 2026 Coffee €10.00 €75.00
 Start date Description Money out Money in
Oct 16, 2026 Hosting €6.60
";

    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("potok-store-{}-{label}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }

        fn file(&self, name: &str, content: &str) -> PathBuf {
            let path = self.dir.join(name);
            std::fs::write(&path, content).unwrap();
            path
        }

        fn archive(&self) -> PathBuf {
            self.dir.join("archive")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A whole import, as from the command line: parsing and writing with every
    /// hint. «Today» is in the far future, so that the statement's date does not
    /// turn out to be «from the future».
    fn import_as(
        conn: &mut Connection,
        account: &str,
        path: &Path,
        statement: &import::Statement,
        archive: &Path,
    ) -> Result<Outcome> {
        let plan = import::review::plan(conn, account, path, statement, chrono::NaiveDate::MAX)?;
        apply(conn, path, statement, &plan, archive)
    }

    fn ledger_with_account(opening_minor: i64) -> Connection {
        let conn = db::open_memory().unwrap();
        create_account(&conn, "Revolut", Currency::new("EUR").unwrap(), "BANK", opening_minor)
            .unwrap();
        conn
    }

    /// The main test: after the load the account's balance equals the total the
    /// statement printed about itself. If that agrees, the import lost nothing
    /// and invented nothing.
    #[test]
    fn after_an_import_the_balance_equals_what_the_statement_declares() {
        let scratch = Scratch::new("balance");
        let path = scratch.file("statement.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let totals = import::revolut::declared_totals(SAMPLE).unwrap();

        let mut conn = ledger_with_account(totals.opening.minor());
        let outcome =
            import_as(&mut conn, "Revolut", &path, &statement, &scratch.archive()).unwrap();

        assert_eq!(outcome.operations_created, 3);
        let balances = query::balances(&conn).unwrap();
        assert_eq!(
            balances[0].balance.minor(),
            totals.closing.minor(),
            "the balance after an import must equal the statement's closing total"
        );
    }

    /// Idempotent by content: the same file a second time creates nothing.
    #[test]
    fn the_same_file_twice_changes_nothing_the_second_time() {
        let scratch = Scratch::new("twice");
        let path = scratch.file("statement.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let mut conn = ledger_with_account(10000);

        let first = import_as(&mut conn, "Revolut", &path, &statement, &scratch.archive()).unwrap();
        assert!(!first.already_imported);
        assert_eq!(first.operations_created, 3);

        let second = import_as(&mut conn, "Revolut", &path, &statement, &scratch.archive()).unwrap();
        assert!(second.already_imported);
        assert_eq!(second.operations_created, 0);
        assert_eq!(second.rows_recorded, 0);
        assert_eq!(second.file_id, first.file_id);

        let count: i64 =
            conn.query_row("select count(*) from operation", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 3, "the operations must not double");
    }

    /// A file's name means nothing — the key is in the content. A renamed copy
    /// of the same statement does not load again.
    #[test]
    fn a_renamed_copy_is_still_the_same_file() {
        let scratch = Scratch::new("renamed");
        let first_path = scratch.file("september.txt", SAMPLE);
        let second_path = scratch.file("september-copy.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let mut conn = ledger_with_account(10000);

        import_as(&mut conn, "Revolut", &first_path, &statement, &scratch.archive()).unwrap();
        let again =
            import_as(&mut conn, "Revolut", &second_path, &statement, &scratch.archive()).unwrap();
        assert!(again.already_imported, "the same statement under another name");
    }

    /// The original must end up in the archive — on it rests the chance to run
    /// the parsing again when the parser gets better.
    #[test]
    fn the_original_file_lands_in_the_archive_under_its_own_hash() {
        let scratch = Scratch::new("archive");
        let path = scratch.file("statement.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let mut conn = ledger_with_account(10000);

        let outcome = import_as(&mut conn, "Revolut", &path, &statement, &scratch.archive()).unwrap();
        assert!(outcome.archived_at.exists(), "the file is not in the archive");
        assert_eq!(
            std::fs::read_to_string(&outcome.archived_at).unwrap(),
            SAMPLE,
            "the archive must hold the original byte for byte"
        );
        let sha = sha256_of(&path).unwrap();
        assert!(
            outcome.archived_at.to_string_lossy().contains(&sha),
            "the name in the archive is the content's hash"
        );
    }

    /// A pending row enters the import's journal but becomes no operation: it is
    /// not money, and in the next statement it arrives completed.
    #[test]
    fn a_pending_row_is_recorded_but_is_not_an_operation() {
        let scratch = Scratch::new("pending");
        let path = scratch.file("statement.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let mut conn = ledger_with_account(10000);

        let outcome = import_as(&mut conn, "Revolut", &path, &statement, &scratch.archive()).unwrap();
        assert_eq!(outcome.rows_recorded, 4, "every row goes into the journal");
        assert_eq!(outcome.operations_created, 3);
        assert_eq!(outcome.pending_skipped, 1);
    }

    /// Overlapping periods: the second statement repeats a row of the first. The
    /// same signature from another file is not a new spend.
    #[test]
    fn a_row_already_in_the_ledger_from_another_file_is_not_imported_twice() {
        let scratch = Scratch::new("overlap");
        let first = scratch.file("september.txt", SAMPLE);
        // The second statement: the first row the same, the second new.
        let overlapping = "\
EUR Statement
Revolut Bank UAB
 Account (Current Account) €75.00 €10.00 €0.00 €65.00
Date Description Money out Money in Balance
Sep 3, 2026 Coffee €10.00 €65.00
Sep 4, 2026 Bread €5.00 €60.00
";
        let second = scratch.file("october.txt", overlapping);
        let mut conn = ledger_with_account(10000);

        import_as(
            &mut conn,
            "Revolut",
            &first,
            &import::parse(SAMPLE).unwrap(),
            &scratch.archive(),
        )
        .unwrap();
        let outcome = import_as(
            &mut conn,
            "Revolut",
            &second,
            &import::parse(overlapping).unwrap(),
            &scratch.archive(),
        )
        .unwrap();

        assert_eq!(outcome.duplicates_skipped, 1, "the Coffee row is in the ledger already");
        assert_eq!(outcome.operations_created, 1, "only Bread should be written");
    }

    /// Invariant 4: a statement in one currency does not load onto an account in
    /// another, and the refusal happens before writing rather than halfway.
    #[test]
    fn a_statement_in_another_currency_is_refused_before_anything_is_written() {
        let scratch = Scratch::new("currency");
        let path = scratch.file("statement.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let mut conn = db::open_memory().unwrap();
        create_account(&conn, "Till", Currency::new("BGN").unwrap(), "CASH", 0).unwrap();

        let refused = import_as(&mut conn, "Till", &path, &statement, &scratch.archive());
        assert!(refused.is_err());
        let files: i64 =
            conn.query_row("select count(*) from import_file", [], |r| r.get(0)).unwrap();
        assert_eq!(files, 0, "the import journal must stay empty");
    }

    /// A non-existent account — an error that says how to fix it.
    #[test]
    fn an_unknown_account_says_how_to_create_it() {
        let scratch = Scratch::new("noaccount");
        let path = scratch.file("statement.txt", SAMPLE);
        let statement = import::parse(SAMPLE).unwrap();
        let mut conn = db::open_memory().unwrap();

        let error = import_as(&mut conn, "No such thing", &path, &statement, &scratch.archive())
            .unwrap_err()
            .to_string();
        assert!(error.contains("potok account add"), "{error}");
    }

    /// A signature must not depend on how the bank placed the spaces and the
    /// case: it is the same money on the same day.
    #[test]
    fn a_signature_ignores_spacing_and_case() {
        let statement = import::parse(SAMPLE).unwrap();
        let row = statement.posted().next().unwrap();
        let mut noisy = row.clone();
        noisy.description = "  SHOP   ".to_string();
        assert_eq!(signature(1, row), signature(1, &noisy));
    }

    /// What the description is normalised in the signature for: saldo's cleaned
    /// row and the PDF's raw row are one purchase, not two.
    #[test]
    fn a_cleaned_description_and_the_raw_one_sign_the_same() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        assert_eq!(
            signature_of(1, date, -3115, "POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR"),
            signature_of(1, date, -3115, "Kaufland"),
        );
    }

    /// The amount carries no sign: saldo's «excluded» row has a zero effect, and
    /// the sign of the original statement row is unknown.
    #[test]
    fn the_sign_of_an_amount_is_not_part_of_its_signature() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
        assert_eq!(signature_of(1, date, -80000, "X"), signature_of(1, date, 80000, "X"));
    }

    /// One and the same file onto two different accounts is different signatures,
    /// because the account is in the key. Else a second card's statement would double.
    #[test]
    fn the_same_row_on_a_different_account_is_a_different_signature() {
        let statement = import::parse(SAMPLE).unwrap();
        let row = statement.posted().next().unwrap();
        assert_ne!(signature(1, row), signature(2, row));
    }
}
