//! Backups: whole copies of the ledger, made by SQLite itself.
//!
//! The ledger is one file on one laptop. A backup is `VACUUM INTO` — a
//! consistent copy even while the ledger is open in another window, with no
//! WAL to carry along. One is made on its own when the last is older than a
//! day; the folder can be pointed at a synced one (iCloud Drive, Dropbox) to
//! get the copy off the machine.
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// How many copies to keep: a month of daily ones.
pub const KEEP: usize = 30;
const PREFIX: &str = "ledger-";

/// The default folder: `backups` next to the ledger.
pub fn default_dir(db: &Path) -> PathBuf {
    db.parent().unwrap_or(Path::new(".")).join("backups")
}

/// The folder in use: the setting, or the default.
///
/// The setting is the default ledger's only. A copy opened with `--db`
/// carries the setting along and used to put its own daily copy into the real
/// ledger's folder — where `restore` and the daily send to Drive took the
/// newest file, whichever ledger it came from. Its copies stay beside it.
pub fn dir(conn: &Connection, db: &Path) -> Result<PathBuf> {
    let default_ledger = crate::db::default_path().ok().as_deref() == Some(db);
    Ok(match crate::settings::get(conn, crate::settings::BACKUP_DIR)? {
        Some(dir) if default_ledger => expand(&dir),
        _ => default_dir(db),
    })
}

/// A consistent copy of the ledger at `path`, readable by its owner only —
/// outside the rotation of [`make`].
pub fn snapshot(conn: &Connection, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(path);
    conn.execute("vacuum into ?1", [path.to_string_lossy()])
        .with_context(|| crate::tr!("the backup to {} failed", "бэкап в {} не сделан", path.display()))?;
    crate::db::owner_only(path)
}

/// `~/…` as people type it.
pub fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// A file potok keeps in a backup folder: copies, copies from Drive, a send
/// being staged, and the system's own litter.
pub fn is_ours(name: &str) -> bool {
    name.starts_with(PREFIX) || name.starts_with(".ledger-") || name.starts_with(".sending") || name.starts_with(".pulled") || name == ".DS_Store"
}

/// Copies there are, newest first.
pub fn list(dir: &Path) -> Vec<(SystemTime, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.starts_with(PREFIX) && name.ends_with(".db")
        })
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    found.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    found
}

/// Put the ledger in `source` in place of the open one at `db`, keeping the
/// settings that belong to this machine, and say so in the journal — so a
/// restored ledger counts as changed and goes to Drive as the next copy
/// instead of being taken for an old one. Returns the connection reopened,
/// with the schema brought up to date.
///
/// Through SQLite's own backup into the open ledger, not by writing over the
/// file: another connection on it — a second window — then sees the change
/// or waits, where a file written under it is a corrupt ledger.
pub fn put_in_place(mut conn: Connection, db: &Path, source: &Path, what: &str) -> Result<Connection> {
    let from = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    from.query_row("select count(*) from operation", [], |r| r.get::<_, i64>(0))
        .with_context(|| crate::tr!("{} is not a potok ledger", "{} — не леджер potok", source.display()))?;
    crate::db::check_version(&from, true)?;
    let local: Vec<(&str, Option<String>)> =
        crate::settings::LOCAL_ONLY.iter().map(|k| (*k, crate::settings::get(&conn, k).ok().flatten())).collect();
    conn.busy_timeout(Duration::from_secs(10))?;
    rusqlite::backup::Backup::new(&from, &mut conn)?
        .run_to_completion(256, Duration::ZERO, None)
        .with_context(|| crate::tr!("cannot copy {} over the ledger", "не копируется {} на место леджера", source.display()))?;
    drop(conn);
    drop(from);
    // Opened again so a copy from an older potok is brought up to this
    // schema, as any ledger is on open.
    let conn = crate::db::open(db)?;
    for (key, value) in local {
        match value {
            Some(value) => crate::settings::set(&conn, key, &value)?,
            None => crate::settings::clear(&conn, key)?,
        }
    }
    crate::journal::log(&conn, what, None)?;
    // The journal came with the copy and may be shorter than this machine's
    // mark of what it sent: without the mark the ledger is changed, and goes
    // to Drive next. A copy taken from Drive sets the mark again right after.
    crate::settings::clear(&conn, crate::settings::CLOUD_LOG_MARK)?;
    Ok(conn)
}

/// Make a copy now; keep the newest [`KEEP`].
pub fn make(conn: &Connection, dir: &Path) -> Result<PathBuf> {
    crate::db::private_dir(dir, is_ours)
        .with_context(|| crate::tr!("cannot create the backup folder {}", "не создаётся папка бэкапов {}", dir.display()))?;
    let name = format!("{PREFIX}{}.db", chrono::Local::now().format("%Y-%m-%d-%H%M%S-%3f"));
    let path = dir.join(name);
    snapshot(conn, &path)?;
    // The folder only if it is potok's alone (above): it may be somebody's
    // ~/Documents. The copies always — older ones too, made before this was.
    for (at, copy) in list(dir).into_iter().enumerate().map(|(i, (_, p))| (i, p)) {
        if at < KEEP {
            crate::db::owner_only(&copy)?;
        } else {
            let _ = std::fs::remove_file(copy);
        }
    }
    Ok(path)
}

/// Make a copy if the newest is older than a day, or there is none.
pub fn make_if_due(conn: &Connection, dir: &Path) -> Result<Option<PathBuf>> {
    let fresh = list(dir)
        .first()
        .and_then(|(at, _)| at.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(24 * 3600));
    if fresh {
        return Ok(None);
    }
    make(conn, dir).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_a_whole_ledger_and_old_ones_go() {
        let dir = std::env::temp_dir().join(format!("potok-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let conn = crate::db::open_memory().unwrap();
        conn.execute("insert into account (name,currency,kind) values ('Cash','EUR','CASH')", []).unwrap();

        let made = make_if_due(&conn, &dir).unwrap().expect("no copy yet — one is due");
        assert!(make_if_due(&conn, &dir).unwrap().is_none(), "a fresh one is there");
        let copy = Connection::open(&made).unwrap();
        let accounts: i64 = copy.query_row("select count(*) from account", [], |r| r.get(0)).unwrap();
        assert_eq!(accounts, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| p.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&made), 0o600, "a copy of the ledger is the owner's only");
            assert_eq!(mode(&dir), 0o700, "and so is a folder of copies only");
            let shared = std::env::temp_dir().join(format!("potok-backup-shared-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&shared);
            std::fs::create_dir_all(&shared).unwrap();
            std::fs::write(shared.join("letter.txt"), b"mine").unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
            make(&conn, &shared).unwrap();
            assert_eq!(mode(&shared), 0o755, "a folder with somebody's files in it is left as it was");
            let _ = std::fs::remove_dir_all(&shared);
        }

        for i in 0..KEEP + 3 {
            std::fs::write(dir.join(format!("{PREFIX}0000-00-{i:02}.db")), b"").unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        make(&conn, &dir).unwrap();
        assert_eq!(list(&dir).len(), KEEP);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A ledger that is not the default one keeps its copies beside it, even
    /// with the setting copied from the real one.
    #[test]
    fn another_ledger_does_not_use_the_shared_folder() {
        let conn = crate::db::open_memory().unwrap();
        crate::settings::set(&conn, crate::settings::BACKUP_DIR, "/somewhere/shared").unwrap();
        let elsewhere = std::env::temp_dir().join("potok-trial").join("ledger.db");
        assert_eq!(dir(&conn, &elsewhere).unwrap(), default_dir(&elsewhere));
        if let Ok(default) = crate::db::default_path() {
            assert_eq!(dir(&conn, &default).unwrap(), PathBuf::from("/somewhere/shared"));
        }
    }

    /// A copy put in place keeps this machine's settings — the cloud
    /// generation among them — and is a change in the journal.
    #[test]
    fn a_copy_put_in_place_keeps_this_machine_and_is_journaled() {
        let dir = std::env::temp_dir().join(format!("potok-put-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("ledger.db");
        let conn = crate::db::open(&db).unwrap();
        conn.execute("insert into account (name,currency,kind) values ('Cash','EUR','CASH')", []).unwrap();
        crate::settings::set(&conn, crate::settings::CLOUD_GENERATION, "7").unwrap();
        crate::settings::set(&conn, crate::settings::LANGUAGE, "en").unwrap();
        crate::settings::set(&conn, crate::settings::THEME, "saldo").unwrap();
        let old = dir.join("old.db");
        snapshot(&conn, &old).unwrap();
        conn.execute("insert into account (name,currency,kind) values ('Later','EUR','CASH')", []).unwrap();
        crate::settings::set(&conn, crate::settings::CLOUD_GENERATION, "9").unwrap();
        crate::settings::set(&conn, crate::settings::LANGUAGE, "ru").unwrap();
        crate::settings::set(&conn, crate::settings::THEME, "light").unwrap();
        crate::settings::set(&conn, crate::settings::CLOUD_LOG_MARK, "99").unwrap();

        let conn = put_in_place(conn, &db, &old, "restored").unwrap();
        let accounts: i64 = conn.query_row("select count(*) from account", [], |r| r.get(0)).unwrap();
        assert_eq!(accounts, 1, "the copy is in place");
        assert_eq!(crate::settings::get(&conn, crate::settings::CLOUD_GENERATION).unwrap().as_deref(), Some("9"), "this machine's generation stays");
        assert_eq!(crate::settings::get(&conn, crate::settings::LANGUAGE).unwrap().as_deref(), Some("en"), "the ledger's own settings come with it");
        assert_eq!(crate::settings::get(&conn, crate::settings::THEME).unwrap().as_deref(), Some("light"), "this Mac's theme stays");
        let last: String = conn.query_row("select what from change_log order by id desc limit 1", [], |r| r.get(0)).unwrap();
        assert_eq!(last, "restored", "a restore is in the journal");
        assert_eq!(crate::settings::get(&conn, crate::settings::CLOUD_LOG_MARK).unwrap(), None, "and counts as not yet sent");
        crate::i18n::set_lang(crate::i18n::Lang::En);

        // A copy from a newer potok version is refused before the open ledger is overwritten.
        let future = dir.join("future.db");
        snapshot(&conn, &future).unwrap();
        let f_conn = Connection::open(&future).unwrap();
        f_conn.execute_batch("pragma user_version = 99; delete from account;").unwrap();
        drop(f_conn);
        assert!(put_in_place(conn, &db, &future, "bad").is_err(), "future version must be refused");
        let preserved = crate::db::open(&db).unwrap();
        let accounts_left: i64 = preserved.query_row("select count(*) from account", [], |r| r.get(0)).unwrap();
        assert_eq!(accounts_left, 1, "original ledger was not overwritten before version check");
        assert_eq!(crate::settings::get(&preserved, crate::settings::CLOUD_GENERATION).unwrap().as_deref(), Some("9"));
        drop(preserved);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
