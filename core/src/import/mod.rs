//! Parsing statements: a file → the rows a person confirms.
//!
//! Nothing here writes to the database. Parsing is a pure function of the text,
//! so it is checked by tests with no database and no PDF, and on real
//! `potok import --dry-run`.
pub mod icard;
mod mock;
pub mod pdf;
pub mod review;
pub mod revolut;
pub mod store;

use chrono::NaiveDate;

use crate::money::{Currency, Money};

/// The bank whose format was recognised. Recognised by the text rather than the
/// file's name: the name is the browser's at download and guarantees nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Revolut,
    Icard,
}

impl Bank {
    pub fn as_str(self) -> &'static str {
        match self {
            Bank::Revolut => "revolut",
            Bank::Icard => "icard",
        }
    }
}

/// One row of a statement, as the bank printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRow {
    /// The row's number in the extracted text. Together with the file's hash
    /// that is the idempotency key (§7): a row is identified by its place, not
    /// by a description, which changes as the cleanup improves.
    pub line_no: usize,
    pub occurred_on: NaiveDate,
    pub description: String,
    /// The signed amount: minus means the money left.
    pub amount: Money,
    /// The balance after the operation, if the bank prints it.
    ///
    /// Revolut prints it, and that gives a check **on every row** rather than
    /// on the batch's total alone (§7). iCard does not — there it is `None`, and
    /// that has to be said on screen rather than passed off as checked.
    pub running_balance: Option<Money>,
    /// Whether the operation is completed. A pending one is not money (invariant 6).
    pub posted: bool,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub bank: Bank,
    pub currency: Currency,
    pub rows: Vec<ParsedRow>,
}

impl Statement {
    pub fn posted(&self) -> impl Iterator<Item = &ParsedRow> {
        self.rows.iter().filter(|r| r.posted)
    }

    pub fn pending_count(&self) -> usize {
        self.rows.iter().filter(|r| !r.posted).count()
    }
}

/// Recognises the bank by the extracted text and parses it.
pub fn parse(text: &str) -> anyhow::Result<Statement> {
    if revolut::looks_like(text) {
        revolut::parse(text)
    } else if icard::looks_like(text) {
        icard::parse(text)
    } else {
        anyhow::bail!(crate::tr!(
            "format not recognised. Known now: Revolut and iCard; \
             adding a bank needs a sample of its statement",
            "формат не опознан. Сейчас известны Revolut и iCard; \
             чтобы добавить банк, нужен образец его выписки"
        ))
    }
}

/// Money moved inside one account, both halves on the same statement — iCard's
/// «Balance transfer» between two balances of a card. Written as transit: no
/// other leg to look for, no debt, not spending and not income.
pub fn moves_within_the_account(bank: Bank, description: &str) -> bool {
    bank == Bank::Icard && description.starts_with("Balance transfer")
}

// ── Small parsers shared by the banks ───────────────────────────────────────

/// `Sep 1, 2026` → a date. The month is an English abbreviation, as Revolut prints it.
pub(crate) fn parse_month_day_year(tokens: &[&str]) -> Option<NaiveDate> {
    if tokens.len() < 3 {
        return None;
    }
    let month = month_from_abbrev(tokens[0])?;
    let day: u32 = tokens[1].trim_end_matches(',').parse().ok()?;
    let year: i32 = tokens[2].parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// `06.10.2026` → a date. That is how iCard prints it.
pub(crate) fn parse_dotted_date(token: &str) -> Option<NaiveDate> {
    let mut parts = token.split('.');
    let day: u32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let year: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    NaiveDate::from_ymd_opt(year, month, day)
}

fn month_from_abbrev(token: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = token.to_ascii_lowercase();
    MONTHS.iter().position(|m| lower.starts_with(m)).map(|i| i as u32 + 1)
}

/// Whether a token looks like a sum of money: digits, and exactly two after the separator.
pub fn looks_like_amount(token: &str) -> bool {
    let cleaned: String = token
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-')
        .collect();
    match cleaned.rfind(['.', ',']) {
        Some(at) => {
            cleaned[at + 1..].len() == 2
                && cleaned[at + 1..].chars().all(|c| c.is_ascii_digit())
                && cleaned[..at].chars().any(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

pub(crate) fn parse_amount(token: &str, currency: Currency) -> Option<Money> {
    Money::parse(token, currency).ok()
}
