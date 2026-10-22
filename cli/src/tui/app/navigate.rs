//! The cursor, the screens, the panes: walking the lists and drilling in.
use super::*;

impl App {
    /// `Tab` / `Shift+Tab`: moving between the panes. The analytics has three
    /// (Months, Regular payments, Details on the right); on the other screens —
    /// the list and the details.
    pub fn switch_pane(&mut self, forward: bool) -> Result<()> {
        if self.screen == Screen::Home {
            // «where it went» → the months → the right stack → back. An empty
            // list is skipped: Tab must not take the cursor nowhere.
            // One column is no chart: it is not drawn, and there is nothing there.
            let months = self.home_months().len() > 1;
            let right = self.home_right_len() > 0;
            let order = [(Focus::List, HomePane::Flows), (Focus::List, HomePane::Months), (Focus::Detail, HomePane::Flows)];
            let here = order.iter().position(|(f, p)| *f == self.focus && (self.focus == Focus::Detail || *p == self.home_pane)).unwrap_or(0);
            for step in 1..=order.len() {
                let next = if forward { (here + step) % order.len() } else { (here + order.len() - step) % order.len() };
                let (focus, pane) = order[next];
                let usable = match (focus, pane) {
                    (Focus::Detail, _) => right,
                    (_, HomePane::Months) => months,
                    (_, HomePane::Flows) => self.rows_on(Screen::Home) > 0,
                };
                if usable {
                    self.focus = focus;
                    if focus == Focus::List {
                        self.home_pane = pane;
                    }
                    break;
                }
            }
            return Ok(());
        }
        if self.screen == Screen::Analytics {
            let has_regular = !self.regular.is_empty();
            // Build the sequence of panes on Analytics:
            // Months -> Month Detail (if non-empty) -> Regular (if non-empty) -> Regular Detail
            match (self.focus, self.analytics_pane, forward) {
                (Focus::List, AnalyticsPane::Months, true) => {
                    if !self.detail_items().is_empty() {
                        self.focus = Focus::Detail;
                    } else if has_regular {
                        self.analytics_pane = AnalyticsPane::Regular;
                        self.detail_cursor = 0;
                        self.refresh_detail()?;
                    }
                }
                (Focus::Detail, AnalyticsPane::Months, true) => {
                    self.focus = Focus::List;
                    if has_regular {
                        self.analytics_pane = AnalyticsPane::Regular;
                        self.detail_cursor = 0;
                        self.refresh_detail()?;
                    }
                }
                (Focus::List, AnalyticsPane::Regular, true) => {
                    if !self.detail_items().is_empty() {
                        self.focus = Focus::Detail;
                    } else {
                        self.analytics_pane = AnalyticsPane::Months;
                        self.detail_cursor = 0;
                        self.refresh_detail()?;
                    }
                }
                (Focus::Detail, AnalyticsPane::Regular, true) => {
                    self.focus = Focus::List;
                    self.analytics_pane = AnalyticsPane::Months;
                    self.detail_cursor = 0;
                    self.refresh_detail()?;
                }
                (Focus::List, AnalyticsPane::Months, false) => {
                    if has_regular {
                        self.analytics_pane = AnalyticsPane::Regular;
                        self.detail_cursor = 0;
                        self.refresh_detail()?;
                        if !self.detail_items().is_empty() {
                            self.focus = Focus::Detail;
                        }
                    } else if !self.detail_items().is_empty() {
                        self.focus = Focus::Detail;
                    }
                }
                (Focus::Detail, AnalyticsPane::Months, false) => {
                    self.focus = Focus::List;
                }
                (Focus::List, AnalyticsPane::Regular, false) => {
                    self.analytics_pane = AnalyticsPane::Months;
                    self.detail_cursor = 0;
                    self.refresh_detail()?;
                    if !self.detail_items().is_empty() {
                        self.focus = Focus::Detail;
                    }
                }
                (Focus::Detail, AnalyticsPane::Regular, false) => {
                    self.focus = Focus::List;
                }
            }
            return Ok(());
        }
        match self.focus {
            Focus::List => self.focus_detail(),
            Focus::Detail => self.focus_list(),
        }
        Ok(())
    }

    /// `→`: into the details, if there is something to stand on there.
    pub fn focus_detail(&mut self) {
        // On the Overview the second pane is not the details but the right
        // stack: what is ahead, what to look at, the debts. It walks the same.
        let has_rows = if self.screen == Screen::Home { self.home_right_len() > 0 } else { !self.detail_items().is_empty() };
        if has_rows {
            self.focus = Focus::Detail;
        }
    }

    pub fn focus_list(&mut self) {
        self.focus = Focus::List;
    }

    pub fn move_detail_cursor(&mut self, by: isize) {
        let count = if self.screen == Screen::Home { self.home_right_len() } else { self.detail_items().len() };
        if count > 0 {
            self.detail_cursor = (self.detail_cursor as isize + by).clamp(0, count as isize - 1) as usize;
        }
    }

    pub fn rows_on(&self, screen: Screen) -> usize {
        match screen {
            Screen::Accounts => self.accounts.len(),
            Screen::Operations if self.fold => self.groups().len(),
            Screen::Operations => self.visible_operations().len(),
            Screen::Flows => self.flows.len(),
            Screen::Debts => self.debts.len(),
            // On the import screen the cursor walks the counterparties: marking
            // is the work done here, and the file journal is only above it.
            Screen::Counterparties => self.visible_counterparties().len(),
            Screen::Statements => self.imports.len(),
            Screen::Duplicates => self.duplicates.len(),
            Screen::Journal => self.changes.len(),
            Screen::Settings => SettingKey::ALL.len(),
            Screen::Categories => self.category_lines.len(),
            // «Where it went» is a list the cursor walks.
            Screen::Home => self.home_flows().len(),
            Screen::Analytics if self.analytics_pane == AnalyticsPane::Regular => self.regular.len(),
            // Newest first: the month that matters is the one at the top.
            Screen::Analytics => self.months.len(),
            Screen::Budget => self.budget_lines.len(),
        }
    }

    pub fn month_cursor(&self) -> usize {
        self.cursors[Screen::Analytics as usize]
    }

    pub fn cursor(&self) -> usize {
        if self.screen == Screen::Analytics && self.analytics_pane == AnalyticsPane::Regular {
            return self.regular_cursor;
        }
        self.cursors[self.screen as usize]
    }

    #[cfg(test)]
    pub fn cursors_mut(&mut self) -> &mut [usize; Screen::COUNT] {
        &mut self.cursors
    }

    pub(super) fn set_cursor(&mut self, at: usize) {
        let last = self.rows_on(self.screen).saturating_sub(1);
        if self.screen == Screen::Analytics && self.analytics_pane == AnalyticsPane::Regular {
            self.regular_cursor = at.min(last);
        } else {
            self.cursors[self.screen as usize] = at.min(last);
        }
    }

    pub(super) fn clamp_cursors(&mut self) {
        for screen in Screen::ALL {
            let last = match screen {
                Screen::Analytics => self.months.len().saturating_sub(1),
                other => self.rows_on(other).saturating_sub(1),
            };
            let cursor = &mut self.cursors[screen as usize];
            *cursor = (*cursor).min(last);
        }
        self.regular_cursor = self.regular_cursor.min(self.regular.len().saturating_sub(1));
        if self.regular.is_empty() {
            self.analytics_pane = AnalyticsPane::Months;
        }
    }

    pub fn move_cursor(&mut self, by: isize) {
        if self.screen == Screen::Home && self.home_pane == HomePane::Months && self.focus == Focus::List {
            let months = self.home_months().len();
            if months > 0 {
                self.home_month_cursor = Some((self.home_month_at() as isize + by).clamp(0, months as isize - 1) as usize);
            }
            return;
        }
        let rows = self.rows_on(self.screen);
        if rows == 0 {
            return;
        }
        let at = self.cursor() as isize + by;
        self.set_cursor(at.clamp(0, rows as isize - 1) as usize);
    }

    pub fn go_first(&mut self) {
        self.set_cursor(0);
    }

    pub fn go_last(&mut self) {
        self.set_cursor(self.rows_on(self.screen).saturating_sub(1));
    }

    pub fn next_screen(&mut self, by: isize) {
        let count = Screen::COUNT as isize;
        let at = (self.screen as isize + by).rem_euclid(count);
        self.go_to(Screen::ALL[at as usize]);
    }

    /// To another screen — with no window or focus left over from the last one.
    pub fn go_to(&mut self, screen: Screen) {
        self.zoom = false;
        self.pinned = false;
        self.focus = Focus::List;
        self.screen = screen;
    }

    /// Walks the account filter: every account, then each in turn. It returns to
    /// the whole journal — an account is chosen in the journal, not in a search.
    pub fn cycle_account(&mut self) -> Result<()> {
        let at = match self.account_filter {
            None => self.account_choices.first().map(|choice| choice.id),
            Some(current) => {
                let index = self.account_choices.iter().position(|c| c.id == current);
                match index {
                    Some(index) if index + 1 < self.account_choices.len() => {
                        Some(self.account_choices[index + 1].id)
                    }
                    _ => None,
                }
            }
        };
        self.account_filter = at;
        self.ops_view = OpsView::All;
        self.load_operations()?;
        self.refresh_view()?;
        self.apply_order();
        self.cursors[Screen::Operations as usize] = 0;
        Ok(())
    }

    pub fn cycle_period(&mut self) -> Result<()> {
        let at = Period::ALL.iter().position(|p| *p == self.period).unwrap_or(0);
        self.period = Period::ALL[(at + 1) % Period::ALL.len()];
        self.reload_flows()?;
        self.cursors[Screen::Flows as usize] = 0;
        Ok(())
    }

    pub fn account_filter_label(&self) -> &str {
        match self.account_filter {
            None => trs!("all accounts", "все счета"),
            Some(id) => self
                .account_choices
                .iter()
                .find(|choice| choice.id == id)
                .map(|choice| choice.name.as_str())
                .unwrap_or(trs!("all accounts", "все счета")),
        }
    }

    // ── 1. Drilling in ──────────────────────────────────────────────────────

    /// A flow row's period: this one, and for a gone row the last one it was in.
    pub(super) fn flow_bounds(&self, count: usize) -> (String, String) {
        if count == 0 {
            if let Some(bounds) = self.period.previous_bounds_on(self.today, &self.month_rule) {
                return bounds;
            }
        }
        self.period.bounds_on(self.today, &self.month_rule)
    }

    /// The accounting month starting on `start`, to its last day or to today.
    pub fn month_bounds(&self, start: chrono::NaiveDate) -> (String, String) {
        let end = self.month_rule.cycle(start, 0).end.min(self.today);
        (start.to_string(), end.to_string())
    }

    /// Enter on a flow row: its operations. A gone row — one with nothing in
    /// this period — opens the last period it was in.
    pub fn drill(&mut self) -> Result<()> {
        // A month column on the Overview: its operations, as on «Analytics».
        if self.screen == Screen::Home && self.focus == Focus::List && self.home_pane == HomePane::Months {
            let Some(m) = self.home_months().get(self.home_month_at()).cloned() else { return Ok(()) };
            let (from, to) = self.month_bounds(m.start);
            self.ops_view = OpsView::Range { from, to };
            self.refresh_view()?;
            self.return_to = Some(Screen::Home);
            self.go_to(Screen::Operations);
            self.cursors[Screen::Operations as usize] = 0;
            return Ok(());
        }
        let from_home = self.screen == Screen::Home;
        // Two lists of one kind: a «Flows» row knows the previous period too, a
        // «where it went» row only this one. The name and the window are all.
        let found = if from_home {
            self.home_flows().get(self.cursor()).map(|row| (row.label.clone(), self.home_bounds()))
        } else {
            self.flows.get(self.cursors[Screen::Flows as usize]).map(|row| (row.label.clone(), self.flow_bounds(row.count)))
        };
        let Some((label, (from, to))) = found else { return Ok(()) };
        self.ops_view = OpsView::Drill { label, from, to };
        self.refresh_view()?;
        self.return_to = Some(if from_home { Screen::Home } else { Screen::Flows });
        self.focus = Focus::List;
        self.screen = Screen::Operations;
        self.cursors[Screen::Operations as usize] = 0;
        Ok(())
    }

    /// Enter: deeper, as in lazygit. Where there is no deeper — the details over
    /// the whole screen; Enter again folds them.
    pub fn open(&mut self) -> Result<()> {
        if self.screen == Screen::Home && self.focus == Focus::Detail {
            return match self.home_right_go(self.detail_cursor) {
                Some(go) => self.go_home_row(go),
                None => Ok(()),
            };
        }
        if self.focus == Focus::Detail {
            match self.detail_items().get(self.detail_cursor).cloned() {
                Some(DetailItem::Operation(id)) => return self.open_operation(id),
                Some(DetailItem::Movement { operation: Some(id), .. }) => return self.open_operation(id),
                Some(DetailItem::Alias(_)) => {
                    if let Detail::Counterparty(d) = &self.detail {
                        let name = d.name.clone();
                        self.ops_view = OpsView::Counterparty { name };
                        self.refresh_view()?;
                        self.return_to = Some(self.screen);
                        self.focus = Focus::List;
                        self.screen = Screen::Operations;
                        self.cursors[Screen::Operations as usize] = 0;
                    }
                    return Ok(());
                }
                Some(DetailItem::MonthFlow { label, from, to }) => {
                    self.ops_view = OpsView::Drill { label, from, to };
                    self.refresh_view()?;
                    self.return_to = Some(self.screen);
                    self.focus = Focus::List;
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        match self.screen {
            Screen::Settings => return self.change_setting(),
            Screen::Home => return self.drill(),
            // Enter on a statement: what it brought. Esc returns to the list.
            Screen::Statements => {
                if let Some(file) = self.imports.get(self.cursor()) {
                    self.ops_view = OpsView::Import { id: file.id, name: file.filename.clone() };
                    self.refresh_view()?;
                    self.return_to = Some(Screen::Statements);
                    self.go_to(Screen::Operations);
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Analytics if self.analytics_pane == AnalyticsPane::Regular => {
                if let Some(r) = self.regular.get(self.regular_cursor) {
                    self.ops_view = OpsView::Counterparty { name: r.who.clone() };
                    self.refresh_view()?;
                    self.return_to = Some(self.screen);
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Analytics => {
                if let Some(m) = self.months.iter().rev().nth(self.month_cursor()) {
                    let (from, to) = self.month_bounds(m.start);
                    self.ops_view = OpsView::Range { from, to };
                    self.refresh_view()?;
                    self.return_to = Some(self.screen);
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Budget => {
                if let Some(line) = self.budget_lines.get(self.cursor()) {
                    let (from, to) = self.month_bounds(self.month_rule.cycle(self.today, 0).start);
                    self.ops_view = OpsView::Drill { label: line.category.clone(), from, to };
                    self.refresh_view()?;
                    self.return_to = Some(self.screen);
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Categories => {
                if let Some(line) = self.category_lines.get(self.cursor()) {
                    self.ops_view = OpsView::Drill { label: line.name.clone(), from: "0000-01-01".into(), to: "9999-12-31".into() };
                    self.refresh_view()?;
                    self.return_to = Some(self.screen);
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Flows => return self.drill(),
            Screen::Accounts => {
                if let Some(account) = self.accounts.get(self.cursor()) {
                    self.ops_view = OpsView::Account { id: account.id, name: account.name.clone() };
                    self.refresh_view()?;
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Counterparties => {
                if let Some(name) = self.selected_counterparty().map(|line| line.name.clone()) {
                    self.ops_view = OpsView::Counterparty { name };
                    self.refresh_view()?;
                    self.return_to = Some(Screen::Counterparties);
                    self.screen = Screen::Operations;
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Operations if self.fold => {
                if let Some(group) = self.groups().get(self.cursor()) {
                    self.ops_view = OpsView::Counterparty { name: group.name.clone() };
                    self.fold = false;
                    self.refresh_view()?;
                    self.apply_order();
                    self.return_to = Some(Screen::Operations);
                    self.cursors[Screen::Operations as usize] = 0;
                }
            }
            Screen::Operations | Screen::Debts | Screen::Duplicates | Screen::Journal => {
                self.zoom = !self.zoom;
                if self.zoom {
                    self.focus_detail();
                }
            }
        }
        Ok(())
    }

    /// Esc: a step back out of whatever was entered.
    pub fn back(&mut self) -> Result<()> {
        if self.pinned {
            self.pinned = false;
            self.zoom = false;
            return self.refresh_detail();
        }
        if self.zoom {
            self.zoom = false;
            return Ok(());
        }
        if self.focus == Focus::Detail {
            self.focus = Focus::List;
            return Ok(());
        }
        match (&self.screen, &self.ops_view) {
            (Screen::Operations, OpsView::Account { .. }) => {
                self.ops_view = OpsView::All;
                self.refresh_view()?;
                self.screen = Screen::Accounts;
            }
            (Screen::Operations, OpsView::Counterparty { .. }) => {
                self.ops_view = OpsView::All;
                self.refresh_view()?;
                self.apply_order();
                self.screen = self.return_to.take().unwrap_or(Screen::Counterparties);
            }
            (Screen::Operations, OpsView::Drill { .. } | OpsView::Range { .. } | OpsView::Import { .. }) => {
                self.ops_view = OpsView::All;
                self.refresh_view()?;
                // Back where Enter came from: flows, categories, budget or
                // analytics all drill into the same view.
                self.screen = self.return_to.take().unwrap_or(Screen::Flows);
            }
            (Screen::Operations, OpsView::Search { .. }) => {
                self.ops_view = OpsView::All;
                self.refresh_view()?;
                self.cursors[Screen::Operations as usize] = 0;
            }
            (Screen::Counterparties, _) if self.queue => self.stop_queue(),
            (Screen::Counterparties, _) if !self.counterparty_filter.is_empty() => {
                self.counterparty_filter.clear();
                self.cursors[Screen::Counterparties as usize] = 0;
            }
            _ => {}
        }
        Ok(())
    }

    // ── 6. The search ───────────────────────────────────────────────────────

    /// The journal as shown, folded by counterparty.
    pub fn groups(&self) -> Vec<query::Group> {
        query::group_by_counterparty(self.visible_operations())
    }

    pub(super) fn apply_order(&mut self) {
        query::sort(&mut self.operations, self.order);
        query::sort(&mut self.view_operations, self.order);
    }

    /// `o`: newest, then largest, then smallest first.
    pub fn next_order(&mut self) {
        self.order = match self.order {
            query::Order::Date => query::Order::Largest,
            query::Order::Largest => query::Order::Smallest,
            query::Order::Smallest => query::Order::Date,
        };
        self.apply_order();
        self.cursors[Screen::Operations as usize] = 0;
    }

    /// `f`: fold by counterparty and back.
    pub fn toggle_fold(&mut self) {
        self.fold = !self.fold;
        self.cursors[Screen::Operations as usize] = 0;
    }

    /// Space: mark the operation under the cursor, or unmark it; the cursor
    /// steps on, so a run of rows is marked by holding Space.
    pub fn toggle_mark(&mut self) {
        if self.fold {
            return;
        }
        if let Some(id) = self.visible_operations().get(self.cursor()).map(|o| o.id) {
            if !self.marked.remove(&id) {
                self.marked.insert(id);
            }
            self.move_cursor(1);
        }
    }

    /// Run one action on every marked operation; say how many it took and
    /// how many it refused (a leg removed with its pair, an already removed one).
    pub(super) fn on_marked(&mut self, what: impl Fn(&mut Connection, i64) -> Result<()>) -> Result<()> {
        let ids: Vec<i64> = self.marked.iter().copied().collect();
        let (mut done, mut refused) = (0, 0);
        for id in ids {
            match what(&mut self.conn, id) {
                Ok(()) => done += 1,
                Err(_) => refused += 1,
            }
        }
        self.marked.clear();
        self.status = Some(if refused == 0 {
            tr!("done: {done}", "сделано: {done}")
        } else {
            tr!("done: {done}, not possible: {refused}", "сделано: {done}, нельзя: {refused}")
        });
        Ok(())
    }

    /// `t` on Operations: transfers between own accounts out of the way, or
    /// back.
    pub fn toggle_transfers(&mut self) -> Result<()> {
        self.hide_transfers = !self.hide_transfers;
        self.status = Some(if self.hide_transfers {
            trs!("transfers between own accounts hidden · t — show", "переводы между своими скрыты · t — показать").into()
        } else {
            trs!("transfers shown", "переводы показаны").into()
        });
        self.reload()
    }

    /// The operations on the screen now: the whole journal or what is chosen.
    pub fn visible_operations(&self) -> &[OperationLine] {
        match self.ops_view {
            OpsView::All => &self.operations,
            _ => &self.view_operations,
        }
    }

    /// The counterparties with the filter applied: case aside, in any alphabet.
    pub fn visible_counterparties(&self) -> Vec<&CounterpartyLine> {
        let needle = self.counterparty_filter.to_lowercase();
        self.counterparties
            .iter()
            .filter(|line| needle.is_empty() || line.name.to_lowercase().contains(&needle))
            .filter(|line| !self.queue || line.category.is_none())
            .collect()
    }

    pub fn selected_counterparty(&self) -> Option<&CounterpartyLine> {
        if self.screen == Screen::Counterparties {
            return self.visible_counterparties().get(self.cursors[Screen::Counterparties as usize]).copied();
        }
        if let Detail::Counterparty(d) = &self.detail {
            return self.counterparties.iter().find(|c| c.name == d.name);
        }
        if let Detail::Operation(op) = &self.detail {
            if let Some(who) = &op.line.counterparty {
                return self.counterparties.iter().find(|c| &c.name == who);
            }
        }
        self.visible_counterparties().get(self.cursors[Screen::Counterparties as usize]).copied()
    }

}
