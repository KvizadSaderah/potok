//! `potok-core` — the domain, the store and parsing statements. Not a line about a terminal.
//!
//! The boundary rule (§5 of the specification):
//!
//! > The TUI runs not one SQL query and makes not one money computation. It
//! > gets finished values and draws them.
//!
//! There are exactly two reasons. The money rules are checked by tests with no
//! terminal, and without this boundary no surface for an agent is possible.
pub mod account;
pub mod analytics;
pub mod backup;
pub mod budget;
pub mod category;
pub mod counterparty;
pub mod db;
pub mod debt;
pub mod detail;
pub mod edit;
pub mod export;
pub mod i18n;
pub mod import;
pub mod journal;
pub mod migrate;
pub mod money;
pub mod period;
pub mod query;
pub mod rate;
pub mod reconcile;
pub mod settings;
pub mod verify;

pub use money::{Currency, Money, MoneyBag};
