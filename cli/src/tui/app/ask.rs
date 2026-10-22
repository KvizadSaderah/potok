//! The input line: the search, the reconciliation and all that is asked in one line.
use super::*;

impl App {
    /// Enter in `Mode::Input`.
    pub fn apply_input(&mut self) -> Result<()> {
        self.mode = Mode::Normal;
        let Some(ask) = self.asking.take() else { return Ok(()) };
        let text = self.input.trim().to_string();
        match ask {
            Ask::MonthStart(key) => {
                let done = if text.is_empty() {
                    potok_core::period::set_start(&self.conn, key, None)
                } else {
                    potok_core::edit::parse_date(&text, self.today).and_then(|day| potok_core::period::set_start(&self.conn, key, Some(day)))
                };
                self.status = Some(match done {
                    Ok(()) => tr!("month start saved — flows, analytics and budgets recounted", "начало месяца сохранено — потоки, аналитика и бюджеты пересчитаны"),
                    Err(error) => tr!("not saved: {error}", "не сохранено: {error}"),
                });
                self.load_settings()?;
            }
            Ask::LockSet => {
                self.status = Some(match crate::tui::lock::hash(&self.input) {
                    Ok(hash) => {
                        potok_core::settings::set(&self.conn, potok_core::settings::LOCK_HASH, &hash)?;
                        tr!("lock on — asked at start, L locks now", "блокировка включена — спросит при запуске, L блокирует сейчас")
                    }
                    Err(error) => tr!("not set: {error}", "не включена: {error}"),
                });
                self.input.clear();
                return Ok(());
            }
            Ask::LockOff => {
                if self.lock_wait() > 0 {
                    self.input.clear();
                    self.status = Some(tr!("wait {} s", "подожди {} с", self.lock_wait()));
                    return Ok(());
                }
                let ok = self.lock_hash().is_some_and(|h| crate::tui::lock::verify(&self.input, &h));
                self.input.clear();
                self.status = Some(if ok {
                    self.lock_failures = 0;
                    self.lock_wait_until = None;
                    potok_core::settings::clear(&self.conn, potok_core::settings::LOCK_HASH)?;
                    tr!("lock off", "блокировка выключена")
                } else {
                    self.lock_failures += 1;
                    let wait = (1u64 << (self.lock_failures - 1).min(5)).min(30);
                    self.lock_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(wait));
                    tr!("wrong passphrase — the lock stays", "пароль не тот — блокировка осталась")
                });
                return Ok(());
            }
            Ask::Range => {
                let done = (|| -> Result<(String, String)> {
                    let words: Vec<&str> = text.split_whitespace().collect();
                    match words.as_slice() {
                        // «2026-09» — the whole month
                        [month] if month.len() == 7 => {
                            let start = chrono::NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")?;
                            Ok(self.month_bounds(start))
                        }
                        [from, to] => Ok((
                            potok_core::edit::parse_date(from, self.today)?.to_string(),
                            potok_core::edit::parse_date(to, self.today)?.to_string(),
                        )),
                        _ => anyhow::bail!(tr!("type 2026-09, or two dates", "набери 2026-09 или две даты")),
                    }
                })();
                match done {
                    Ok((from, to)) => {
                        self.ops_view = OpsView::Range { from, to };
                        self.refresh_view()?;
                        self.apply_order();
                        self.return_to = Some(Screen::Operations);
                        self.cursors[Screen::Operations as usize] = 0;
                    }
                    Err(error) => self.status = Some(tr!("no range: {error}", "отрезок не задан: {error}")),
                }
                return Ok(());
            }
            Ask::Rate => {
                let done = (|| -> Result<String> {
                    let mut parts = text.split_whitespace();
                    let (Some(code), Some(rate)) = (parts.next(), parts.next()) else {
                        anyhow::bail!(tr!("type «RUB 0.0098»", "набери «RUB 0.0098»"));
                    };
                    let on = match parts.next() {
                        Some(day) => potok_core::edit::parse_date(day, self.today)?,
                        None => self.today,
                    };
                    let base = potok_core::settings::base_currency(&self.conn)?;
                    let from = potok_core::Currency::new(&code.to_uppercase())?;
                    let rate = potok_core::rate::parse(rate)?;
                    potok_core::rate::set(&self.conn, from, base, rate, &on.to_string())?;
                    Ok(tr!("1 {from} = {} {base} from {on}", "1 {from} = {} {base} с {on}", potok_core::rate::show(rate)))
                })();
                self.status = Some(done.unwrap_or_else(|e| tr!("no rate: {e}", "курс не сохранён: {e}")));
            }
            Ask::RenameCategory(from) => {
                self.status = Some(match potok_core::category::rename(&self.conn, &from, &text) {
                    Ok(()) => tr!("«{from}» is now «{text}»", "«{from}» теперь «{text}»"),
                    Err(error) => tr!("not renamed: {error}", "не переименовано: {error}"),
                });
            }
            Ask::MergeCategory(from) => {
                let wanted = text.to_lowercase();
                let mut found: Vec<String> = self
                    .category_lines
                    .iter()
                    .map(|c| c.name.clone())
                    .filter(|name| *name != from && name.to_lowercase().contains(&wanted))
                    .collect();
                found.sort_by_key(|name| name.to_lowercase() != wanted);
                found.truncate(12);
                if wanted.is_empty() || found.is_empty() {
                    self.status = Some(tr!("no category like «{text}»", "нет категории похожей на «{text}»"));
                    return Ok(());
                }
                self.menu = found
                    .into_iter()
                    .map(|into| MenuItem {
                        label: tr!("into «{into}»", "в «{into}»"),
                        confirm: Some(tr!(
                            "Merge «{from}» into «{into}»? Everything in it moves; this cannot be undone",
                            "Свести «{from}» в «{into}»? Всё из неё перейдёт; это не отменяется"
                        )),
                        action: MenuAction::MergeCategory(from.clone(), into),
                    })
                    .collect();
                self.mode = Mode::Menu { selected: 0 };
                return Ok(());
            }
            Ask::Budget(name) => {
                let done = if text.is_empty() {
                    potok_core::budget::set(&self.conn, &name, None)
                } else {
                    potok_core::reconcile::parse_input(&text).and_then(|minor| potok_core::budget::set(&self.conn, &name, Some(minor.abs())))
                };
                self.status = Some(match done {
                    Ok(()) => tr!("budget for «{name}» saved", "бюджет «{name}» сохранён"),
                    Err(error) => tr!("not saved: {error}", "не сохранено: {error}"),
                });
            }
            Ask::Split(id) => {
                let total = query::operation_by_id(&self.conn, id)?.map(|o| o.amount.minor().abs()).unwrap_or(0);
                let done = if text.is_empty() {
                    potok_core::edit::split(&mut self.conn, id, &[])
                } else {
                    potok_core::edit::parse_split(&text, total).and_then(|parts| potok_core::edit::split(&mut self.conn, id, &parts))
                };
                self.status = Some(match done {
                    Ok(()) => tr!("split saved — the flows count the parts", "разбивка сохранена — потоки считают части"),
                    Err(error) => tr!("not split: {error}", "не разбито: {error}"),
                });
            }
            Ask::RenameAccount(id) => {
                self.status = Some(match potok_core::account::rename(&self.conn, id, &text) {
                    Ok(()) => tr!("the account is now «{text}»", "счёт теперь «{text}»"),
                    Err(error) => tr!("not renamed: {error}", "не переименовано: {error}"),
                });
            }
            Ask::DeviceName => {
                let key = potok_core::settings::DEVICE_NAME;
                if text.is_empty() {
                    potok_core::settings::clear(&self.conn, key)?;
                } else {
                    potok_core::settings::set(&self.conn, key, &text)?;
                }
                self.status = Some(tr!("this device is «{}»", "это устройство — «{}»", crate::cloud::device_name(&self.conn)));
                return Ok(());
            }
            Ask::BackupDir => {
                let key = potok_core::settings::BACKUP_DIR;
                if text.is_empty() {
                    potok_core::settings::clear(&self.conn, key)?;
                } else {
                    potok_core::settings::set(&self.conn, key, &text)?;
                }
                self.status = Some(tr!(
                    "backups folder: {}",
                    "папка бэкапов: {}",
                    self.backup_dir().map(|d| d.display().to_string()).unwrap_or_default()
                ));
                return Ok(());
            }
            Ask::StatementsDir => {
                let key = potok_core::settings::STATEMENTS_DIR;
                if text.is_empty() {
                    potok_core::settings::clear(&self.conn, key)?;
                } else {
                    potok_core::settings::set(&self.conn, key, &text)?;
                }
                self.load_settings()?;
                self.status = Some(tr!("statements folder: {}", "папка выписок: {}", self.statements_folder()));
                return Ok(());
            }
            Ask::Note(who) => {
                let done = potok_core::counterparty::set_note(&self.conn, &who, &text);
                self.status = Some(match done {
                    Ok(()) if text.trim().is_empty() => tr!("note on «{who}» removed", "заметка с «{who}» снята"),
                    Ok(()) => tr!("«{who}»: {text}", "«{who}»: {text}"),
                    Err(error) => tr!("not written: {error}", "не записано: {error}"),
                });
            }
            Ask::OperationNote(id) => {
                let done = potok_core::edit::set_note(&self.conn, id, &text);
                self.status = Some(match done {
                    Ok(()) if text.trim().is_empty() => tr!("note on #{id} removed", "заметка с #{id} снята"),
                    Ok(()) => tr!("#{id}: {text}", "#{id}: {text}"),
                    Err(error) => tr!("not written: {error}", "не записано: {error}"),
                });
            }
            Ask::Alias(who) => {
                let done = potok_core::counterparty::set_alias(&self.conn, &text, &who);
                self.status = Some(match done {
                    Ok(alias) => tr!("«{alias}» now leads to «{who}»", "«{alias}» теперь ведёт на «{who}»"),
                    Err(error) => tr!("not tied: {error}", "не привязано: {error}"),
                });
            }
            Ask::Rename(from) => {
                let done = potok_core::counterparty::rename(&self.conn, &from, &text);
                self.status = Some(match done {
                    Ok(()) => tr!("«{from}» is now «{text}»", "«{from}» теперь «{text}»"),
                    Err(error) => tr!("not renamed: {error}", "не переименовано: {error}"),
                });
            }
            Ask::Respell(op) => {
                let done = potok_core::counterparty::reassign(&mut self.conn, op, &text);
                self.status = Some(match done {
                    Ok(moved) => tr!("now «{text}»: {moved} operations, and the same on future imports", "теперь «{text}»: операций {moved}, и так же будет при следующих импортах"),
                    Err(error) => tr!("not done: {error}", "не сделано: {error}"),
                });
            }
            Ask::MergeInto(from) => {
                // Part of a name is typed: there may be several candidates, and
                // a person chooses rather than the first match.
                let wanted = text.to_lowercase();
                let mut found: Vec<&str> = self
                    .counterparties
                    .iter()
                    .map(|c| c.name.as_str())
                    .filter(|name| *name != from && name.to_lowercase().contains(&wanted))
                    .collect();
                found.sort_by_key(|name| name.to_lowercase() != wanted);
                found.truncate(12);
                if wanted.is_empty() || found.is_empty() {
                    self.status = Some(tr!("nobody like «{text}» — nothing to merge into", "никого похожего на «{text}» — свести не в кого"));
                    return Ok(());
                }
                self.menu = found
                    .into_iter()
                    .map(|into| MenuItem {
                        label: tr!("into «{into}»", "в «{into}»"),
                        action: MenuAction::Merge(from.clone(), into.to_string()),
                        confirm: Some(tr!(
                            "Merge «{from}» into «{into}»? Operations and spellings move; this cannot be undone", "Свести «{from}» в «{into}»? Операции и написания перейдут; это не отменяется"
                        )),
                    })
                    .collect();
                self.mode = Mode::Menu { selected: 0 };
                return Ok(());
            }
        }
        self.reload()
    }

    pub fn start_search(&mut self) -> Result<()> {
        self.input = match (&self.screen, &self.ops_view) {
            (Screen::Counterparties, _) => self.counterparty_filter.clone(),
            (Screen::Operations, OpsView::Search { text }) => text.clone(),
            _ => String::new(),
        };
        if self.screen != Screen::Counterparties {
            self.screen = Screen::Operations;
        }
        self.mode = Mode::Search;
        self.apply_search()
    }

    /// Applies what was typed — on every letter.
    pub fn apply_search(&mut self) -> Result<()> {
        if self.screen == Screen::Counterparties {
            self.counterparty_filter = self.input.clone();
            self.cursors[Screen::Counterparties as usize] = 0;
            return Ok(());
        }
        self.ops_view = if self.input.trim().is_empty() {
            OpsView::All
        } else {
            OpsView::Search { text: self.input.trim().to_string() }
        };
        self.refresh_view()?;
        self.cursors[Screen::Operations as usize] = 0;
        Ok(())
    }

    /// Esc in the middle of typing: as though there had been no search.
    pub fn cancel_search(&mut self) -> Result<()> {
        self.input.clear();
        self.mode = Mode::Normal;
        if self.screen == Screen::Counterparties {
            self.counterparty_filter.clear();
            self.cursors[Screen::Counterparties as usize] = 0;
            return Ok(());
        }
        self.ops_view = OpsView::All;
        self.refresh_view()
    }

    // ── 7. Marking by choosing ──────────────────────────────────────────────

    /// `s` on an account: type the remainder the bank shows.
    pub fn start_reconcile(&mut self) {
        if self.accounts.get(self.cursor()).is_none() {
            self.status = Some(trs!("nothing to reconcile — no accounts", "нечего сверять — счетов нет").into());
            return;
        }
        self.input.clear();
        self.mode = Mode::Reconcile;
    }

    /// Enter in the reconciliation: «507.12» — today, «507.12 2026-10-16» — on a date.
    pub fn apply_reconcile(&mut self) -> Result<()> {
        self.mode = Mode::Normal;
        let Some(account) = self.accounts.get(self.cursor()).map(|a| a.id) else { return Ok(()) };
        let mut parts = self.input.rsplitn(2, ' ');
        let (amount, on) = match (parts.next(), parts.next()) {
            (Some(last), Some(rest)) if last.len() == 10 && last.as_bytes()[4] == b'-' => {
                (rest.to_string(), last.to_string())
            }
            _ => (self.input.clone(), self.today.to_string()),
        };
        let done = potok_core::reconcile::parse_input(&amount)
            .and_then(|bank| potok_core::reconcile::reconcile(&self.conn, account, &on, bank));
        self.status = Some(match done {
            Ok(r) if r.matches() => tr!("{} on {}: matches", "{} на {}: сходится", r.account, r.on_date),
            Ok(r) => tr!(
                "{} on {}: off by {} · without look-alike operations it would be {}", "{} на {}: расходится на {} · без похожих операций было бы {}",
                r.account,
                r.on_date,
                r.difference,
                r.without_duplicates()
            ),
            Err(error) => tr!("not reconciled: {error}", "не сверено: {error}"),
        });
        self.reload()
    }

}
