//! The interface's state.
//!
//! **There is not one SQL query and not one money computation here** (§5).
//! Everything arrives from `potok_core::query` ready to show.
//!
//! And there is no asynchrony. The database is a file on disk in this very
//! process, reading is instant, so there is no `Loadable`, no «loading…» and no
//! cache of the last answer — all the things half of the old client was made
//! of, the one that talked to a cloud over the network. That is the main gain
//! from local-first, and it shows right here: the search asks the database
//! again on every letter, and it goes unnoticed.
#[allow(unused_imports)]
use potok_core::{tr, trs};
use anyhow::Result;
use chrono::NaiveDate;
use potok_core::query::{
    self, AccountBalance, AccountChoice, CounterpartyLine, DebtLine, FlowChange, ImportedFile,
    OperationFilter, OperationLine,
};
use potok_core::detail;
use rusqlite::Connection;

use super::Screen;

mod actions;
mod ask;
mod forms;
mod home;
mod lock;
mod menu;
mod navigate;
mod pick;
mod settings;
mod statements;
mod types;
pub use types::*;

pub struct App {
    pub conn: Connection,
    pub screen: Screen,
    pub mode: Mode,
    pub quit: bool,
    /// `q` pressed once: the next `q` quits.
    pub quit_armed: bool,
    /// Wrong passphrases in a row, and when the next try is allowed.
    pub lock_failures: u32,
    pub lock_wait_until: Option<std::time::Instant>,
    /// When the screen was locked, for the lock screen.
    pub locked_at: Option<chrono::DateTime<chrono::Local>>,
    pub status: Option<String>,

    /// The cursor is its own on every screen: coming back to the operations, a
    /// person expects the place they were, not the top of the list.
    cursors: [usize; Screen::COUNT],

    pub accounts: Vec<AccountBalance>,
    pub account_choices: Vec<AccountChoice>,
    /// `None` — every account.
    pub account_filter: Option<i64>,
    pub operations: Vec<OperationLine>,
    pub ops_view: OpsView,
    /// What the operations screen shows when it is not the whole journal.
    view_operations: Vec<OperationLine>,
    /// The search found more than is shown.
    pub view_truncated: bool,
    pub period: Period,
    pub flows: Vec<FlowChange>,
    pub debts: Vec<DebtLine>,
    pub imports: Vec<ImportedFile>,
    pub counterparties: Vec<CounterpartyLine>,
    pub duplicates: Vec<potok_core::reconcile::DuplicateGroup>,
    pub changes: Vec<potok_core::journal::Entry>,
    /// Categories with what is in them, spending over the flows period.
    pub category_lines: Vec<potok_core::category::CategoryLine>,
    /// Everything in the base currency, and which currencies have no rate.
    pub total: Option<potok_core::rate::Total>,
    /// Each account currency against the base, with the rows it comes from.
    pub rates: Vec<potok_core::rate::Row>,
    pub overview: Option<potok_core::analytics::Overview>,
    /// The last twelve months, oldest first.
    pub months: Vec<potok_core::analytics::Month>,
    pub regular: Vec<potok_core::analytics::Regular>,
    pub budget_lines: Vec<potok_core::budget::BudgetLine>,
    /// The counterparty list's filter on the import screen — `/` searches them.
    pub counterparty_filter: String,
    /// `C`: categorising the uncategorised one after another. The list shows
    /// only them, and the picker opens again after each.
    pub queue: bool,
    pub categories: Vec<String>,
    /// The text being typed: the search line or the category filter.
    pub input: String,
    pub today: NaiveDate,
    /// The details of the row under the cursor.
    pub detail: Detail,
    /// The details over the whole screen — Enter on an operation or a debt.
    pub zoom: bool,
    pub focus: Focus,
    pub analytics_pane: AnalyticsPane,
    pub regular_cursor: usize,
    /// Which of the Overview's lists the cursor walks while it is on the left.
    pub home_pane: HomePane,
    /// The month under the cursor in the columns — the index left to right, as
    /// drawn. `None` — the cursor has not been there: that is this month, the
    /// last column. Otherwise opening would always start at January.
    pub home_month_cursor: Option<usize>,
    /// When the screen appeared. Over a third of a second the bars and columns
    /// grow to their length: that way a change of figures is visible and the
    /// screen does not jump in finished. `None` — draw it whole at once, which
    /// is what `potok screens` and the tests that need the final picture do.
    pub grew_at: Option<std::time::Instant>,
    /// «Where it went» shows not this month but the chosen column.
    pub home_past: Option<PastMonth>,
    pub detail_cursor: usize,
    /// The details are open on a particular operation (Enter inside the details
    /// or «open the operation» in the menu) and must not follow the cursor.
    pub pinned: bool,
    pub menu: Vec<MenuItem>,
    /// What waits for a «yes» in `Mode::Confirm`.
    pub pending: Option<MenuItem>,
    pub pick_target: PickTarget,
    pub review: Option<Review>,
    /// Settings read from the ledger at start and kept in step on change.
    pub statements_dir: Option<String>,
    pub default_account: Option<i64>,
    /// When accounting months start.
    pub month_rule: potok_core::period::MonthRule,
    pub form: Option<Form>,
    /// The account whose opening balance is being typed.
    pub opening_for: Option<i64>,
    pub asking: Option<Ask>,
    /// The screen a drill into operations came from, for Esc.
    pub return_to: Option<Screen>,
    /// How the journal is ordered (`o`).
    pub order: query::Order,
    /// The journal folded by counterparty (`f`).
    pub fold: bool,
    /// Transfers between own accounts left out of the journal (`t`).
    pub hide_transfers: bool,
    /// What the Accounts screen draws beside each account, in its order.
    pub account_views: Vec<AccountView>,
    /// Why the last copy to Google Drive failed, if it did.
    pub cloud_error: Option<String>,
    /// Another device holds the writer mark: the ledger is open to read only.
    pub read_only: Option<crate::cloud::Mark>,
    /// Operations marked with Space, for one action on all of them.
    pub marked: std::collections::BTreeSet<i64>,
}

/// An account's lines and figures for the Accounts screen.
#[derive(Debug, Clone)]
pub struct AccountView {
    /// The balance at the end of each of the last ten months, oldest first.
    pub months: Vec<potok_core::Money>,
    /// The balance every third day of the last thirty, and those days.
    pub days: Vec<(chrono::NaiveDate, potok_core::Money)>,
    /// What moved it in those thirty days.
    pub moves: potok_core::analytics::AccountMoves,
    pub reconciled: Option<potok_core::reconcile::Reconciliation>,
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// The PDFs from «Downloads», the new on top, no more than a dozen: a statement
/// has usually just been downloaded.
fn recent_statements(folder: &str) -> Vec<std::path::PathBuf> {
    let dir = match (folder.strip_prefix("~/"), home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(folder),
    };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    // Named as the banks name their statements first — Revolut's
    // «account-statement_…», iCard's «transactions…» — then any other PDF in
    // Downloads, newest first in each.
    let looks_like_statement = |p: &std::path::Path| {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        name.starts_with("account-statement") || name.starts_with("transactions")
    };
    found.sort_by_key(|a| (!looks_like_statement(&a.1), std::cmp::Reverse(a.0)));
    found.into_iter().take(12).map(|(_, p)| p).collect()
}

/// How many found operations to show. A search over the real ledger of 1774
/// rows gives them in milliseconds; the ceiling is only so that an «a» does not
/// dump the whole journal as a result.
const SEARCH_LIMIT: usize = 1000;

impl App {
    pub fn new(conn: Connection) -> Result<Self> {
        let mut app = Self {
            conn,
            screen: Screen::Accounts,
            mode: Mode::Normal,
            quit: false,
            quit_armed: false,
            lock_failures: 0,
            lock_wait_until: None,
            locked_at: None,
            status: None,
            cursors: [0; Screen::COUNT],
            accounts: Vec::new(),
            account_choices: Vec::new(),
            account_filter: None,
            operations: Vec::new(),
            ops_view: OpsView::All,
            view_operations: Vec::new(),
            view_truncated: false,
            period: Period::ThisMonth,
            flows: Vec::new(),
            debts: Vec::new(),
            imports: Vec::new(),
            counterparties: Vec::new(),
            duplicates: Vec::new(),
            changes: Vec::new(),
            category_lines: Vec::new(),
            total: None,
            rates: Vec::new(),
            overview: None,
            months: Vec::new(),
            regular: Vec::new(),
            budget_lines: Vec::new(),
            counterparty_filter: String::new(),
            queue: false,
            categories: Vec::new(),
            input: String::new(),
            today: chrono::Local::now().date_naive(),
            detail: Detail::Nothing,
            zoom: false,
            focus: Focus::List,
            analytics_pane: AnalyticsPane::Months,
            regular_cursor: 0,
            home_pane: HomePane::Flows,
            home_month_cursor: None,
            grew_at: None,
            home_past: None,
            detail_cursor: 0,
            pinned: false,
            menu: Vec::new(),
            pending: None,
            pick_target: PickTarget::Counterparty,
            review: None,
            statements_dir: None,
            default_account: None,
            month_rule: potok_core::period::MonthRule::calendar(),
            form: None,
            opening_for: None,
            asking: None,
            return_to: None,
            order: query::Order::Date,
            fold: false,
            hide_transfers: false,
            account_views: Vec::new(),
            cloud_error: None,
            read_only: None,
            marked: Default::default(),
        };
        app.load_settings()?;
        app.reload()?;
        Ok(app)
    }

    pub(super) fn load_operations(&mut self) -> Result<()> {
        self.operations = query::operations(&self.conn, self.account_filter, true)?;
        if self.hide_transfers {
            self.operations.retain(|o| o.role != "transfer");
        }
        // All accounts at once: a transfer is one line, its leaving leg —
        // the arriving one said the same thing again on the next row. One
        // account's journal keeps both, since there each is that account's.
        if self.account_filter.is_none() {
            self.operations.retain(|o| !(o.role == "transfer" && !o.removed && o.pair_account.is_some() && o.amount.minor() > 0));
        }
        Ok(())
    }

    pub fn reload(&mut self) -> Result<()> {
        self.accounts = query::balances(&self.conn)?;
        self.account_choices = query::account_choices(&self.conn)?;
        self.load_operations()?;
        self.reload_flows()?;
        self.debts = query::debts(&self.conn, true)?;
        self.imports = query::import_journal(&self.conn)?;
        self.counterparties = query::counterparties(&self.conn)?;
        self.duplicates = potok_core::reconcile::duplicates(&self.conn, None)?;
        self.changes = potok_core::journal::entries(&self.conn, 1000)?;
        let (from, to) = self.period.bounds_on(self.today, &self.month_rule);
        self.category_lines = potok_core::category::list(&self.conn, &from, &to)?;
        self.total = Some(potok_core::rate::total(&self.conn, &self.today.to_string())?);
        self.rates = potok_core::rate::table(&self.conn, &self.today.to_string())?;
        self.overview = Some(potok_core::analytics::overview(&self.conn, self.today)?);
        self.months = potok_core::analytics::months(&self.conn, self.today, 12)?;
        self.regular = potok_core::analytics::regular(&self.conn, self.today)?;
        self.account_views = self.account_views()?;
        self.cloud_error = potok_core::settings::get(&self.conn, potok_core::settings::CLOUD_ERROR)?;
        let month = self.month_rule.cycle(self.today, 0).start;
        self.budget_lines = potok_core::budget::status(&self.conn, &month.to_string(), &self.today.to_string())?;
        self.categories = query::categories(&self.conn)?;
        self.refresh_view()?;
        self.apply_order();
        self.clamp_cursors();
        self.refresh_detail()
    }

    pub(super) fn account_views(&self) -> Result<Vec<AccountView>> {
        use potok_core::analytics;
        let ends: Vec<chrono::NaiveDate> = self.months.iter().rev().take(10).rev().map(|m| m.end).collect();
        let since = self.today - chrono::Duration::days(30);
        let days: Vec<chrono::NaiveDate> = (0..=10).map(|i| since + chrono::Duration::days(i * 3)).collect();
        self.accounts
            .iter()
            .map(|a| {
                Ok(AccountView {
                    months: analytics::balance_history(&self.conn, a.id, &ends)?,
                    days: days.iter().copied().zip(analytics::balance_history(&self.conn, a.id, &days)?).collect(),
                    moves: analytics::account_moves(&self.conn, a.id, since, self.today)?,
                    reconciled: potok_core::reconcile::last(&self.conn, a.id)?,
                })
            })
            .collect()
    }

    /// `W` while read only: take the writer mark from the other device, take
    /// its newest copy, and open the ledger for writing.
    pub fn take_write(&mut self) -> Result<()> {
        if self.read_only.is_none() {
            self.status = Some(trs!("this device writes already", "это устройство и так пишет").into());
            return Ok(());
        }
        let Some(path) = self.conn.path().filter(|p| !p.is_empty()).map(std::path::PathBuf::from) else { return Ok(()) };
        // The ledger file may be replaced: nothing may hold it open.
        drop(std::mem::replace(&mut self.conn, Connection::open_in_memory()?));
        let start = crate::cloud::start(&path, true);
        match &start {
            crate::cloud::Start::Writer { .. } | crate::cloud::Start::Off => {
                self.conn = potok_core::db::open(&path)?;
                self.read_only = None;
                self.status = crate::cloud::start_note(&start).or_else(|| Some(trs!("writing here now", "теперь пишем здесь").into()));
            }
            _ => {
                self.conn = potok_core::db::open_read_only(&path)?;
                self.status = crate::cloud::start_note(&start);
            }
        }
        self.reload()
    }

    pub(super) fn reload_flows(&mut self) -> Result<()> {
        let (from, to) = self.period.bounds_on(self.today, &self.month_rule);
        self.flows = match self.period.previous_bounds_on(self.today, &self.month_rule) {
            Some((before_from, before_to)) => {
                query::compare_flows(&self.conn, (&from, &to), (&before_from, &before_to))?
            }
            // «All time» has nothing to be compared with.
            None => query::compare_flows(&self.conn, (&from, &to), ("9999-12-31", "9999-12-31"))?,
        };
        Ok(())
    }

    /// Rereads what the operations screen shows when drilled in or searching.
    pub(super) fn refresh_view(&mut self) -> Result<()> {
        match &self.ops_view {
            OpsView::All => {
                self.view_operations.clear();
                self.view_truncated = false;
            }
            OpsView::Import { id, .. } => {
                let found = query::search_operations(
                    &self.conn,
                    &query::OperationFilter { import_file: Some(*id), limit: usize::MAX, ..Default::default() },
                )?;
                self.view_operations = found.operations;
                self.view_truncated = found.truncated;
            }
            OpsView::Drill { label, from, to } => {
                self.view_operations = query::flow_operations(&self.conn, label, from, to)?;
                self.view_truncated = false;
            }
            OpsView::Search { text } => {
                let found = query::search_operations(
                    &self.conn,
                    &OperationFilter {
                        text: Some(text.clone()),
                        limit: SEARCH_LIMIT,
                        ..Default::default()
                    },
                )?;
                self.view_operations = found.operations;
                self.view_truncated = found.truncated;
            }
            OpsView::Account { id, .. } => {
                self.view_operations = query::operations(&self.conn, Some(*id), true)?;
                self.view_truncated = false;
            }
            OpsView::Counterparty { name } => {
                self.view_operations = query::search_operations(
                    &self.conn,
                    &OperationFilter {
                        counterparty: Some(name.clone()),
                        limit: usize::MAX,
                        ..Default::default()
                    },
                )?
                .operations;
                self.view_truncated = false;
            }
            OpsView::Range { from, to } => {
                self.view_operations = query::search_operations(
                    &self.conn,
                    &OperationFilter { from: Some(from.clone()), to: Some(to.clone()), limit: usize::MAX, ..Default::default() },
                )?
                .operations;
                self.view_truncated = false;
            }
        }
        if self.hide_transfers {
            self.view_operations.retain(|o| o.role != "transfer");
        }
        Ok(())
    }

    /// Rebuilds the details of the row under the cursor. Called after every
    /// key — the database is in this process, and two or three queries per
    /// press go unnoticed.
    pub fn refresh_detail(&mut self) -> Result<()> {
        if self.pinned {
            // A pinned operation is reread — it may have just been removed or
            // brought back — but it is not swapped for another.
            if let Detail::Operation(op) = &self.detail {
                let id = op.line.id;
                if let Some(fresh) = detail::operation(&self.conn, id)? {
                    self.detail = Detail::Operation(Box::new(fresh));
                }
            }
            return Ok(());
        }
        let at = self.cursor();
        self.detail = match self.screen {
            // A statement has no details: all that is known of it is in its row.
            Screen::Statements => None,
            Screen::Accounts => match self.accounts.get(at) {
                Some(account) => detail::account(&self.conn, account.id)?.map(Detail::Account),
                None => None,
            },
            Screen::Operations if self.fold => match self.groups().get(at) {
                Some(group) => detail::counterparty(&self.conn, &group.name)?.map(Detail::Counterparty),
                None => None,
            },
            Screen::Operations => match self.visible_operations().get(at) {
                Some(op) => detail::operation(&self.conn, op.id)?.map(|d| Detail::Operation(Box::new(d))),
                None => None,
            },
            Screen::Flows => match self.flows.get(at) {
                Some(row) => {
                    let (from, to) = self.flow_bounds(row.count);
                    Some(Detail::Flow(detail::flow(&self.conn, &row.label, &from, &to)?))
                }
                None => None,
            },
            Screen::Debts => match self.debts.get(at) {
                Some(debt) => detail::debt(&self.conn, debt.id)?.map(Detail::Debt),
                None => None,
            },
            Screen::Counterparties => match self.selected_counterparty() {
                Some(line) => {
                    let name = line.name.clone();
                    detail::counterparty(&self.conn, &name)?.map(Detail::Counterparty)
                }
                None => None,
            },
            Screen::Duplicates => self.duplicates.get(at).cloned().map(Detail::Duplicates),
            Screen::Journal => self.changes.get(at).cloned().map(Detail::Change),
            Screen::Settings => SettingKey::ALL.get(at).copied().map(Detail::Setting),
            Screen::Categories => self.category_lines.get(at).cloned().map(Detail::Category),
            Screen::Home => match self.home_flows().get(at) {
                Some(row) => {
                    let label = row.label.clone();
                    let (from, to) = self.home_bounds();
                    Some(Detail::Flow(detail::flow(&self.conn, &label, &from, &to)?))
                }
                None => None,
            },
            Screen::Analytics if self.analytics_pane == AnalyticsPane::Regular => {
                match self.regular.get(self.regular_cursor) {
                    Some(r) => detail::counterparty(&self.conn, &r.who)?.map(Detail::Counterparty),
                    None => None,
                }
            }
            Screen::Analytics => {
                let idx = self.month_cursor();
                match self.months.iter().rev().nth(idx) {
                    Some(m) => {
                        let (from, to) = self.month_bounds(m.start);
                        let prev_out = self
                            .months
                            .iter()
                            .rev()
                            .nth(idx + 1)
                            .and_then(|p| p.out_base);
                        let full: Vec<potok_core::Money> = self
                            .months
                            .iter()
                            .rev()
                            .skip(1)
                            .filter(|x| !x.out.is_empty() || !x.inn.is_empty())
                            .filter_map(|x| x.out_base)
                            .collect();
                        let avg_out = (!full.is_empty()).then(|| {
                            let cur = full[0].currency();
                            let sum: i64 = full.iter().map(|x| x.minor()).sum();
                            potok_core::Money::new((sum + full.len() as i64 / 2) / full.len() as i64, cur)
                        });
                        Some(Detail::Month {
                            month: m.clone(),
                            prev_out,
                            avg_out,
                            flows: query::flows(&self.conn, &from, &to)?,
                        })
                    }
                    None => None,
                }
            }
            Screen::Budget => match self.budget_lines.get(at) {
                Some(line) => Some(Detail::Budget {
                    line: line.clone(),
                    trend: potok_core::analytics::trend(&self.conn, &line.category, self.today, 6)?,
                }),
                None => None,
            },
        }
        .unwrap_or(Detail::Nothing);
        if self.screen == Screen::Home {
            self.sync_home_month()?;
        }
        // On the Overview the right pane is a list of its own rather than the
        // row's details: its cursor is not clipped by empty details and keeps focus.
        let items = if self.screen == Screen::Home { self.home_right_len() } else { self.detail_items().len() };
        self.detail_cursor = self.detail_cursor.min(items.saturating_sub(1));
        if items == 0 {
            self.focus = Focus::List;
        }
        Ok(())
    }

    /// The detail rows one can stand on.
    pub fn detail_items(&self) -> Vec<DetailItem> {
        match &self.detail {
            Detail::Debt(d) => d
                .movements
                .iter()
                .map(|m| DetailItem::Movement { id: m.id, operation: m.operation_id })
                .collect(),
            Detail::Account(d) => d.recent.iter().map(|o| DetailItem::Operation(o.id)).collect(),
            // The spellings first, then the operations — in the same order they
            // are drawn, or the cursor lands on the wrong thing.
            Detail::Counterparty(d) => d
                .aliases
                .iter()
                .map(|a| DetailItem::Alias(a.clone()))
                .chain(d.recent.iter().map(|o| DetailItem::Operation(o.id)))
                .collect(),
            Detail::Flow(d) => d.recent.iter().map(|o| DetailItem::Operation(o.id)).collect(),
            Detail::Duplicates(g) => g.operations.iter().map(|o| DetailItem::Operation(o.id)).collect(),
            Detail::Month { month, flows, .. } => {
                let (from, to) = self.month_bounds(month.start);
                flows
                    .iter()
                    .filter(|f| !f.out.is_empty())
                    .chain(flows.iter().filter(|f| !f.inn.is_empty()))
                    .map(|f| DetailItem::MonthFlow {
                        label: f.label.clone(),
                        from: from.clone(),
                        to: to.clone(),
                    })
                    .collect()
            }
            Detail::Operation(_)
            | Detail::Change(_)
            | Detail::Setting(_)
            | Detail::Category(_)
            | Detail::Budget { .. }
            | Detail::Nothing => {
                Vec::new()
            }
        }
    }

    /// How far the bars have grown: 0 — only begun, 1 — grown. Slowing towards
    /// the end (ease-out), or the growth looks mechanical.
    pub fn growth(&self) -> f64 {
        const MS: f64 = 320.0;
        match self.grew_at {
            None => 1.0,
            Some(since) => {
                let t = (since.elapsed().as_secs_f64() * 1000.0 / MS).clamp(0.0, 1.0);
                1.0 - (1.0 - t).powi(3)
            }
        }
    }

    /// Open one operation whole — over everything, until Esc.
    pub fn open_operation(&mut self, id: i64) -> Result<()> {
        if let Some(op) = detail::operation(&self.conn, id)? {
            self.detail = Detail::Operation(Box::new(op));
            self.pinned = true;
            self.zoom = true;
        }
        Ok(())
    }

    /// The operation being stood on now: pinned, in the details or in the
    /// journal.
    pub(super) fn focused_operation(&self) -> Option<i64> {
        if self.pinned {
            if let Detail::Operation(op) = &self.detail {
                return Some(op.line.id);
            }
        }
        if self.focus == Focus::Detail {
            return match self.detail_items().get(self.detail_cursor) {
                Some(DetailItem::Operation(id)) => Some(*id),
                Some(DetailItem::Movement { operation, .. }) => *operation,
                Some(DetailItem::MonthFlow { .. }) | Some(DetailItem::Alias(_)) | None => None,
            };
        }
        if self.screen == Screen::Operations && !self.fold {
            return self.visible_operations().get(self.cursor()).map(|op| op.id);
        }
        None
    }

    /// A refusal in words: SQLite's own text is not for the person.
    pub fn explain(&self, error: &anyhow::Error) -> String {
        let text = format!("{error:#}");
        if text.contains("readonly database") {
            if let Some(note) = self.read_only_note() {
                return note;
            }
        }
        if text.contains("database is locked") {
            return trs!(
                "not done: the ledger is busy in another window or command — try again",
                "не сделано: леджер занят в другом окне или команде — попробуй ещё раз"
            )
            .into();
        }
        tr!("not done: {error}", "не сделано: {error}")
    }

}

/// A fuzzy match: the place in the output, or `None` if it does not fit.
///
/// 0 — the name's start, 1 — a word's start, 2 — a substring, 3 — the letters
/// in order («fg» → «Food & Groceries»). Empty typing fits everything alike.
pub fn fuzzy_rank(typed: &str, name: &str) -> Option<u8> {
    if typed.is_empty() {
        return Some(0);
    }
    let typed = typed.to_lowercase();
    let name = name.to_lowercase();
    if name.starts_with(&typed) {
        return Some(0);
    }
    if name.split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(&typed)) {
        return Some(1);
    }
    if name.contains(&typed) {
        return Some(2);
    }
    let mut letters = name.chars();
    typed
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|wanted| letters.any(|c| c == wanted))
        .then_some(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    pub(super) fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    /// A period's bounds are counted from the date passed in, or the test would
    /// depend on the day it ran — and that is the kind of test that goes red on
    /// New Year's Eve.
    #[test]
    pub(super) fn a_period_knows_its_own_bounds() {
        let today = day("2026-10-16");
        assert_eq!(
            Period::ThisMonth.bounds(today),
            ("2026-10-01".to_string(), "2026-10-16".to_string())
        );
        assert_eq!(
            Period::PreviousMonth.bounds(today),
            ("2026-09-01".to_string(), "2026-09-30".to_string())
        );
        assert_eq!(
            Period::ThisYear.bounds(today),
            ("2026-01-01".to_string(), "2026-10-16".to_string())
        );
    }

    /// January: the previous month is last December, not month number zero.
    #[test]
    pub(super) fn the_previous_month_crosses_the_year() {
        assert_eq!(
            Period::PreviousMonth.bounds(day("2026-01-07")),
            ("2025-12-01".to_string(), "2025-12-31".to_string())
        );
    }

    /// The comparison is with a span of the same length: eighteen days with eighteen.
    #[test]
    pub(super) fn a_period_is_compared_with_one_of_the_same_length() {
        let today = day("2026-10-18");
        assert_eq!(
            Period::ThisMonth.previous_bounds(today),
            Some(("2026-09-01".to_string(), "2026-09-18".to_string()))
        );
        assert_eq!(
            Period::PreviousMonth.previous_bounds(today),
            Some(("2026-08-01".to_string(), "2026-08-31".to_string()))
        );
        assert_eq!(
            Period::ThisYear.previous_bounds(today),
            Some(("2025-01-01".to_string(), "2025-10-18".to_string()))
        );
        assert_eq!(Period::Everything.previous_bounds(today), None);
    }

    /// 31 March: February has no thirty-first — the bound is its last day.
    /// 29 February of a leap year: last year has none — the 28th.
    #[test]
    pub(super) fn the_comparison_survives_short_months_and_leap_days() {
        assert_eq!(
            Period::ThisMonth.previous_bounds(day("2026-03-31")),
            Some(("2026-02-01".to_string(), "2026-02-28".to_string()))
        );
        assert_eq!(
            Period::ThisYear.previous_bounds(day("2028-02-29")),
            Some(("2027-01-01".to_string(), "2027-02-28".to_string()))
        );
    }

    /// The first version held 500 operations and promised a loading that was not
    /// there: on the real ledger of 1774 rows the older 1274 could not be reached.
    #[test]
    pub(super) fn the_oldest_operation_is_reachable_however_long_the_journal() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind) values ('A','EUR','BANK');")
            .unwrap();
        for day in 0..600 {
            let at = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Days::new(day);
            conn.execute(
                "insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description)
                 values (1,'expense',?1,100,'EUR',-100,'x')",
                [format!("{at}T00:00:00Z")],
            )
            .unwrap();
        }
        let mut app = App::new(conn).unwrap();
        assert_eq!(app.operations.len(), 600, "the whole journal, not the first page");
        app.screen = Screen::Operations;
        app.go_last();
        assert_eq!(app.cursor(), 599);
        assert_eq!(app.operations[599].occurred_on, "2025-01-01", "the oldest at the end");
    }

    /// February as the previous month of 31 March: February has no thirty-first,
    /// and the bound must be its last day.
    #[test]
    pub(super) fn a_short_previous_month_ends_on_its_own_last_day() {
        assert_eq!(
            Period::PreviousMonth.bounds(day("2026-03-31")),
            ("2026-02-01".to_string(), "2026-02-28".to_string())
        );
    }

    #[test]
    pub(super) fn fuzzy_ranking_prefers_what_a_person_means() {
        assert_eq!(fuzzy_rank("food", "Food & Groceries"), Some(0));
        assert_eq!(fuzzy_rank("groc", "Food & Groceries"), Some(1), "a word's start");
        assert_eq!(fuzzy_rank("ocer", "Food & Groceries"), Some(2), "a substring");
        assert_eq!(fuzzy_rank("fg", "Food & Groceries"), Some(3), "the letters in order");
        assert_eq!(fuzzy_rank("гро", "Продукты"), None);
        assert_eq!(fuzzy_rank("ПРОД", "Продукты"), Some(0), "case in any alphabet");
    }

    pub(super) fn ledger() -> App {
        let conn = potok_core::db::open_memory().unwrap();
        let today = chrono::Local::now().date_naive();
        let this_month = today.with_day(1).unwrap();
        conn.execute_batch(&format!(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');
             insert into category (name) values ('Food & Groceries'), ('Household');
             insert into counterparty (name) values ('Kaufland'), ('Еконт');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,'expense','{d}T00:00:00Z',624,'EUR',-624,'Kaufland',1,1),
               (1,'expense','{d}T00:00:00Z',3115,'EUR',-3115,'Kaufland',1,1),
               (1,'expense','{d}T00:00:00Z',807,'EUR',-807,'Kaufland',1,null),
               (1,'expense','{d}T00:00:00Z',350,'EUR',-350,'Еконт',2,null);",
            d = this_month
        ))
        .unwrap();
        App::new(conn).unwrap()
    }

    /// Drilling in and Esc back: where one came from.
    #[test]
    pub(super) fn drilling_in_shows_the_rows_of_the_flow_and_esc_comes_back() {
        let mut app = ledger();
        app.screen = Screen::Flows;
        let row = app.flows.iter().position(|f| f.label == "Food & Groceries").unwrap();
        app.cursors[Screen::Flows as usize] = row;
        app.drill().unwrap();
        assert_eq!(app.screen, Screen::Operations);
        assert_eq!(app.visible_operations().len(), app.flows[row].count);
        app.back().unwrap();
        assert_eq!(app.screen, Screen::Flows, "Esc returns to the flows");
        assert_eq!(app.ops_view, OpsView::All);
    }

    /// The details follow the cursor — what the pane was made for.
    #[test]
    pub(super) fn the_detail_follows_the_cursor() {
        let mut app = ledger();
        app.screen = Screen::Operations;
        app.refresh_detail().unwrap();
        let Detail::Operation(first) = &app.detail else { panic!("{:?}", app.detail) };
        let first = first.line.id;
        app.move_cursor(1);
        app.refresh_detail().unwrap();
        let Detail::Operation(second) = &app.detail else { panic!("{:?}", app.detail) };
        assert_ne!(first, second.line.id);
    }

    /// Enter on an account and on a counterparty — their operations; Esc back.
    #[test]
    pub(super) fn enter_goes_deeper_and_esc_comes_back_where_it_started() {
        let mut app = ledger();
        app.screen = Screen::Accounts;
        app.open().unwrap();
        assert_eq!(app.screen, Screen::Operations);
        assert!(matches!(app.ops_view, OpsView::Account { .. }));
        app.back().unwrap();
        assert_eq!(app.screen, Screen::Accounts);

        app.screen = Screen::Counterparties;
        let at = app.visible_counterparties().iter().position(|l| l.name == "Kaufland").unwrap();
        app.cursors[Screen::Counterparties as usize] = at;
        app.open().unwrap();
        assert_eq!(app.visible_operations().len(), 3, "every Kaufland operation");
        app.back().unwrap();
        assert_eq!(app.screen, Screen::Counterparties);
    }

    /// Where there is no deeper, Enter opens the details, Esc folds them.
    #[test]
    pub(super) fn enter_on_an_operation_zooms_and_esc_unzooms() {
        let mut app = ledger();
        app.screen = Screen::Operations;
        app.open().unwrap();
        assert!(app.zoom);
        app.back().unwrap();
        assert!(!app.zoom);
        assert_eq!(app.screen, Screen::Operations, "Esc from the opened details does not leave the screen");
    }

    /// The search updates on every letter and finds in any alphabet and by amount.
    #[test]
    pub(super) fn a_search_narrows_as_it_is_typed() {
        let mut app = ledger();
        app.start_search().unwrap();
        assert_eq!(app.screen, Screen::Operations);
        for c in "еко".chars() {
            app.input.push(c);
            app.apply_search().unwrap();
        }
        assert_eq!(app.visible_operations().len(), 1);
        app.input = "31.15".into();
        app.apply_search().unwrap();
        assert_eq!(app.visible_operations().len(), 1, "by the amount on the receipt");
        app.cancel_search().unwrap();
        assert_eq!(app.visible_operations().len(), 4, "cancelling gives the whole journal");
    }

    /// On the import screen `/` searches counterparties, not operations.
    #[test]
    pub(super) fn on_the_import_screen_search_filters_counterparties() {
        let mut app = ledger();
        app.screen = Screen::Counterparties;
        app.start_search().unwrap();
        assert_eq!(app.screen, Screen::Counterparties);
        app.input = "KAUF".into();
        app.apply_search().unwrap();
        let names: Vec<_> = app.visible_counterparties().iter().map(|l| l.name.clone()).collect();
        assert_eq!(names, ["Kaufland"]);
        app.mode = Mode::Normal;
        app.back().unwrap();
        assert_eq!(app.visible_counterparties().len(), 2, "Esc clears the filter");
    }

    /// What it is all for: the hint first, and one Enter sees it through.
    #[test]
    pub(super) fn the_hint_comes_first_and_one_enter_finishes_the_counterparty() {
        let mut app = ledger();
        app.screen = Screen::Counterparties;
        let at = app.visible_counterparties().iter().position(|l| l.name == "Kaufland").unwrap();
        app.cursors[Screen::Counterparties as usize] = at;
        app.start_pick();
        let picks = app.picks();
        assert_eq!(picks[0], Pick::Hint { category: "Food & Groceries".into(), count: 2, of: 3 });
        assert!(!picks[1..].iter().any(|p| p.category() == "Food & Groceries"), "with no repeat");
        app.apply_pick().unwrap();
        let kaufland = app.counterparties.iter().find(|l| l.name == "Kaufland").unwrap();
        assert_eq!(kaufland.category.as_deref(), Some("Food & Groceries"));
        let flow = app.flows.iter().find(|f| f.label == "Food & Groceries").unwrap();
        assert_eq!(flow.count, 3, "and the one that had no category");
    }

    /// Typing narrows the list, and an unknown name is offered to be created.
    #[test]
    pub(super) fn typing_narrows_the_list_and_offers_to_create() {
        let mut app = ledger();
        app.screen = Screen::Counterparties;
        app.start_pick();
        app.input = "house".into();
        assert_eq!(app.picks(), vec![Pick::Existing("Household".into()), Pick::Create("house".into())]);
        app.input = "Household".into();
        assert_eq!(app.picks(), vec![Pick::Existing("Household".into())], "an exact match offers no «create»");
        app.input = "Доставка".into();
        assert_eq!(app.picks(), vec![Pick::Create("Доставка".into())]);
    }
}
