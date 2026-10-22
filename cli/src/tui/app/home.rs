//! The Overview screen: three lists and what Enter does on each.
use super::*;

impl App {
    /// The «where it went» rows on the Overview: only those something left by.
    /// The cursor, the details, drilling in and the drawing all take them from
    /// here — else an income row shifts the numbering and the cursor misses.
    pub fn home_flows(&self) -> Vec<potok_core::query::Flow> {
        let rows = match &self.home_past {
            Some(past) => past.flows.clone(),
            None => self.overview.as_ref().map(|o| o.where_it_went.clone()).unwrap_or_default(),
        };
        rows.into_iter().filter(|f| !f.out.is_empty()).collect()
    }

    /// Keeps «where it went» in agreement with the column under the cursor.
    /// Computed after a key press, not on every draw: it is a query.
    pub fn sync_home_month(&mut self) -> Result<()> {
        let months = self.home_months();
        let at = self.home_month_at();
        let current = at + 1 >= months.len();
        if current {
            self.home_past = None;
            return Ok(());
        }
        if self.home_past.as_ref().is_some_and(|past| past.at == at) {
            return Ok(());
        }
        let Some(month) = months.get(at) else { return Ok(()) };
        let (from, to) = self.month_bounds(month.start);
        self.home_past = Some(PastMonth { at, flows: potok_core::query::flows(&self.conn, &from, &to)?, from, to });
        Ok(())
    }

    /// Which column is under the cursor: the chosen one or, until one is chosen, this month.
    pub fn home_month_at(&self) -> usize {
        self.home_month_cursor.unwrap_or_else(|| self.home_months().len().saturating_sub(1))
    }

    /// The months as columns: from the first one something happened in up to this one.
    /// The drawing takes this same list, so a column's index and the index
    /// under the cursor are one and the same.
    pub fn home_months(&self) -> Vec<potok_core::analytics::Month> {
        self.months.iter().skip_while(|m| m.out.is_empty() && m.inn.is_empty()).cloned().collect()
    }

    /// «Look at»: everything awaiting a decision, in one stack. Computed here
    /// rather than at the drawing: the cursor stands on these rows and Enter works.
    pub fn home_look(&self) -> Vec<LookRow> {
        let mut look = Vec::new();
        let Some(o) = &self.overview else { return look };
        let row = |text: String, keys: &'static str, go: HomeGo| LookRow { text, keys, go };
        // An empty ledger is not «all is well»: it is empty, and where to start has to be said.
        if self.accounts.is_empty() {
            look.push(row(trs!("no accounts yet — create one", "счетов ещё нет — создай").into(), "2 · x", HomeGo::Accounts));
            look.push(row(trs!("then load a statement", "потом загрузи выписку").into(), "I · i", HomeGo::Import));
        }
        if let Some(first) = o.unreconciled.first() {
            look.push(row(tr!("not reconciled: {}", "не сверены: {}", o.unreconciled.join(", ")), "2 · s", HomeGo::Reconcile(first.clone())));
        }
        if o.duplicates > 0 {
            look.push(row(tr!("{} look-alike groups", "{} групп похожих", o.duplicates), "0", HomeGo::Duplicates));
        }
        let uncategorised = self.counterparties.iter().filter(|c| c.category.is_none() && c.operations > 0).count();
        if uncategorised > 0 {
            look.push(row(
                tr!("{uncategorised} counterparties without a category", "{uncategorised} контрагентов без категории"),
                "8 · C",
                HomeGo::Categorise,
            ));
        }
        if o.refunds_as_income > 0 {
            look.push(row(
                tr!("{} refunds counted as income", "{} возвратов считаются доходом", o.refunds_as_income),
                "8 · x",
                HomeGo::Refunds,
            ));
        }
        for (bank, day) in &o.imports {
            let age = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| (self.today - d).num_days()).unwrap_or(0);
            if age > 31 {
                look.push(row(
                    tr!("{bank}: last statement {age} days ago", "{bank}: последняя выписка {age} дней назад"),
                    "I · i",
                    HomeGo::Import,
                ));
            }
        }
        if let Some(error) = &self.cloud_error {
            look.push(row(tr!("copy to Google Drive failed: {error}", "копия в Google Drive не ушла: {error}"), ", ", HomeGo::Settings));
        }
        if let Some(since) = o.stale_rates {
            let days = (self.today - since).num_days();
            look.push(row(tr!("exchange rates {days} days old", "курсам валют {days} дн."), ", ", HomeGo::Settings));
        }
        look
    }

    /// The debts with an outstanding remainder — the right pane's lower part.
    pub fn home_debts(&self) -> Vec<potok_core::query::DebtLine> {
        self.debts.iter().filter(|d| d.remaining.minor() != 0).cloned().collect()
    }

    /// How many rows the Overview's right pane has — ahead, look at, debts, in
    /// that order.
    pub fn home_right_len(&self) -> usize {
        let upcoming = self.overview.as_ref().map_or(0, |o| o.upcoming.len());
        upcoming + self.home_look().len() + self.home_debts().len()
    }

    /// Where the right pane's row number `at` leads. The one place where the
    /// order of the rows becomes numbers: the drawing takes the same three lists.
    pub fn home_right_go(&self, at: usize) -> Option<HomeGo> {
        let upcoming: Vec<String> = self.overview.as_ref().map(|o| o.upcoming.iter().map(|r| r.who.clone()).collect()).unwrap_or_default();
        if let Some(who) = upcoming.get(at) {
            return Some(HomeGo::Counterparty(who.clone()));
        }
        let look = self.home_look();
        if let Some(row) = look.get(at - upcoming.len()) {
            return Some(row.go.clone());
        }
        self.home_debts().get(at - upcoming.len() - look.len()).map(|d| HomeGo::Debt(d.id))
    }

    /// Enter on a right-pane row: does what the row says.
    pub fn go_home_row(&mut self, go: HomeGo) -> Result<()> {
        match go {
            HomeGo::Counterparty(name) => {
                self.ops_view = OpsView::Counterparty { name };
                self.refresh_view()?;
                self.return_to = Some(Screen::Home);
                self.go_to(Screen::Operations);
                self.cursors[Screen::Operations as usize] = 0;
            }
            HomeGo::Debt(id) => {
                let at = self.debts.iter().position(|d| d.id == id).unwrap_or(0);
                self.go_to(Screen::Debts);
                self.cursors[Screen::Debts as usize] = at;
            }
            HomeGo::Reconcile(account) => {
                let at = self.accounts.iter().position(|a| a.name == account).unwrap_or(0);
                self.go_to(Screen::Accounts);
                self.cursors[Screen::Accounts as usize] = at;
                self.refresh_detail()?;
                if self.read_only.is_some() {
                    self.status = self.read_only_note();
                } else {
                    self.start_reconcile();
                }
                return Ok(());
            }
            HomeGo::Duplicates => self.go_to(Screen::Duplicates),
            HomeGo::Categorise => {
                self.go_to(Screen::Counterparties);
                if self.read_only.is_some() {
                    self.status = self.read_only_note();
                } else {
                    self.start_queue();
                }
                return Ok(());
            }
            HomeGo::Refunds => {
                if self.read_only.is_some() {
                    self.status = self.read_only_note();
                    return Ok(());
                }
                let refunds = potok_core::counterparty::refunds_counted_as_income(&self.conn)?.len();
                self.go_to(Screen::Counterparties);
                self.pending = Some(MenuItem {
                    label: String::new(),
                    action: MenuAction::TakeRefunds,
                    confirm: Some(tr!(
                        "Count {refunds} refunds against their shop's spending? Each is a line in Changes and can be undone",
                        "Вычесть {refunds} возвратов из трат их магазинов? Каждый — строка в «Правках», отменяется"
                    )),
                });
                self.mode = Mode::Confirm;
                return Ok(());
            }
            HomeGo::Import => self.go_to(Screen::Statements),
            HomeGo::Accounts => self.go_to(Screen::Accounts),
            HomeGo::Settings => self.go_to(Screen::Settings),
        }
        self.refresh_detail()
    }

    /// The «where it went» window on the Overview: the accounting month from
    /// its start to today — exactly what `analytics::overview` counts it by.
    pub fn home_bounds(&self) -> (String, String) {
        if let Some(past) = &self.home_past {
            return (past.from.clone(), past.to.clone());
        }
        let cycle = self.month_rule.cycle(self.today, 0);
        (cycle.start.to_string(), self.today.to_string())
    }

}
