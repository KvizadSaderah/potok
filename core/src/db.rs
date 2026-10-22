//! Opening the database and applying the schema.
//!
//! The schema lives in `schema.sql` and is built into the binary. It is the
//! single source: §4 of the specification is checked against this file by the
//! `spec_sql_matches_schema` test, so the document and the database cannot
//! drift apart — in the old project a document that disagreed with reality
//! was the costliest finding of the audit.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

pub const SCHEMA: &str = include_str!("schema.sql");

/// The schema's version. It changes with every change to `schema.sql`.
///
/// Without it the schema was applied with `create … if not exists`, and a
/// database made by an earlier potok quietly kept its old schema. Not in
/// theory: on 2026-10-18 a ledger from the 16th turned up, empty but holding
/// a `debt_remaining` view that doubled every debt. `create view if not
/// exists` does not replace such a view — the database would have counted
/// debts twice under the new code, silently. A file of another version is
/// not opened at all now.
///
/// The history: 1 — the first version under the counter (a debt without a
/// stored original amount, the exception for a zero «excluded» row, the
/// category on the counterparty).
/// 2 — a debt's remainder ignores movements of removed operations.
/// 3 — reconciliation with the bank and dismissed look-alikes: two tables.
/// 4 — the journal of changes.
/// 5 — a counterparty's name given by hand (`counterparty.renamed_at`).
/// 6 — settings (`setting`).
/// 7 — `operation.id` with `autoincrement`: rolling a statement back deleted
/// operations and their numbers went to new ones — a journal entry then
/// undid somebody else's edit.
pub const SCHEMA_VERSION: i64 = 9;

/// How to reach the current version from an earlier one: SQL that runs before
/// the schema is applied. The schema is idempotent and creates what is
/// missing by itself; migrations are for what `if not exists` does not
/// replace — views above all.
///
/// The first of them is the first schema change after a real ledger existed
/// (the migration from saldo, 2026-10-18), and so the first real one.
///
/// A migration with empty SQL is not a mistake: new tables are created by the
/// schema itself, and the entry is there so a file of the earlier version
/// counts as convertible rather than foreign.
const MIGRATIONS: &[(i64, &str)] = &[(1, "drop view if exists debt_remaining;"), (2, ""), (3, ""), (4, ""), (5, ""), (6, OPERATION_AUTOINCREMENT), (7, "alter table counterparty add column note text;"), (8, "alter table operation add column note text;")];

/// 6 → 7: the operation table rebuilt with `autoincrement`. SQLite cannot add
/// it in place. Runs with foreign keys off (see `init`) — dropping the old
/// table with them on would null every debt movement's operation and delete
/// every split. The views go first and come back from the schema, as do the
/// indexes and the currency trigger that went with the table.
const OPERATION_AUTOINCREMENT: &str = "
drop view if exists account_balance;
drop view if exists debt_remaining;
create table operation_rebuilt (
  id               integer primary key autoincrement,
  account_id       integer not null references account(id) on delete restrict,
  role             text    not null,
  state            text    not null default 'POSTED',
  occurred_at      text    not null,
  amount_minor     integer not null check (amount_minor > 0),
  currency         text    not null,
  effect_minor     integer not null,
  description      text,
  counterparty_id  integer references counterparty(id) on delete set null,
  category_id      integer references category(id)     on delete set null,
  pair_id          integer references operation(id)    on delete restrict,
  reverses_id      integer references operation(id)    on delete restrict,
  import_row_id    integer references import_row(id)   on delete restrict,
  created_at       text    not null default (datetime('now')),
  check (role  in ('income','expense','transfer','passthrough',
                   'debt_movement','opening_balance','correction','excluded')),
  check (state in ('POSTED','PENDING','REVERSED','CORRECTED')),
  check (abs(effect_minor) = amount_minor or (role = 'excluded' and effect_minor = 0))
);
insert into operation_rebuilt (id, account_id, role, state, occurred_at, amount_minor, currency, effect_minor,
                               description, counterparty_id, category_id, pair_id, reverses_id, import_row_id, created_at)
select id, account_id, role, state, occurred_at, amount_minor, currency, effect_minor,
       description, counterparty_id, category_id, pair_id, reverses_id, import_row_id, created_at
  from operation;
drop table operation;
alter table operation_rebuilt rename to operation;
";

/// `$XDG_DATA_HOME/potok`, or `~/.local/share/potok`.
pub fn data_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("potok"));
        }
    }
    let home = std::env::var("HOME").with_context(|| crate::tr!("neither XDG_DATA_HOME nor HOME is set", "не задан ни XDG_DATA_HOME, ни HOME"))?;
    Ok(PathBuf::from(home).join(".local/share/potok"))
}

/// Where the statement files themselves are kept — §4, «the archive».
///
/// It is there so the parser can be improved and everything parsed again
/// from scratch. Without the archive, improving how descriptions are cleaned
/// reopens the history to duplicates — a known hole in the old system.
pub fn archive_dir(db: &Path) -> PathBuf {
    db.parent().unwrap_or(Path::new(".")).join("archive")
}

pub fn default_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("ledger.db"))
}

/// Readable by its owner only: a folder 0700, a file 0600.
///
/// The ledger is every payment somebody made, and the default umask leaves it
/// readable by every account on the machine. The default folder covers what
/// is already inside it — the archive of statements, copies made by hand.
pub fn owner_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| crate::tr!("cannot make {} private", "не закрывается доступ к {}", path.display()))?;
    }
    Ok(())
}

/// A folder of potok's own files — backups, the archive of statements —
/// made if missing and readable by its owner only. A folder that holds
/// anything else is somebody's (their ~/Documents named as the backup
/// folder) and is left as it was; its files are closed one by one.
pub fn private_dir(dir: &Path, ours: impl Fn(&str) -> bool) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| crate::tr!("cannot create {}", "не создаётся {}", dir.display()))?;
    let only_ours = std::fs::read_dir(dir)?.flatten().all(|e| ours(&e.file_name().to_string_lossy()));
    if only_ours {
        owner_only(dir)?;
    }
    Ok(())
}

/// Write a file readable by its owner only from its first byte — not
/// written under the umask and closed afterwards.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| crate::tr!("cannot write {}", "не пишется {}", path.display()))?;
    file.write_all(bytes)?;
    owner_only(path)
}

/// Opens the database, creating it on the first run.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| crate::tr!("cannot create {}", "не создаётся {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| crate::tr!("cannot open {}", "не открывается {}", path.display()))?;
    wait_for_others(&conn)?;
    init(&conn)?;
    // The WAL and the shared memory are already there by now, made with the
    // file's mode as it was; they hold the same rows.
    for suffix in ["", "-wal", "-shm"] {
        let file = PathBuf::from(format!("{}{suffix}", path.display()));
        if file.exists() {
            owner_only(&file)?;
        }
    }
    // Its own folder only: a ledger opened from /tmp or ~/Documents leaves
    // the folder as it was.
    if let Some(parent) = path.parent().filter(|p| data_dir().ok().as_deref() == Some(*p)) {
        owner_only(parent)?;
    }
    Ok(conn)
}

/// A database nothing can be written to — for an agent (§6).
///
/// The refusal is held by SQLite itself (`pragma query_only`), not by our
/// code: any `insert`, `update` or `delete` on this connection fails whatever
/// function was called. That is stronger than «no write tools are
/// registered» — what is not registered today is registered in one line
/// tomorrow.
///
/// The `SQLITE_OPEN_READ_ONLY` flag would be stricter still, but on a
/// database in WAL mode it refuses to open while there is no `-shm` file
/// beside it, and there is none until the database has been opened for
/// writing. `query_only` works whatever state the file is in.
///
/// A database that is not there is not created: an agent connecting before
/// the first import gets a plain refusal rather than a silently created
/// empty ledger.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    if !path.exists() {
        anyhow::bail!(crate::tr!(
            "there is no ledger at {}. Import a statement first — potok import <file> --account <account>",
            "леджера нет: {}. Сначала загрузи выписку — potok import <файл> --account <счёт>",
            path.display()
        ));
    }
    let conn = Connection::open(path)
        .with_context(|| crate::tr!("cannot open {}", "не открывается {}", path.display()))?;
    wait_for_others(&conn)?;
    // Another version is dangerous even for reading: an old view counts debts
    // differently, and the agent gets wrong figures with a confident face.
    check_version(&conn, false)?;
    conn.execute_batch("pragma query_only = 1;")?;
    Ok(conn)
}

/// An in-memory database — for the tests and for `--dry-run`.
pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    init(&conn)?;
    Ok(conn)
}

/// Another connection may be writing — a second window, the rates fetched in
/// the background, a command in another terminal: wait for it rather than
/// refuse. rusqlite sets the same five seconds itself; said here so the wait
/// does not hang on a library default. What made 96 of 133 `potok add` in a
/// row fail «database is locked» (audit, 2026-10-20) was not this but every
/// open taking the write lock to re-apply the schema — see `up_to_date`.
fn wait_for_others(conn: &Connection) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

/// Run a multi-statement write atomically on `&Connection` using a SQLite
/// savepoint. Works both standalone (starts and commits a transaction) and
/// nested inside an outer `conn.transaction()`.
pub(crate) fn atomic<T>(conn: &Connection, f: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("savepoint potok_atomic;")?;
    match f() {
        Ok(value) => {
            conn.execute_batch("release savepoint potok_atomic;")?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute_batch("rollback to savepoint potok_atomic; release savepoint potok_atomic;");
            Err(error)
        }
    }
}

/// Names the schema declares: what `if not exists` would create.
fn declared_objects() -> Vec<&'static str> {
    SCHEMA
        .split("if not exists")
        .skip(1)
        .filter_map(|rest| rest.split_whitespace().next())
        .map(|name| name.trim_end_matches('('))
        .collect()
}

/// This version, every table, index, view and trigger, every column added
/// later: nothing to apply. Opening such a file must not write — a write
/// takes the lock, and every open took it, so two openers refused each other.
/// Checked in a single query rather than ~30 round-trips to `sqlite_master`.
fn up_to_date(conn: &Connection) -> Result<bool> {
    let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        return Ok(false);
    }
    let objects = declared_objects();
    let placeholders = std::iter::repeat_n("?", objects.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "select (select count(distinct name) from sqlite_master where name in ({placeholders})) = {}
            and exists (select 1 from pragma_table_info('counterparty') where name = 'renamed_at')
            and exists (select 1 from pragma_table_info('counterparty') where name = 'note')
            and exists (select 1 from pragma_table_info('operation') where name = 'note')
            and exists (select 1 from pragma_table_info('import_file') where name = 'account_id')
            and exists (select 1 from pragma_table_info('import_row') where name = 'pending')",
        objects.len()
    );
    Ok(conn.query_row(&sql, rusqlite::params_from_iter(objects), |r| r.get(0))?)
}

fn init(conn: &Connection) -> Result<()> {
    let version = check_version(conn, true)?;
    // Before the transaction, and that is not a matter of taste: inside one
    // SQLite silently ignores both pragmas (checked). For `journal_mode` that
    // is a real regression — without this line a new ledger would not be in
    // WAL, and the `foreign_keys_are_enforced_on_every_connection` test
    // catches it. For `foreign_keys` our build has no consequence either way:
    // `rusqlite` with `bundled` turns foreign keys on by default, and no test
    // could catch that. The line is here so key checking does not depend on
    // which SQLite this was built against.
    conn.execute_batch("pragma foreign_keys = on;")?;
    conn.query_row("pragma journal_mode = wal", [], |_| Ok(()))?;
    if up_to_date(conn)? {
        return Ok(());
    }
    // A migration that rebuilds a table needs foreign keys off — the pragma
    // has no effect inside a transaction, so here — and checked afterwards.
    let rebuilds = version != 0 && version < SCHEMA_VERSION;
    if rebuilds {
        conn.execute_batch("pragma foreign_keys = off;")?;
    }
    // The migrations, the schema and the version number in one transaction: a
    // migration cut in half must not leave the file without the debt view.
    if let Err(error) = conn.execute_batch("begin immediate;") {
        if rebuilds {
            let _ = conn.execute_batch("pragma foreign_keys = on;");
        }
        return Err(error.into());
    }
    let applied = (|| -> Result<()> {
        for (from, sql) in MIGRATIONS {
            if *from >= version && version != 0 {
                // A column that is already there is a migration already done,
                // not a failure: the schema beside it declares the column, and
                // a file built by this version's schema has it from the
                // start.
                match conn.execute_batch(sql) {
                    Err(error) if error.to_string().contains("duplicate column name") => {}
                    other => other.with_context(|| crate::tr!("migration from version {from} fails", "не проходит миграция с версии {from}"))?,
                }
            }
        }
        // The schema is idempotent: every object is declared with `if not
        // exists`, so opening a file of its own version again breaks nothing.
        conn.execute_batch(SCHEMA).context("schema.sql does not apply")?;
        // A column on an existing table: `create table if not exists` will not
        // add it, and `alter table` fails the second time. Hence by fact.
        ensure_column(conn, "counterparty", "renamed_at", "text")?;
        ensure_column(conn, "import_file", "account_id", "integer references account(id) on delete set null")?;
        ensure_column(conn, "import_row", "pending", "integer not null default 0")?;
        conn.execute_batch(&format!("pragma user_version = {SCHEMA_VERSION};"))?;
        Ok(())
    })();
    let applied = applied.and_then(|()| {
        if rebuilds {
            let broken: i64 = conn.query_row("select count(*) from pragma_foreign_key_check", [], |r| r.get(0))?;
            anyhow::ensure!(broken == 0, crate::tr!("the migration broke {broken} links — nothing was changed", "миграция порвала связей: {broken} — ничего не изменено"));
        }
        Ok(())
    });
    let done = match applied {
        Ok(()) => conn.execute_batch("commit;").map_err(anyhow::Error::from),
        Err(error) => {
            let _ = conn.execute_batch("rollback;");
            Err(error)
        }
    };
    if rebuilds {
        conn.execute_batch("pragma foreign_keys = on;")?;
    }
    done
}

fn ensure_column(conn: &Connection, table: &str, column: &str, kind: &str) -> Result<()> {
    let present: bool = conn.query_row(
        &format!("select exists (select 1 from pragma_table_info('{table}') where name = ?1)"),
        [column],
        |r| r.get(0),
    )?;
    if !present {
        conn.execute_batch(&format!("alter table {table} add column {column} {kind};"))?;
    }
    Ok(())
}

/// Its own file is opened, an empty one is created, one of another version is
/// refused with the reason.
///
/// Returns the file's version when it can be worked with: its own, an empty
/// one, or one there is a migration from (`can_migrate`). Read-only cannot
/// migrate — writing is forbidden — so an earlier version is refused there,
/// with a hint.
pub(crate) fn check_version(conn: &Connection, can_migrate: bool) -> Result<i64> {
    let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0))?;
    let tables: i64 = conn.query_row(
        "select count(*) from sqlite_master where type = 'table' and name not like 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    if version == SCHEMA_VERSION || (version == 0 && tables == 0) {
        return Ok(version);
    }
    let migratable = version > 0 && MIGRATIONS.iter().any(|(from, _)| *from == version);
    if migratable && can_migrate {
        return Ok(version);
    }
    if migratable {
        bail!(crate::tr!(
            "the ledger is version {version}, {SCHEMA_VERSION} is needed. Open it once with potok itself \\
             (just `potok`) — it will upgrade — and then the agent can too",
            "леджер версии {version}, а нужна {SCHEMA_VERSION}. Открой его один раз самим potok \\
             (просто `potok`) — он обновится, — и тогда агенту тоже можно"
        ))
    }
    let empty: bool = tables > 0
        && conn
            .query_row(
                "select (select count(*) from account) + (select count(*) from operation) = 0",
                [],
                |r| r.get(0),
            )
            .unwrap_or(false);
    let made_by = if version == 0 {
        crate::tr!("an older potok, before the schema had a version", "прежним potok, до того как у схемы появилась версия")
    } else {
        crate::tr!(
            "potok with schema version {version}, and this one understands version {SCHEMA_VERSION}",
            "potok со схемой версии {version}, а этот понимает версию {SCHEMA_VERSION}"
        )
    };
    if empty {
        bail!(crate::tr!(
            "this ledger was created by {made_by}. It is empty — there is no data in it — so it can \
             simply be deleted, and potok will create a new one",
            "этот леджер создан {made_by}. Он пуст — данных в нём нет, — так что его можно \
             просто удалить, и potok создаст новый"
        ))
    }
    bail!(crate::tr!(
        "this ledger was created by {made_by}, and it holds data. It must not be opened by newer code: \
         the old schema would count some figures differently. Migrate the data again into a new database \
         (--db <path>) or go back to the potok version that created it",
        "этот леджер создан {made_by}, и в нём есть данные. Открывать его новым кодом нельзя: \
         старая схема посчитала бы часть цифр иначе. Перенеси данные заново в новую базу \
         (--db <путь>) или верни версию potok, которая его создала"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema in the specification and the schema in the binary are one
    /// text.
    ///
    /// In the previous project the costliest finding of the audit was that
    /// the documents had spent five weeks asserting things about the database
    /// that had stopped being true. This test keeps §4 and `schema.sql` from
    /// drifting apart.
    #[test]
    fn the_spec_and_the_schema_are_the_same_sql() {
        let spec = include_str!("../../SPEC.md");
        let block = spec
            .split("```sql\n")
            .nth(1)
            .and_then(|rest| rest.split("\n```").next())
            .expect("no ```sql block in SPEC.md");
        assert_eq!(
            block.trim_end(),
            SCHEMA.trim_end(),
            "§4 of the specification drifted from core/src/schema.sql"
        );
    }

    /// The agent's connection does not write — and the database holds that,
    /// not our code.
    #[test]
    fn a_read_only_ledger_refuses_every_kind_of_write() {
        let path = std::env::temp_dir().join(format!("potok-ro-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = open(&path).unwrap();
            conn.execute_batch(
                "insert into account (name,currency,kind) values ('A','EUR','BANK');",
            )
            .unwrap();
        }
        let conn = open_read_only(&path).unwrap();
        let count: i64 =
            conn.query_row("select count(*) from account", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "reading is allowed");
        for write in [
            "insert into account (name,currency,kind) values ('B','EUR','BANK')",
            "update account set name = 'C'",
            "delete from account",
            "drop table account",
        ] {
            assert!(conn.execute_batch(write).is_err(), "went through: {write}");
        }
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    /// An agent connecting before the first import must not silently get an
    /// empty database — that is a write to disk nobody asked for.
    #[test]
    fn a_ledger_that_does_not_exist_is_not_created_for_reading() {
        let path = std::env::temp_dir().join(format!("potok-absent-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let error = open_read_only(&path).unwrap_err().to_string();
        assert!(error.contains("potok import"), "{error}");
        assert!(!path.exists(), "файл появился");
    }

    /// A ledger made under the default umask was readable by every account on
    /// the machine; opening it takes that away. A folder that is not potok's
    /// own is left as it was.
    #[cfg(unix)]
    #[test]
    fn a_ledger_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("potok-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.join("ledger.db");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let conn = open(&path).unwrap();
        conn.execute("insert into account (name,currency,kind) values ('Cash','EUR','CASH')", []).unwrap();
        let mode = |p: &Path| p.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o755, "not potok's folder");
        let wal = dir.join("ledger.db-wal");
        assert_eq!(mode(&wal), 0o600, "the WAL holds the same rows");
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Foreign keys and WAL — on every connection, on a new file and on one
    /// reopened. Inside a transaction both pragmas are no-ops. WAL this test
    /// catches (checked by removing the line); foreign keys are on by default in
    /// our build, so that half is insurance against another SQLite build.
    #[test]
    fn foreign_keys_are_enforced_on_every_connection() {
        let path = std::env::temp_dir().join(format!("potok-fk-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        for _ in 0..2 {
            let conn = open(&path).unwrap();
            let on: i64 = conn.query_row("pragma foreign_keys", [], |r| r.get(0)).unwrap();
            assert_eq!(on, 1);
            let orphan = conn.execute_batch(
                "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor)
                     values (999,'expense','2026-09-01T00:00:00Z',1,'EUR',-1);",
            );
            assert!(orphan.is_err(), "операция на несуществующий счёт прошла");
            let mode: String = conn.query_row("pragma journal_mode", [], |r| r.get(0)).unwrap();
            assert_eq!(mode, "wal");
        }
        assert!(open_memory().unwrap().query_row("pragma foreign_keys", [], |r| r.get::<_, i64>(0)).unwrap() == 1);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn a_new_ledger_carries_the_schema_version() {
        let conn = open_memory().unwrap();
        let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// Exactly the case found on 2026-10-18: an empty ledger on the old schema.
    /// Opening it would mean counting debts by the old formula for good.
    #[test]
    fn a_ledger_from_before_versioning_is_refused_and_says_whether_it_is_empty() {
        let path = std::env::temp_dir().join(format!("potok-old-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap(); // схема без версии, как у прежнего potok
        }
        let error = open(&path).unwrap_err().to_string();
        assert!(error.contains("empty"), "{error}");
        assert!(open_read_only(&path).is_err(), "и для чтения тоже");

        Connection::open(&path)
            .unwrap()
            .execute_batch("insert into account (name,currency,kind) values ('A','EUR','BANK');")
            .unwrap();
        let error = open(&path).unwrap_err().to_string();
        assert!(error.contains("holds data"), "{error}");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    /// A version 1 ledger — what you have after the move — upgrades itself on
    /// the first open, the data stays, the debt view is the new one.
    #[test]
    fn a_version_one_ledger_migrates_on_open() {
        let path = std::env::temp_dir().join(format!("potok-v1-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        {
            let conn = Connection::open(&path).unwrap();
            // schema version 1: the same file, but the old debt view
            let v1 = SCHEMA.replace(
                "left join debt_movement m on m.debt_id = d.id\n  and not exists (select 1 from operation o\n                   where o.id = m.operation_id and o.state <> 'POSTED')",
                "left join debt_movement m on m.debt_id = d.id",
            );
            assert_ne!(v1, SCHEMA, "подмена вида не сработала — тест проверял бы ничего");
            conn.execute_batch(&v1).unwrap();
            conn.execute_batch(
                "pragma user_version = 1;
                 insert into account (name,currency,kind) values ('A','EUR','BANK');
                 insert into counterparty (name) values ('Кто-то');
                 insert into operation (id,account_id,role,state,occurred_at,amount_minor,currency,effect_minor)
                     values (1,1,'debt_movement','REVERSED','2026-09-03T00:00:00Z',72000,'EUR',-72000);
                 insert into debt (counterparty_id,direction,currency,opened_on) values (1,'I_OWE','EUR','2026-01-01');
                 insert into debt_movement (debt_id,operation_id,kind,amount_minor,occurred_on)
                     values (1,null,'ISSUE',100000,'2026-01-01'), (1,1,'REPAYMENT',72000,'2026-09-03');",
            )
            .unwrap();
        }
        assert!(open_read_only(&path).unwrap_err().to_string().contains("Open it once"));
        let conn = open(&path).unwrap();
        let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let remaining: i64 =
            conn.query_row("select remaining_minor from debt_remaining", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 100000, "возврат от убранной операции больше не считается");
        let accounts: i64 = conn.query_row("select count(*) from account", [], |r| r.get(0)).unwrap();
        assert_eq!(accounts, 1, "данные на месте");
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn a_ledger_of_another_version_is_refused() {
        let path = std::env::temp_dir().join(format!("potok-future-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        open(&path).unwrap().execute_batch("pragma user_version = 99;").unwrap();
        assert!(open(&path).unwrap_err().to_string().contains("version 99"));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn applying_the_schema_twice_changes_nothing() {
        let conn = open_memory().unwrap();
        init(&conn).expect("схема идемпотентна");
    }

    /// Invariant 6: a pending operation is not money. What holds it is not the
    /// code but a filter inside the view, and that is what has to be checked.
    #[test]
    fn a_pending_operation_stays_out_of_the_balance() {
        let conn = open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor)
                 values ('Revolut','EUR','BANK',40212);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,'expense','2026-09-01T00:00:00Z',624,'EUR',-624);
             insert into operation (account_id,role,state,occurred_at,amount_minor,currency,effect_minor)
                 values (1,'expense','PENDING','2026-10-16T00:00:00Z',660,'EUR',-660);",
        )
        .unwrap();
        let balance: i64 = conn
            .query_row("select balance_minor from account_balance", [], |r| r.get(0))
            .unwrap();
        assert_eq!(balance, 39588, "402.12 - 6.24, ожидающие 6.60 не в счёт");
    }

    #[test]
    fn an_operation_in_the_wrong_currency_is_refused_by_the_database() {
        let conn = open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');",
        )
        .unwrap();
        let refused = conn.execute_batch(
            "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,'expense','2026-09-02T00:00:00Z',100,'USD',-100);",
        );
        assert!(refused.is_err(), "триггер валюты не сработал");
    }

    /// Invariant 5 with its one exception: an «excluded» row may move no
    /// balance — that is how saldo stores statement rows a person told it not to
    /// count. Any other role with no effect is a mistake.
    #[test]
    fn only_an_excluded_row_may_hold_no_money() {
        let conn = open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind) values ('A','EUR','BANK');")
            .unwrap();
        let excluded = conn.execute_batch(
            "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,'excluded','2026-09-01T00:00:00Z',80000,'EUR',0);",
        );
        assert!(excluded.is_ok(), "{excluded:?}");
        let expense = conn.execute_batch(
            "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor)
                 values (1,'expense','2026-09-01T00:00:00Z',80000,'EUR',0);",
        );
        assert!(expense.is_err(), "расход без движения денег должен отвергаться");
    }

    /// The first revision of the schema added the original amount to the
    /// movements, and the original amount is the first ISSUE movement — a debt
    /// came out twice as large. The formula is saldo's: ΣISSUE − Σthe rest.
    #[test]
    fn a_debt_is_counted_once_not_twice() {
        let conn = open_memory().unwrap();
        conn.execute_batch(
            "insert into counterparty (name) values ('Кто-то');
             insert into debt (counterparty_id,direction,currency,opened_on)
                 values (1,'I_OWE','EUR','2026-01-01');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'ISSUE',220000,'2026-01-01'),
                        (1,'REPAYMENT',40000,'2026-02-01');",
        )
        .unwrap();
        let (issued, remaining): (i64, i64) = conn
            .query_row("select issued_minor, remaining_minor from debt_remaining", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(issued, 220000);
        assert_eq!(remaining, 180000, "2200 − 400 = 1800, как в saldo, а не 4000 − 400");
    }

    /// Invariant 2: a debt's remainder is computed, not stored — so it cannot
    /// disagree with its movements.
    #[test]
    fn a_debt_remainder_is_the_sum_of_its_movements() {
        let conn = open_memory().unwrap();
        conn.execute_batch(
            "insert into counterparty (name) values ('Кто-то');
             insert into debt (counterparty_id,direction,currency,opened_on)
                 values (1,'OWED_TO_ME','EUR','2026-09-03');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'ISSUE',80000,'2026-09-03');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'REPAYMENT',30000,'2026-09-20');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'ISSUE',5000,'2026-10-01');",
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row("select remaining_minor from debt_remaining", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 55000, "800 - 300 + 50");
    }

    /// Opening a ledger of this version writes nothing, and a write waits for
    /// another writer instead of failing. Before, every open took the write
    /// lock to re-apply the schema, and `potok add` failed «database is
    /// locked» 96 times in 133 while the background send opened the same file.
    #[test]
    fn opening_does_not_take_the_lock_and_a_write_waits_its_turn() {
        let path = std::env::temp_dir().join(format!("potok-busy-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        drop(open(&path).unwrap());
        assert!(declared_objects().contains(&"debt_remaining"), "{:?}", declared_objects());

        let writer = open(&path).unwrap();
        writer.execute_batch("begin immediate; insert into account (name,currency,kind) values ('A','EUR','BANK');").unwrap();
        // Opened while the other holds the lock: up to date, so no write.
        let second = open(&path).expect("opening must not need the lock");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            writer.execute_batch("commit;").unwrap();
        });
        second
            .execute("insert into account (name,currency,kind) values ('B','EUR','BANK')", [])
            .expect("the write waits for the other one, not refuses");
        release.join().unwrap();
        let count: i64 = second.query_row("select count(*) from account", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2);
        drop(second);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    /// A version-6 ledger moves to `autoincrement` with every link intact,
    /// and a number freed by a deletion is not handed out again. Before, a
    /// rolled-back statement's numbers went to the next operations, and an
    /// old journal entry undid a change on the wrong one.
    #[test]
    fn version_six_moves_to_autoincrement_and_numbers_are_not_reused() {
        let path = std::env::temp_dir().join(format!("potok-v6-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(&SCHEMA.replace("integer primary key autoincrement", "integer primary key")).unwrap();
            old.execute_batch(
                "insert into account (name,currency,kind,opening_minor) values ('A','EUR','BANK',10000), ('B','EUR','BANK',0);
                 insert into counterparty (name) values ('Friend');
                 insert into category (name) values ('Food'), ('Home');
                 insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor) values
                   (1,1,'transfer','2026-09-01T00:00:00Z',500,'EUR',-500),
                   (2,2,'transfer','2026-09-01T00:00:00Z',500,'EUR',500),
                   (3,1,'debt_movement','2026-09-02T00:00:00Z',300,'EUR',300),
                   (4,1,'expense','2026-09-03T00:00:00Z',1000,'EUR',-1000),
                   (5,1,'expense','2026-09-04T00:00:00Z',200,'EUR',-200);
                 update operation set pair_id = 2 where id = 1;
                 update operation set pair_id = 1 where id = 2;
                 insert into debt (counterparty_id,direction,currency,opened_on) values (1,'I_OWE','EUR','2026-09-02');
                 insert into debt_movement (debt_id,operation_id,kind,amount_minor,occurred_on) values (1,3,'ISSUE',300,'2026-09-02');
                 insert into operation_split (operation_id,category_id,amount_minor) values (4,1,600), (4,2,400);
                 delete from operation where id = 5;
                 pragma user_version = 6;",
            )
            .unwrap();
        }
        let conn = open(&path).unwrap();
        let version: i64 = conn.query_row("pragma user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let sql: String = conn.query_row("select sql from sqlite_master where name = 'operation'", [], |r| r.get(0)).unwrap();
        assert!(sql.contains("autoincrement"), "{sql}");
        let links: (i64, i64, i64, i64) = conn
            .query_row(
                "select (select count(*) from operation),
                        (select operation_id from debt_movement),
                        (select count(*) from operation_split where operation_id = 4),
                        (select pair_id from operation where id = 1)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(links, (4, 3, 2, 2), "operations, the debt link, the split and the pair survive");
        let balance: i64 = conn.query_row("select balance_minor from account_balance where account_id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(balance, 10000 - 500 + 300 - 1000, "the views are back");
        assert!(conn.execute_batch("insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor) values (1,'expense','2026-09-05T00:00:00Z',1,'USD',-1)").is_err(), "the currency trigger is back");

        conn.execute_batch("delete from operation where id = 4;").unwrap();
        conn.execute("insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor) values (1,'expense','2026-09-05T00:00:00Z',1,'EUR',-1)", []).unwrap();
        assert!(conn.last_insert_rowid() > 4, "a freed number is not handed out again: got {}", conn.last_insert_rowid());
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn atomic_rolls_back_all_statements_when_a_later_one_fails() {
        let conn = open_memory().unwrap();
        let res: Result<()> = atomic(&conn, || {
            conn.execute(
                "insert into account (name, currency, kind) values ('A', 'EUR', 'BANK')",
                [],
            )?;
            anyhow::bail!("boom");
        });
        assert!(res.is_err());
        let count: i64 = conn
            .query_row("select count(*) from account", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "failed atomic block must leave no partial row behind");
    }
}
