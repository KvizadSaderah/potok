//! Choosing a category: one counterparty, the marked ones, a queue in a row.
use super::*;

impl App {
    pub fn start_pick(&mut self) {
        self.input.clear();
        self.mode = Mode::PickCategory { selected: 0 };
    }

    /// `c` on the import screen — a category for the counterparty.
    pub fn start_counterparty_pick(&mut self) {
        self.pick_target = PickTarget::Counterparty;
        self.start_pick();
    }

    /// `C` on Import: every uncategorised counterparty in turn, the ones with
    /// the most money first — the order the list is already in.
    pub fn start_queue(&mut self) {
        self.queue = true;
        self.counterparty_filter.clear();
        self.cursors[Screen::Counterparties as usize] = 0;
        if self.selected_counterparty().is_none() {
            self.queue = false;
            self.status = Some(trs!("everybody has a category", "у всех есть категория").to_string());
            return;
        }
        self.start_counterparty_pick();
    }

    /// The next one in the queue, or the end of it.
    pub(super) fn queue_next(&mut self) {
        let left = self.visible_counterparties().len();
        if left == 0 || self.cursors[Screen::Counterparties as usize] >= left {
            self.stop_queue();
            self.status = Some(trs!("the queue is done", "очередь пройдена").to_string());
            return;
        }
        self.start_counterparty_pick();
    }

    /// Tab in the picker: leave this one for later.
    pub fn queue_skip(&mut self) {
        self.mode = Mode::Normal;
        self.input.clear();
        self.cursors[Screen::Counterparties as usize] += 1;
        self.queue_next();
    }

    pub fn stop_queue(&mut self) {
        self.queue = false;
        self.mode = Mode::Normal;
        self.input.clear();
        self.cursors[Screen::Counterparties as usize] = 0;
    }

    /// The rows of the choosing window: the hint if there is one, then the
    /// categories matching what is typed, then «create» if there is no such one.
    pub fn picks(&self) -> Vec<Pick> {
        let mut out = Vec::new();
        let typed = self.input.trim();
        if typed.is_empty() && self.pick_target == PickTarget::Counterparty {
            if let Some(line) = self.selected_counterparty() {
                if line.category.is_none() {
                    if let Some((category, count)) = &line.hint {
                        out.push(Pick::Hint {
                            category: category.clone(),
                            count: *count,
                            of: line.operations,
                        });
                    }
                }
            }
        }
        let hinted = out.first().map(|p| p.category().to_string());
        let mut ranked: Vec<(u8, &String)> = self
            .categories
            .iter()
            .filter(|name| Some(name.as_str()) != hinted.as_deref())
            .filter_map(|name| fuzzy_rank(typed, name).map(|rank| (rank, name)))
            .collect();
        ranked.sort();
        out.extend(ranked.into_iter().map(|(_, name)| Pick::Existing(name.clone())));
        let exists = self.categories.iter().any(|name| name.to_lowercase() == typed.to_lowercase());
        if !typed.is_empty() && !exists {
            out.push(Pick::Create(typed.to_string()));
        }
        out
    }

    pub fn move_pick(&mut self, by: isize) {
        if let Mode::PickCategory { selected } = self.mode {
            let count = self.picks().len();
            if count == 0 {
                return;
            }
            let at = (selected as isize + by).clamp(0, count as isize - 1) as usize;
            self.mode = Mode::PickCategory { selected: at };
        }
    }

    /// Typing resets the choice to the top: the list under it has changed.
    pub fn type_into_pick(&mut self) {
        if let Mode::PickCategory { .. } = self.mode {
            self.mode = Mode::PickCategory { selected: 0 };
        }
    }

    pub fn apply_pick(&mut self) -> Result<()> {
        let Mode::PickCategory { selected } = self.mode else { return Ok(()) };
        self.mode = Mode::Normal;
        let Some(pick) = self.picks().get(selected).cloned() else {
            self.status = Some(trs!("nothing to apply — type a category name", "нечего применить — набери название категории").to_string());
            return Ok(());
        };
        let category = pick.category().to_string();
        if self.pick_target == PickTarget::Marked {
            self.pick_target = PickTarget::Counterparty;
            self.input.clear();
            self.on_marked(|conn, id| potok_core::edit::set_own_category(conn, id, Some(&category)))?;
            return self.reload();
        }
        if let PickTarget::Operation(id) = self.pick_target {
            // Where this operation was counted before the edit: that is where
            // the same counterparty's other operations look too, and they move.
            let was = detail::operation(&self.conn, id)?.and_then(|op| op.line.category.clone());
            potok_core::edit::set_own_category(&self.conn, id, Some(&category))?;
            self.pick_target = PickTarget::Counterparty;
            self.status = Some(tr!("operation → {category} (its own, for it only)", "операция → {category} (своя, только для неё)"));
            self.input.clear();
            self.reload()?;
            // One row corrected usually means the rest of that shop is wrong
            // too: offer to move them and to remember, rather than make the
            // same correction twenty times.
            self.offer_to_promote(id, &category, was)?;
            return Ok(());
        }
        // Either the named one (marking from an operation's menu) or the one
        // under the cursor on «Counterparties».
        let named = match &self.pick_target {
            PickTarget::Named(who) => Some(who.clone()),
            _ => None,
        };
        self.pick_target = PickTarget::Counterparty;
        let Some(who) = named.or_else(|| self.selected_counterparty().map(|line| line.name.clone())) else {
            self.status = Some(trs!("nobody to categorise", "некого разметить").to_string());
            return Ok(());
        };
        // Where its operations are counted now is where they are pulled from: an
        // operation's own category beats the rule, and without this step marking
        // the counterparty would touch not one old row (after the move from
        // saldo the category is written on every one).
        let was = self
            .counterparties
            .iter()
            .find(|line| line.name == who)
            .and_then(|line| line.category.clone().or_else(|| line.hint.as_ref().map(|(name, _)| name.clone())));
        let (affected, moved) = potok_core::counterparty::take_over(&self.conn, &who, &category, was.as_deref())?;
        self.status = Some(tr!(
            "{who} → {category}: {affected} operations, and all future ones{}{}", "{who} → {category}: {affected} операций, и все будущие{}{}",
            if moved > 0 { tr!(" · taken out of {}: {moved}", " · вынуто из {}: {moved}", was.as_deref().unwrap_or("—")) } else { String::new() },
            if matches!(pick, Pick::Create(_)) { trs!(" (category created)", " (категория создана)") } else { "" }
        ));
        self.input.clear();
        self.reload()?;
        // The one just categorised left the list; the cursor is on the next.
        if self.queue {
            self.queue_next();
        }
        Ok(())
    }

    /// After an own category on one operation: the counterparty has more
    /// operations — offer to move them too. «No» leaves the edit on the one;
    /// «yes» makes it the counterparty's category, and it covers the past and
    /// the future.
    pub(super) fn offer_to_promote(&mut self, operation: i64, category: &str, from: Option<String>) -> Result<()> {
        let Some(op) = detail::operation(&self.conn, operation)? else { return Ok(()) };
        let Some(who) = op.line.counterparty.clone().filter(|name| !name.is_empty()) else { return Ok(()) };
        let Some(party) = detail::counterparty(&self.conn, &who)? else { return Ok(()) };
        // Already its category — nothing to move; one operation — the same.
        if party.category.as_deref() == Some(category) || party.operations < 2 {
            return Ok(());
        }
        let rest = party.operations - 1;
        self.pending = Some(MenuItem {
            label: String::new(),
            action: MenuAction::PromoteCategory { operation, who: who.clone(), category: category.to_string(), from },
            confirm: Some(tr!(
                "«{who}» has {rest} more operations. Move them to {category} too, and remember it for the next ones?",
                "У «{who}» ещё {rest} операций. Перенести и их в {category}, и запоминать дальше?"
            )),
        });
        self.mode = Mode::Confirm;
        Ok(())
    }

}
