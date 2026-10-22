//! Settings kept in the ledger itself: a copy of the ledger opens the way it
//! was left.
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::i18n::Lang;

pub const LANGUAGE: &str = "language";
/// Where `i` on the import screen looks for statements.
pub const STATEMENTS_DIR: &str = "statements_dir";
/// Account a new hand-entered operation starts on; empty — the one under the
/// cursor, else cash.
pub const DEFAULT_ACCOUNT: &str = "default_account";
/// Period the flows screen opens with.
pub const FLOWS_PERIOD: &str = "flows_period";
/// Where backups go; empty — `backups` next to the ledger.
pub const BACKUP_DIR: &str = "backup_dir";
/// Argon2id hash of the screen-lock passphrase; absent — no lock.
pub const LOCK_HASH: &str = "lock_hash";
/// Lock the screen after this many idle minutes; absent — never.
pub const LOCK_AFTER: &str = "lock_after";

/// How accounting months start: "day" (on [`MONTH_DAY`]) or "last_workday".
pub const MONTH_MODE: &str = "month_mode";
/// Day of the month an accounting month starts on (1–28): a salary day.
pub const MONTH_DAY: &str = "month_day";

pub fn month_day(conn: &Connection) -> Result<u32> {
    Ok(get(conn, MONTH_DAY)?.and_then(|v| v.parse().ok()).filter(|d| (1..=28).contains(d)).unwrap_or(1))
}

/// Fetch rates once a day on start: "on" | absent.
pub const RATES_AUTO: &str = "rates_auto";
/// The day rates were last fetched.
pub const RATES_FETCHED: &str = "rates_fetched";

/// The currency totals and budgets are in.
pub const BASE_CURRENCY: &str = "base_currency";

pub fn base_currency(conn: &Connection) -> Result<crate::Currency> {
    let code = get(conn, BASE_CURRENCY)?.unwrap_or_else(|| "EUR".into());
    Ok(crate::Currency::new(&code)?)
}

/// When a copy last reached Google Drive, «YYYY-MM-DD HH:MM».
pub const CLOUD_SENT: &str = "cloud_sent";
/// Why the last attempt to send one failed; absent after a success.
pub const CLOUD_ERROR: &str = "cloud_error";
/// With gcloud's sign-in: the Google Cloud project Drive counts calls
/// against. Kept here, not in gcloud's own sign-in, so other tools using that
/// sign-in are not moved onto potok's project.
pub const CLOUD_PROJECT: &str = "cloud_project";
/// The generation of the Drive copy this ledger was last sent as or taken
/// from; a newer one in Drive means another device wrote since.
pub const CLOUD_GENERATION: &str = "cloud_generation";
/// The last journal entry at that moment: an entry after it is a change not
/// yet in Drive.
pub const CLOUD_LOG_MARK: &str = "cloud_log_mark";
/// This device's name in the writer mark; absent — the Mac's own name.
pub const DEVICE_NAME: &str = "device_name";

/// Settings that belong to the machine, not to the ledger: a copy taken from
/// another device keeps this one's.
///
/// The generation and the journal mark say what this machine last sent or
/// took: a copy put in place — from Drive or from a backup — must not bring
/// another moment's, or the next start takes the restored ledger for an old
/// one and replaces it with Drive's.
pub const LOCAL_ONLY: [&str; 11] = [
    BACKUP_DIR,
    CLOUD_GENERATION,
    CLOUD_LOG_MARK,
    STATEMENTS_DIR,
    CLOUD_PROJECT,
    CLOUD_SENT,
    CLOUD_ERROR,
    DEVICE_NAME,
    LOCK_HASH,
    LOCK_AFTER,
    THEME,
];

/// Colour theme of the interface: saldo | terminal | light.
pub const THEME: &str = "theme";
/// How many days apart two legs of a transfer may be.
pub const TRANSFER_DAYS: &str = "transfer_days";
/// Choices offered for [`TRANSFER_DAYS`]; the first is the default.
pub const TRANSFER_DAYS_CHOICES: [i64; 5] = [3, 1, 5, 7, 14];

pub fn transfer_days(conn: &Connection) -> Result<i64> {
    Ok(get(conn, TRANSFER_DAYS)?.and_then(|v| v.parse().ok()).unwrap_or(TRANSFER_DAYS_CHOICES[0]))
}

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn.query_row("select value from setting where key = ?1", [key], |r| r.get(0)).optional()?)
}

/// A change of a setting the ledger carries — the month rule, the base
/// currency, the language — is a change to the ledger: journaled, so the
/// cloud sees something to send. Unjournaled, it stayed on this Mac and the
/// next copy taken from Drive put the old value back. This machine's own
/// settings and the date of the last rate fetch (journaled by the fetch) are not.
fn journal_change(conn: &Connection, key: &str, was: Option<&str>, value: Option<&str>) -> Result<()> {
    if LOCAL_ONLY.contains(&key) || key == RATES_FETCHED || was == value {
        return Ok(());
    }
    let shown = |v: Option<&str>| v.unwrap_or("—").to_string();
    crate::journal::log(
        conn,
        &crate::tr!("setting {key}: {} → {}", "настройка {key}: {} → {}", shown(was), shown(value)),
        None,
    )
}

pub fn set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    crate::db::atomic(conn, || {
        let was = get(conn, key)?;
        conn.execute(
            "insert into setting (key, value) values (?1, ?2)
             on conflict (key) do update set value = excluded.value",
            params![key, value],
        )?;
        journal_change(conn, key, was.as_deref(), Some(value))
    })
}

pub fn clear(conn: &Connection, key: &str) -> Result<()> {
    crate::db::atomic(conn, || {
        let was = get(conn, key)?;
        conn.execute("delete from setting where key = ?1", [key])?;
        journal_change(conn, key, was.as_deref(), None)
    })
}

/// The ledger's language, English when unset.
pub fn language(conn: &Connection) -> Result<Lang> {
    Ok(get(conn, LANGUAGE)?.as_deref().and_then(Lang::from_code).unwrap_or(Lang::En))
}

/// Read the ledger's language and switch this thread to it.
pub fn apply_language(conn: &Connection) -> Result<Lang> {
    let lang = language(conn)?;
    crate::i18n::set_lang(lang);
    Ok(lang)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_kept_replaced_and_cleared() {
        let conn = crate::db::open_memory().unwrap();
        assert_eq!(language(&conn).unwrap(), Lang::En);
        set(&conn, LANGUAGE, "ru").unwrap();
        assert_eq!(apply_language(&conn).unwrap(), Lang::Ru);
        set(&conn, LANGUAGE, "en").unwrap();
        assert_eq!(get(&conn, LANGUAGE).unwrap().as_deref(), Some("en"));
        clear(&conn, LANGUAGE).unwrap();
        assert_eq!(get(&conn, LANGUAGE).unwrap(), None);
        assert_eq!(crate::journal::last_id(&conn).unwrap(), 3, "three changes, three entries");
        set(&conn, LANGUAGE, "en").unwrap();
        set(&conn, LANGUAGE, "en").unwrap();
        set(&conn, THEME, "light").unwrap();
        assert_eq!(crate::journal::last_id(&conn).unwrap(), 4, "the same value again and this Mac's theme are not changes to the ledger");
        crate::i18n::set_lang(Lang::En);
    }
}
