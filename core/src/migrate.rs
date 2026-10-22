//! Moving a ledger over from a saldo dump — stage 5, criterion §9.
//!
//! The source is a `supabase db dump --data-only` dump as `INSERT`s, the one
//! `.github/workflows/backup.yml` puts into GitHub daily. There is no live
//! connection to production and none is needed: the move repeats and does not
//! depend on whether the hosting is reachable this minute.
//!
//! What moves and how was decided by reading the real dump before the code,
//! not from memory of the schema:
//!
//! * **Posted operations only.** 242 rows in state REVERSED — 241 of them
//!   import rollbacks — enter no balance in saldo: its trigger sums POSTED
//!   only. Moving them would litter the dedup signatures with rows that never
//!   were money.
//! * **Debts as movements.** A remainder in saldo is ΣISSUE − Σthe rest
//!   (`v2_verify_debt_movements`), and the original amount is itself written
//!   as an ISSUE movement. The stored remainder is not copied: here it is
//!   computed, and copying it is exactly the mistake being left behind. It is
//!   needed for the check alone.
//! * **Time in UTC with a `Z`.** The dump has `2026-09-01 00:00:00+00`, and
//!   SQLite does not understand an offset without minutes: `date()` would
//!   return NULL, and the operations would quietly fall out of every date query.
//! * **Corrections** are stored in saldo with a negative amount. Here the
//!   amount is positive and the direction is in the effect's sign: the same
//!   movement of money. **Every moved operation has an import row with a
//!   signature**, under a file of its own, «saldo-migration». Without it the
//!   next statement import over an overlapping period would double it.
//!
//! The `auth` tables are parsed only in order to be skipped: nothing is taken
//! from them, and the dump itself is not archived — it holds password hashes.
use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection};

use crate::import::store;
use crate::money::Currency;
use crate::verify::{self, Finding};

// ── Parsing the dump ────────────────────────────────────────────────────────

/// One table from the dump: its columns and rows as text or NULL.
#[derive(Debug, Default)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl Table {
    fn index(&self, column: &str) -> Result<usize> {
        self.columns
            .iter()
            .position(|c| c == column)
            .with_context(|| format!("the dump has no column {column:?}"))
    }
}

/// Every `INSERT` of the dump in turn; rows of the needed `public` tables are
/// kept, the rest are parsed and thrown away.
///
/// Parsed character by character rather than line by line: a quoted string may
/// hold a newline, and `''` inside it is an escaped quote. And in turn rather
/// than by searching the text: a row holding `INSERT INTO "…` inside a value
/// would otherwise throw off the search for the next table.
pub fn read_tables(dump: &str, wanted: &[&str]) -> Result<HashMap<String, Table>> {
    const HEAD: &str = "INSERT INTO \"";
    let bytes = dump.as_bytes();
    let mut out: HashMap<String, Table> = HashMap::new();
    let mut at = 0usize;

    while let Some(found) = dump[at..].find(HEAD) {
        at += found + HEAD.len();
        // "schema"."table" ("a", "b") VALUES
        let schema_end = dump[at..].find('"').context("a cut-off schema name")? + at;
        let schema = &dump[at..schema_end];
        let table_start = schema_end + 3; // ".""
        let table_end = dump[table_start..].find('"').context("a cut-off table name")? + table_start;
        let table = &dump[table_start..table_end];
        let cols_start = dump[table_end..].find('(').context("no column list")? + table_end + 1;
        let cols_end = dump[cols_start..].find(')').context("a cut-off column list")? + cols_start;
        let columns: Vec<String> = dump[cols_start..cols_end]
            .split(',')
            .map(|c| c.trim().trim_matches('"').to_string())
            .collect();
        let values_at = dump[cols_end..].find("VALUES").context("no VALUES")? + cols_end + 6;

        let keep = schema == "public" && wanted.contains(&table);
        let mut rows = Vec::new();
        let mut i = values_at;
        loop {
            while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n' | b',') {
                i += 1;
            }
            match bytes.get(i) {
                Some(b';') => {
                    i += 1;
                    break;
                }
                Some(b'(') => i += 1,
                other => bail!(
                    "{schema}.{table}: expected '(' or ';', found {:?}",
                    other.map(|b| *b as char)
                ),
            }
            let mut row: Vec<Option<String>> = Vec::with_capacity(columns.len());
            loop {
                while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    i += 1;
                }
                match bytes.get(i) {
                    Some(b'\'') => {
                        i += 1;
                        let mut value = String::new();
                        let mut start = i;
                        loop {
                            match bytes.get(i) {
                                Some(b'\'') if bytes.get(i + 1) == Some(&b'\'') => {
                                    value.push_str(&dump[start..i]);
                                    value.push('\'');
                                    i += 2;
                                    start = i;
                                }
                                Some(b'\'') => {
                                    value.push_str(&dump[start..i]);
                                    i += 1;
                                    break;
                                }
                                Some(_) => i += 1,
                                None => bail!("{schema}.{table}: a quoted string is not closed"),
                            }
                        }
                        row.push(Some(value));
                    }
                    Some(b'E') if bytes.get(i + 1) == Some(&b'\'') => bail!(
                        "{schema}.{table}: a string like E'…' — the dump was taken without \
                         standard_conforming_strings, and cannot be parsed blind"
                    ),
                    Some(_) => {
                        let start = i;
                        while !matches!(bytes.get(i), Some(b',' | b')') | None) {
                            i += 1;
                        }
                        let token = dump[start..i].trim();
                        row.push(if token == "NULL" { None } else { Some(token.to_string()) });
                    }
                    None => bail!("{schema}.{table}: a row is cut off"),
                }
                while matches!(bytes.get(i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    i += 1;
                }
                match bytes.get(i) {
                    Some(b',') => i += 1,
                    Some(b')') => {
                        i += 1;
                        break;
                    }
                    other => bail!(
                        "{schema}.{table}: after a value expected ',' or ')', found {:?}",
                        other.map(|b| *b as char)
                    ),
                }
            }
            if row.len() != columns.len() {
                bail!(
                    "{schema}.{table}: a row has {} values for {} columns",
                    row.len(),
                    columns.len()
                );
            }
            if keep {
                rows.push(row);
            }
        }
        at = i;
        if keep {
            let entry = out.entry(table.to_string()).or_default();
            entry.columns = columns;
            entry.rows.extend(rows);
        }
    }
    Ok(out)
}

/// `numeric(15,2)` from the dump → minor units. Exactly, with no `f64`.
///
/// Parsed separately rather than by `Money::parse`: that one guesses the
/// thousands separator in a bank's text, while here the format is known and
/// there is nothing to guess. «More than two decimals» is an error, not rounding.
pub fn minor_of(text: &str) -> Result<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let digits_only = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !digits_only(whole) || !digits_only(frac) || frac.len() > 2 {
        bail!("{text:?} is not an amount with at most two decimals");
    }
    let value = whole.parse::<i64>()? * 100 + format!("{frac:0<2}").parse::<i64>()?;
    Ok(if negative { -value } else { value })
}

/// `2026-09-01 00:00:00+00` → `2026-09-01T00:00:00Z`.
pub fn utc_iso(text: &str) -> Result<String> {
    let parsed = DateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f%#z")
        .with_context(|| format!("{text:?} is not a PostgreSQL timestamp"))?;
    Ok(parsed.with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn date_of(text: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(&text[..10.min(text.len())], "%Y-%m-%d")
        .with_context(|| format!("{text:?} is not a date"))
}

// ── The move ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AccountCheck {
    pub name: String,
    pub currency: String,
    /// `current_balance` — what saldo stores.
    pub saldo_stored: i64,
    /// The opening balance plus the posted operations from the dump. If that
    /// disagrees with the stored one, the drift is in saldo, not in the move.
    pub saldo_recomputed: i64,
    pub potok: i64,
}

#[derive(Debug, Clone)]
pub struct DebtCheck {
    pub counterparty: String,
    pub currency: String,
    pub saldo_stored: i64,
    pub saldo_recomputed: i64,
    pub potok: i64,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub user_id: String,
    pub accounts: Vec<AccountCheck>,
    pub debts: Vec<DebtCheck>,
    pub operations: usize,
    pub reversed_skipped: usize,
    pub reversed_from_rollbacks: usize,
    pub pairs: usize,
    pub categories: usize,
    pub counterparties: usize,
    /// Counterparties all of whose operations in saldo were in one category —
    /// they get the category whole, and future imports take it on their own.
    pub counterparties_categorized: usize,
    /// Operations whose category differs from the counterparty's, or whose
    /// counterparty has none — their category is their own.
    pub own_categories: usize,
    pub movements: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    /// §9: every balance and every debt remainder agrees with what saldo
    /// stores, and the invariants hold.
    pub fn agrees(&self) -> bool {
        self.accounts.iter().all(|a| a.potok == a.saldo_stored)
            && self.debts.iter().all(|d| d.potok == d.saldo_stored)
            && self.findings.is_empty()
    }
}

const ROLES: [&str; 8] = [
    "income",
    "expense",
    "transfer",
    "passthrough",
    "debt_movement",
    "opening_balance",
    "correction",
    "excluded",
];

fn kind_of(wallet_type: &str) -> &'static str {
    match wallet_type {
        "cash" => "CASH",
        "card" | "credit_card" => "CARD",
        "checking" | "savings" | "bank" => "BANK",
        _ => "OTHER",
    }
}

/// One user's rows: a column's value by name.
struct Records<'a> {
    table: &'a Table,
    rows: Vec<&'a Vec<Option<String>>>,
}

impl<'a> Records<'a> {
    fn of(tables: &'a HashMap<String, Table>, name: &str, user_id: &str) -> Result<Self> {
        static EMPTY: Table = Table { columns: Vec::new(), rows: Vec::new() };
        let table = tables.get(name).unwrap_or(&EMPTY);
        if table.rows.is_empty() {
            return Ok(Self { table, rows: Vec::new() });
        }
        let user = table.index("user_id")?;
        let rows = table
            .rows
            .iter()
            .filter(|row| row[user].as_deref() == Some(user_id))
            .collect();
        Ok(Self { table, rows })
    }

    fn col(&self, name: &str) -> Result<usize> {
        self.table.index(name)
    }
}

fn need<'r>(row: &'r [Option<String>], at: usize, what: &str) -> Result<&'r str> {
    row[at].as_deref().with_context(|| format!("{what} is empty"))
}

pub fn run(
    conn: &mut Connection,
    dump: &str,
    dump_name: &str,
    dump_sha: &str,
    user_prefix: &str,
) -> Result<Report> {
    let existing: i64 = conn.query_row(
        "select (select count(*) from account) + (select count(*) from operation)",
        [],
        |r| r.get(0),
    )?;
    if existing > 0 {
        bail!(
            "this ledger already has data. Migration goes into an empty one — otherwise \
             it is unclear what in the result came from saldo. Use another database: --db <path>"
        );
    }

    let tables = read_tables(
        dump,
        &["v2_wallets", "v2_operations", "v2_debts", "v2_debt_movements"],
    )?;

    // The user by an id prefix — and strictly one. saldo's database also has a
    // test account; guessing between two is worse than refusing.
    let wallets_table = tables.get("v2_wallets").context("the dump has no v2_wallets")?;
    let user_col = wallets_table.index("user_id")?;
    let mut users: Vec<&str> = wallets_table
        .rows
        .iter()
        .filter_map(|row| row[user_col].as_deref())
        .filter(|id| id.starts_with(user_prefix))
        .collect();
    users.sort();
    users.dedup();
    let user_id = match users.as_slice() {
        [one] => one.to_string(),
        [] => bail!("the dump has no user whose id starts with {user_prefix:?}"),
        many => bail!("the prefix {user_prefix:?} fits {} users — give a longer one", many.len()),
    };

    let wallets = Records::of(&tables, "v2_wallets", &user_id)?;
    let operations = Records::of(&tables, "v2_operations", &user_id)?;
    let debts = Records::of(&tables, "v2_debts", &user_id)?;
    let movements = Records::of(&tables, "v2_debt_movements", &user_id)?;

    let transaction = conn.transaction()?;

    // ── Accounts ──
    let (w_id, w_name, w_currency, w_type, w_state, w_inclusion, w_initial, w_current) = (
        wallets.col("id")?,
        wallets.col("name")?,
        wallets.col("currency")?,
        wallets.col("wallet_type")?,
        wallets.col("state")?,
        wallets.col("inclusion")?,
        wallets.col("initial_balance")?,
        wallets.col("current_balance")?,
    );
    let mut account_of: HashMap<String, (i64, Currency)> = HashMap::new();
    for row in &wallets.rows {
        let name = need(row, w_name, "wallet name")?;
        let currency = Currency::new(need(row, w_currency, "wallet currency")?)?;
        let state = need(row, w_state, "wallet state")?;
        // EXCLUDED_FROM_TOTALS in saldo is a state; here the same thing is
        // expressed by inclusion, and the account stays active.
        let (state, inclusion) = match state {
            "EXCLUDED_FROM_TOTALS" => ("ACTIVE", "EXCLUDED"),
            "ARCHIVED" => ("ARCHIVED", need(row, w_inclusion, "inclusion")?),
            _ => ("ACTIVE", need(row, w_inclusion, "inclusion")?),
        };
        transaction
            .execute(
                "insert into account (name, currency, kind, state, inclusion, opening_minor)
                 values (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    name,
                    currency.as_str(),
                    kind_of(need(row, w_type, "wallet type")?),
                    state,
                    inclusion,
                    minor_of(need(row, w_initial, "opening balance")?)?,
                ],
            )
            .with_context(|| format!("account {name:?} was not created"))?;
        account_of
            .insert(need(row, w_id, "wallet id")?.to_string(), (transaction.last_insert_rowid(), currency));
    }

    // ── The source file for the signatures ──
    transaction.execute(
        "insert into import_file (sha256, bank, filename, archived_at)
         values (?1, 'saldo-migration', ?2, ?3)",
        params![
            dump_sha,
            dump_name,
            // The dump is not archived: it holds auth tables with password hashes.
            "(not archived: the dump holds auth tables)"
        ],
    )?;
    let file_id = transaction.last_insert_rowid();

    // ── Operations ──
    let (o_id, o_wallet, o_role, o_state, o_at, o_amount, o_currency, o_effect, o_desc, o_batch, o_pair, o_cat) = (
        operations.col("id")?,
        operations.col("wallet_id")?,
        operations.col("role")?,
        operations.col("state")?,
        operations.col("occurred_at")?,
        operations.col("amount")?,
        operations.col("currency")?,
        operations.col("balance_effect")?,
        operations.col("description")?,
        operations.col("import_batch_id")?,
        operations.col("transfer_pair_id")?,
        operations.col("category")?,
    );

    let mut posted: Vec<&Vec<Option<String>>> = Vec::new();
    let mut reversed_skipped = 0usize;
    let mut reversed_from_rollbacks = 0usize;
    for row in &operations.rows {
        match need(row, o_state, "operation state")? {
            "POSTED" => posted.push(row),
            "REVERSED" => {
                reversed_skipped += 1;
                if row[o_batch].is_some() {
                    reversed_from_rollbacks += 1;
                }
            }
            other => bail!("an operation in state {other:?}: not expected in saldo, migration stopped"),
        }
    }
    // The insert order is by time, then by id: the move repeats, and the ids in
    // potok come out the same on every run.
    posted.sort_by(|a, b| (&a[o_at], &a[o_id]).cmp(&(&b[o_at], &b[o_id])));

    let mut operation_of: HashMap<String, i64> = HashMap::new();
    let mut saldo_category: Vec<(i64, Option<i64>, Option<String>)> = Vec::new();
    for (index, row) in posted.iter().enumerate() {
        let saldo_id = need(row, o_id, "operation id")?;
        let wallet = need(row, o_wallet, "operation wallet")?;
        let (account_id, account_currency) = *account_of
            .get(wallet)
            .with_context(|| format!("operation {saldo_id} points at another user's wallet"))?;
        let role = need(row, o_role, "operation role")?;
        if !ROLES.contains(&role) {
            bail!("operation {saldo_id}: role {role:?} does not exist in potok, migration stopped");
        }
        let currency = Currency::new(need(row, o_currency, "operation currency")?)?;
        if currency != account_currency {
            bail!("operation {saldo_id} is in {currency}, its wallet in {account_currency}");
        }
        let amount = minor_of(need(row, o_amount, "amount")?)?.abs();
        let effect = minor_of(need(row, o_effect, "effect")?)?;
        if amount == 0 || !(effect.abs() == amount || (role == "excluded" && effect == 0)) {
            bail!(
                "operation {saldo_id} ({role}): amount {amount}, effect {effect} — \
                 potok cannot record that, migration stopped"
            );
        }
        let occurred_raw = need(row, o_at, "operation date")?;
        let occurred_at = utc_iso(occurred_raw)?;
        let description = row[o_desc].clone().unwrap_or_default();
        let counterparty_id = crate::counterparty::resolve_or_create(&transaction, &description)?;

        transaction.execute(
            "insert into import_row (file_id, line_no, raw, signature) values (?1, ?2, ?3, ?4)",
            params![
                file_id,
                index as i64 + 1,
                format!("saldo v2_operations {saldo_id}"),
                store::signature_of(account_id, date_of(&occurred_at)?, effect, &description),
            ],
        )?;
        let import_row_id = transaction.last_insert_rowid();
        transaction.execute(
            "insert into operation
                (account_id, role, state, occurred_at, amount_minor, currency, effect_minor,
                 description, counterparty_id, import_row_id)
             values (?1, ?2, 'POSTED', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                account_id,
                role,
                occurred_at,
                amount,
                currency.as_str(),
                effect,
                description,
                counterparty_id,
                import_row_id,
            ],
        )?;
        let id = transaction.last_insert_rowid();
        operation_of.insert(saldo_id.to_string(), id);
        saldo_category.push((id, counterparty_id, row[o_cat].clone()));
    }

    // ── Transfer pairs ── on a second pass: the partner must already exist.
    let mut pairs = 0usize;
    for row in &posted {
        let Some(partner) = row[o_pair].as_deref() else { continue };
        let (Some(&mine), Some(&theirs)) =
            (operation_of.get(need(row, o_id, "id")?), operation_of.get(partner))
        else {
            continue;
        };
        transaction.execute("update operation set pair_id = ?1 where id = ?2", params![theirs, mine])?;
        pairs += 1;
    }

    // ── Categories ──
    // A counterparty's category is set only if in saldo all of its operations
    // were in it — including having none. This is a move, not a guess: if
    // Kaufland has twenty purchases in «Groceries» and three with no category,
    // giving the counterparty «Groceries» would quietly change three rows of a
    // report. Such a counterparty is marked later with one command, by hand.
    let mut category_id: BTreeMap<String, i64> = BTreeMap::new();
    for (_, _, category) in &saldo_category {
        if let Some(name) = category {
            if !category_id.contains_key(name) {
                transaction.execute("insert into category (name) values (?1)", params![name])?;
                category_id.insert(name.clone(), transaction.last_insert_rowid());
            }
        }
    }
    let mut by_counterparty: HashMap<i64, Vec<Option<&String>>> = HashMap::new();
    for (_, counterparty, category) in &saldo_category {
        if let Some(cp) = counterparty {
            by_counterparty.entry(*cp).or_default().push(category.as_ref());
        }
    }
    let mut counterparty_category: HashMap<i64, &String> = HashMap::new();
    for (cp, categories) in &by_counterparty {
        if let Some(Some(first)) = categories.first() {
            if categories.iter().all(|c| *c == Some(*first)) {
                transaction.execute(
                    "update counterparty set category_id = ?1 where id = ?2",
                    params![category_id[*first], cp],
                )?;
                counterparty_category.insert(*cp, *first);
            }
        }
    }
    let mut own_categories = 0usize;
    for (operation, counterparty, category) in &saldo_category {
        let Some(name) = category else { continue };
        let inherited = counterparty.and_then(|cp| counterparty_category.get(&cp).copied());
        if inherited != Some(name) {
            transaction.execute(
                "update operation set category_id = ?1 where id = ?2",
                params![category_id[name], operation],
            )?;
            own_categories += 1;
        }
    }

    // ── Debts and movements ──
    let (d_id, d_cp, d_dir, d_currency, d_remaining, d_created, d_due) = (
        debts.col("id")?,
        debts.col("counterparty")?,
        debts.col("direction")?,
        debts.col("currency")?,
        debts.col("remaining_amount")?,
        debts.col("created_at")?,
        debts.col("due_on")?,
    );
    let (m_debt, m_op, m_kind, m_amount, m_on, m_note) = (
        movements.col("debt_id")?,
        movements.col("operation_id")?,
        movements.col("kind")?,
        movements.col("amount")?,
        movements.col("occurred_on")?,
        movements.col("note")?,
    );
    let mut debt_checks = Vec::new();
    let mut moved = 0usize;
    for row in &debts.rows {
        let saldo_id = need(row, d_id, "debt id")?;
        let name = need(row, d_cp, "debt counterparty")?;
        let currency = need(row, d_currency, "debt currency")?;
        let mine: Vec<_> =
            movements.rows.iter().filter(|m| m[m_debt].as_deref() == Some(saldo_id)).collect();
        let opened_on = mine
            .iter()
            .filter(|m| m[m_kind].as_deref() == Some("ISSUE"))
            .filter_map(|m| m[m_on].clone())
            .min()
            .map(Ok)
            .unwrap_or_else(|| date_of(need(row, d_created, "debt date")?).map(|d| d.to_string()))?;
        let counterparty_id = crate::counterparty::resolve_or_create(&transaction, name)?
            .with_context(|| format!("no counterparty came out of the name {name:?}"))?;
        transaction.execute(
            "insert into debt (counterparty_id, direction, currency, opened_on, due_on)
             values (?1, ?2, ?3, ?4, ?5)",
            params![counterparty_id, need(row, d_dir, "direction")?, currency, opened_on, row[d_due]],
        )?;
        let debt_id = transaction.last_insert_rowid();

        let mut recomputed = 0i64;
        for m in &mine {
            let kind = need(m, m_kind, "movement kind")?;
            let amount = minor_of(need(m, m_amount, "movement amount")?)?;
            if amount <= 0 {
                bail!("debt movement {saldo_id}: amount {amount} ≤ 0 — a movement has one sign in potok");
            }
            recomputed += if kind == "ISSUE" { amount } else { -amount };
            let operation_id = m[m_op].as_deref().and_then(|op| operation_of.get(op).copied());
            transaction.execute(
                "insert into debt_movement (debt_id, operation_id, kind, amount_minor, occurred_on, note)
                 values (?1, ?2, ?3, ?4, ?5, ?6)",
                params![debt_id, operation_id, kind, amount, need(m, m_on, "movement date")?, m[m_note]],
            )?;
            moved += 1;
        }
        debt_checks.push((debt_id, name.to_string(), currency.to_string(),
                          minor_of(need(row, d_remaining, "remainder")?)?, recomputed));
    }

    crate::journal::log(
        &transaction,
        &crate::tr!(
            "migrated from saldo «{dump_name}»: {} operations",
            "перенесено из saldo «{dump_name}»: операций {}",
            posted.len()
        ),
        None,
    )?;
    transaction.commit()?;

    // ── The §9 check ── on what is written, through the views a screen reads.
    let mut accounts = Vec::new();
    for row in &wallets.rows {
        let (account_id, currency) = account_of[need(row, w_id, "id")?];
        let saldo_recomputed = minor_of(need(row, w_initial, "opening")?)?
            + posted
                .iter()
                .filter(|o| o[o_wallet] == row[w_id])
                .map(|o| minor_of(need(o, o_effect, "effect")?))
                .sum::<Result<i64>>()?;
        let potok: i64 = conn.query_row(
            "select balance_minor from account_balance where account_id = ?1",
            params![account_id],
            |r| r.get(0),
        )?;
        accounts.push(AccountCheck {
            name: need(row, w_name, "name")?.to_string(),
            currency: currency.as_str().to_string(),
            saldo_stored: minor_of(need(row, w_current, "stored balance")?)?,
            saldo_recomputed,
            potok,
        });
    }
    let mut debt_report = Vec::new();
    for (debt_id, counterparty, currency, saldo_stored, saldo_recomputed) in debt_checks {
        let potok: i64 = conn.query_row(
            "select remaining_minor from debt_remaining where debt_id = ?1",
            params![debt_id],
            |r| r.get(0),
        )?;
        debt_report.push(DebtCheck { counterparty, currency, saldo_stored, saldo_recomputed, potok });
    }
    let counterparties: i64 =
        conn.query_row("select count(*) from counterparty", [], |r| r.get(0))?;

    Ok(Report {
        user_id,
        accounts,
        debts: debt_report,
        operations: posted.len(),
        reversed_skipped,
        reversed_from_rollbacks,
        pairs,
        categories: category_id.len(),
        counterparties: counterparties as usize,
        counterparties_categorized: counterparty_category.len(),
        own_categories,
        movements: moved,
        findings: verify::run(conn)?.findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    const ME: &str = "067eebb4-0000-0000-0000-000000000001";
    const TEST_ACCOUNT: &str = "c747171b-0000-0000-0000-000000000002";

    /// A miniature dump in the real shape: multi-line INSERTs, `+00` with no
    /// minutes, `''` quotes, a newline inside a value, an auth table with an
    /// «INSERT INTO» inside the data. The figures are invented.
    fn dump() -> String {
        format!(
            r#"--
-- PostgreSQL database dump
--
SET statement_timeout = 0;

INSERT INTO "auth"."users" ("id", "raw_user_meta_data") VALUES
	('{ME}', '{{"note": "INSERT INTO \"public\".\"v2_wallets\" (\"id\") VALUES"}}');

INSERT INTO "public"."v2_wallets" ("id", "user_id", "name", "currency", "wallet_type", "state", "inclusion", "initial_balance", "current_balance", "legacy_account_id", "created_at", "updated_at") VALUES
	('w-rev', '{ME}', 'Revolut', 'EUR', 'checking', 'ACTIVE', 'FULL', 100.00, 37.89, NULL, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00'),
	('w-cash', '{ME}', 'Cash', 'EUR', 'cash', 'ACTIVE', 'FULL', 0.00, 50.00, NULL, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00'),
	('w-test', '{TEST_ACCOUNT}', 'Fake', 'EUR', 'checking', 'ACTIVE', 'FULL', 10000000.00, 10000000.00, NULL, '2026-01-01 00:00:00+00', '2026-01-01 00:00:00+00');

INSERT INTO "public"."v2_operations" ("id", "user_id", "wallet_id", "role", "state", "occurred_at", "amount", "currency", "balance_effect", "household_total_effect", "description", "import_batch_id", "legacy_transaction_id", "created_at", "updated_at", "transfer_pair_id", "category", "audit_note") VALUES
	('o1', '{ME}', 'w-rev', 'expense', 'POSTED', '2026-09-01 00:00:00+00', 6.24, 'EUR', -6.24, -6.24, 'Kaufland', 'b1', NULL, '2026-09-01 00:00:00+00', '2026-09-01 00:00:00+00', NULL, 'Groceries', NULL),
	('o2', '{ME}', 'w-rev', 'expense', 'POSTED', '2026-09-02 12:00:00+00', 3.50, 'EUR', -3.50, -3.50, 'POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR', 'b1', NULL, '2026-09-02 00:00:00+00', '2026-09-02 00:00:00+00', NULL, 'Groceries', NULL),
	('o3', '{ME}', 'w-rev', 'expense', 'REVERSED', '2026-09-03 00:00:00+00', 999.00, 'EUR', -999.00, -999.00, 'rolled back', 'b2', NULL, '2026-09-03 00:00:00+00', '2026-09-03 00:00:00+00', NULL, NULL, NULL),
	('o4', '{ME}', 'w-rev', 'transfer', 'POSTED', '2026-09-04 00:00:00+00', 50.00, 'EUR', -50.00, 0.00, 'To cash', NULL, NULL, '2026-09-04 00:00:00+00', '2026-09-04 00:00:00+00', 'o5', NULL, NULL),
	('o5', '{ME}', 'w-cash', 'transfer', 'POSTED', '2026-09-04 00:00:00+00', 50.00, 'EUR', 50.00, 0.00, 'From revolut', NULL, NULL, '2026-09-04 00:00:00+00', '2026-09-04 00:00:00+00', 'o4', NULL, NULL),
	('o6', '{ME}', 'w-rev', 'excluded', 'POSTED', '2026-09-05 00:00:00+00', 800.00, 'EUR', 0.00, 0.00, 'Told to ignore', 'b1', NULL, '2026-09-05 00:00:00+00', '2026-09-05 00:00:00+00', NULL, NULL, NULL),
	('o7', '{ME}', 'w-rev', 'correction', 'POSTED', '2026-09-06 00:00:00+00', -2.37, 'EUR', -2.37, 0.00, 'Reconcile', NULL, NULL, '2026-09-06 00:00:00+00', '2026-09-06 00:00:00+00', NULL, NULL, NULL),
	('o8', '{ME}', 'w-rev', 'debt_movement', 'POSTED', '2026-09-07 00:00:00+00', 100.00, 'EUR', 100.00, 0.00, 'Borrowed from Pat', NULL, NULL, '2026-09-07 00:00:00+00', '2026-09-07 00:00:00+00', NULL, NULL, 'line one
line two, with a quote: it''s here'),
	('o9', '{ME}', 'w-rev', 'debt_movement', 'POSTED', '2026-09-08 00:00:00+00', 100.00, 'EUR', -100.00, 0.00, 'Paid Pat back', NULL, NULL, '2026-09-08 00:00:00+00', '2026-09-08 00:00:00+00', NULL, NULL, NULL),
	('ot', '{TEST_ACCOUNT}', 'w-test', 'expense', 'POSTED', '2026-09-01 00:00:00+00', 1.00, 'EUR', -1.00, -1.00, 'test', NULL, NULL, '2026-09-01 00:00:00+00', '2026-09-01 00:00:00+00', NULL, NULL, NULL);

INSERT INTO "public"."v2_debts" ("id", "user_id", "counterparty", "direction", "original_amount", "currency", "remaining_amount", "state", "legacy_debt_id", "created_at", "updated_at", "due_on", "expected_payment_amount", "wallet_id", "settlement_chain", "source_trace") VALUES
	('d1', '{ME}', 'Pat', 'I_OWE', 300.00, 'EUR', 120.00, 'OPEN', NULL, '2026-09-07 10:00:00+00', '2026-09-07 10:00:00+00', '2026-12-01', NULL, NULL, NULL, NULL),
	('dt', '{TEST_ACCOUNT}', 'Fake person', 'OWED_TO_ME', 5.00, 'EUR', 5.00, 'OPEN', NULL, '2026-09-01 00:00:00+00', '2026-09-01 00:00:00+00', NULL, NULL, NULL, NULL, NULL);

INSERT INTO "public"."v2_debt_movements" ("id", "user_id", "debt_id", "operation_id", "kind", "amount", "currency", "occurred_on", "note", "created_at") VALUES
	('m1', '{ME}', 'd1', 'o8', 'ISSUE', 100.00, 'EUR', '2026-09-07', NULL, '2026-09-07 00:00:00+00'),
	('m2', '{ME}', 'd1', NULL, 'ISSUE', 200.00, 'EUR', '2026-09-10', 'second loan', '2026-09-10 00:00:00+00'),
	('m3', '{ME}', 'd1', 'o9', 'REPAYMENT', 100.00, 'EUR', '2026-09-08', NULL, '2026-09-08 00:00:00+00'),
	('m4', '{ME}', 'd1', 'o3', 'REPAYMENT', 80.00, 'EUR', '2026-09-20', NULL, '2026-09-20 00:00:00+00'),
	('mt', '{TEST_ACCOUNT}', 'dt', NULL, 'ISSUE', 5.00, 'EUR', '2026-09-01', NULL, '2026-09-01 00:00:00+00');
"#
        )
    }

    fn migrated() -> (Connection, Report) {
        let mut conn = db::open_memory().unwrap();
        let report = run(&mut conn, &dump(), "test.sql", "sha-test", "067e").unwrap();
        (conn, report)
    }

    // ── parsing ──

    #[test]
    fn quotes_newlines_and_nulls_survive_the_reader() {
        let tables = read_tables(&dump(), &["v2_operations"]).unwrap();
        let ops = &tables["v2_operations"];
        let note = ops.index("audit_note").unwrap();
        let o8 = ops.rows.iter().find(|r| r[0].as_deref() == Some("o8")).unwrap();
        assert_eq!(
            o8[note].as_deref(),
            Some("line one\nline two, with a quote: it's here"),
            "a newline and '' inside a value"
        );
        let pair = ops.index("transfer_pair_id").unwrap();
        assert!(ops.rows[0][pair].is_none(), "NULL is None, not the string \"NULL\"");
    }

    /// A row holding «INSERT INTO "public"…» inside the auth data must not throw
    /// off the search for the next table — which is why parsing goes in turn.
    #[test]
    fn text_that_looks_like_a_statement_inside_a_value_is_just_text() {
        let tables = read_tables(&dump(), &["v2_wallets"]).unwrap();
        assert_eq!(tables["v2_wallets"].rows.len(), 3);
    }

    #[test]
    fn a_dump_without_standard_strings_is_refused_rather_than_misread() {
        let bad = "INSERT INTO \"public\".\"v2_wallets\" (\"id\") VALUES\n\t(E'x\\'y');";
        assert!(read_tables(bad, &["v2_wallets"]).is_err());
    }

    #[test]
    fn numerics_are_read_exactly() {
        assert_eq!(minor_of("2805.09").unwrap(), 280509);
        assert_eq!(minor_of("-70.39").unwrap(), -7039);
        assert_eq!(minor_of("0").unwrap(), 0);
        assert_eq!(minor_of("0.5").unwrap(), 50, "one digit after the point is tens of cents");
        assert_eq!(minor_of("10000000.00").unwrap(), 1_000_000_000);
        assert!(minor_of("1.005").is_err(), "three decimals is an error, not rounding");
        assert!(minor_of("1,00").is_err());
        assert!(minor_of("").is_err());
    }

    /// `+00` with no minutes SQLite does not understand: `date()` would be NULL.
    #[test]
    fn a_postgres_timestamp_becomes_one_sqlite_understands() {
        assert_eq!(utc_iso("2026-09-01 00:00:00+00").unwrap(), "2026-09-01T00:00:00Z");
        assert_eq!(utc_iso("2026-09-01 10:23:45.123456+00").unwrap(), "2026-09-01T10:23:45Z");
        assert_eq!(utc_iso("2026-09-01 01:30:00+03").unwrap(), "2026-08-31T22:30:00Z");
        let conn = Connection::open_in_memory().unwrap();
        let (bad, good): (Option<String>, Option<String>) = conn
            .query_row("select date('2026-09-01 00:00:00+00'), date(?1)", [utc_iso("2026-09-01 00:00:00+00").unwrap()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(bad, None, "if SQLite learns it, the check is out of date rather than broken");
        assert_eq!(good.as_deref(), Some("2026-09-01"));
    }

    // ── the move ──

    /// §9 in miniature: every balance and every remainder agrees with what
    /// saldo stores.
    #[test]
    fn every_balance_and_every_debt_matches_what_saldo_stored() {
        let (_, report) = migrated();
        assert!(report.agrees(), "{report:#?}");
        let revolut = report.accounts.iter().find(|a| a.name == "Revolut").unwrap();
        assert_eq!(revolut.potok, 3789, "100 − 6.24 − 3.50 − 50 − 2.37 + 100 − 100");
        let debt = &report.debts[0];
        assert_eq!(debt.potok, 12000, "300 lent − 180 paid back");
        assert_eq!(debt.saldo_recomputed, debt.saldo_stored);
    }

    /// saldo's test account — a fake wallet of ten million — must not land in
    /// your ledger.
    #[test]
    fn nobody_else_is_carried() {
        let (conn, report) = migrated();
        assert_eq!(report.user_id, ME);
        let names: Vec<String> = conn
            .prepare("select name from account order by name").unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(names, ["Cash", "Revolut"]);
        let debts: i64 = conn.query_row("select count(*) from debt", [], |r| r.get(0)).unwrap();
        assert_eq!(debts, 1);
    }

    /// Rollbacks enter no balance in saldo — and do not come here either.
    #[test]
    fn a_reversed_operation_is_not_carried() {
        let (conn, report) = migrated();
        assert_eq!(report.operations, 8);
        assert_eq!(report.reversed_skipped, 1);
        assert_eq!(report.reversed_from_rollbacks, 1);
        let found: i64 = conn
            .query_row("select count(*) from operation where description = 'rolled back'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(found, 0);
    }

    /// A debt movement that referred to a rolled-back operation stays — it is
    /// the debt's money — but the reference is cleared.
    #[test]
    fn a_movement_pointing_at_a_reversed_operation_keeps_its_money_and_loses_the_link() {
        let (conn, _) = migrated();
        let linked: Option<i64> = conn
            .query_row("select operation_id from debt_movement where amount_minor = 8000", [], |r| r.get(0))
            .unwrap();
        assert_eq!(linked, None);
    }

    #[test]
    fn transfer_legs_are_paired_both_ways() {
        let (conn, report) = migrated();
        assert_eq!(report.pairs, 2);
        let symmetric: i64 = conn
            .query_row(
                "select count(*) from operation a join operation b on b.id = a.pair_id where b.pair_id = a.id",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(symmetric, 2);
    }

    /// A zero «excluded» row moves over — otherwise the next import would bring
    /// it back as a spend. A negative correction becomes a positive amount with
    /// a negative effect.
    #[test]
    fn the_odd_rows_are_carried_in_their_own_form() {
        let (conn, _) = migrated();
        let (amount, effect): (i64, i64) = conn
            .query_row("select amount_minor, effect_minor from operation where role = 'excluded'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((amount, effect), (80000, 0));
        let (amount, effect): (i64, i64) = conn
            .query_row("select amount_minor, effect_minor from operation where role = 'correction'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((amount, effect), (237, -237));
    }

    /// Kaufland out of two banks is one counterparty, and its unanimous category
    /// moves onto it whole, so that future imports take it on their own.
    #[test]
    fn a_unanimous_category_moves_to_the_counterparty() {
        let (conn, report) = migrated();
        assert_eq!(report.counterparties_categorized, 1);
        let category: String = conn
            .query_row(
                "select c.name from counterparty p join category c on c.id = p.category_id where p.name = 'Kaufland'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(category, "Groceries");
        assert_eq!(report.own_categories, 0, "an own category is needed only when it differs");
    }

    /// The time is written so that SQLite understands it — otherwise date
    /// queries would quietly lose operations.
    #[test]
    fn every_operation_is_found_by_date() {
        let (conn, _) = migrated();
        let dated: i64 = conn
            .query_row("select count(*) from operation where date(occurred_at) is not null", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dated, 8);
    }

    /// Every moved operation has an import row with a signature, or the next
    /// import of the same statement would double what was moved.
    #[test]
    fn every_carried_operation_can_be_recognised_by_the_next_import() {
        let (conn, _) = migrated();
        let unsigned: i64 = conn
            .query_row("select count(*) from operation where import_row_id is null", [], |r| r.get(0))
            .unwrap();
        assert_eq!(unsigned, 0);
    }

    #[test]
    fn a_ledger_that_already_holds_data_is_refused() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind) values ('X','EUR','BANK');").unwrap();
        let error = run(&mut conn, &dump(), "t", "s", "067e").unwrap_err().to_string();
        assert!(error.contains("--db"), "{error}");
    }

    #[test]
    fn an_ambiguous_or_unknown_user_is_refused_rather_than_guessed() {
        let mut conn = db::open_memory().unwrap();
        assert!(run(&mut conn, &dump(), "t", "s", "").unwrap_err().to_string().contains("fits 2 users"));
        let mut conn = db::open_memory().unwrap();
        assert!(run(&mut conn, &dump(), "t", "s", "ffff").is_err());
    }

    /// The move is all or nothing: a failure halfway leaves no half ledger.
    #[test]
    fn a_failure_part_way_leaves_nothing_behind() {
        let broken = dump().replace("'Told to ignore'", "'x'").replace("800.00, 'EUR', 0.00", "800.00, 'EUR', 1.00");
        let mut conn = db::open_memory().unwrap();
        assert!(run(&mut conn, &broken, "t", "s", "067e").is_err());
        let left: i64 = conn
            .query_row("select (select count(*) from account) + (select count(*) from operation)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "the transaction should have rolled back whole");
    }
}
