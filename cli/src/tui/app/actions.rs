//! What happens on the chosen menu item.
use super::*;

impl App {
    /// A menu action from a key.
    pub fn run_action(&mut self, action: MenuAction) -> Result<()> {
        self.run_reporting(action)
    }

    pub(super) fn run(&mut self, action: MenuAction) -> Result<()> {
        match action {
            MenuAction::OpenOperation(id) => return self.open_operation(id),
            MenuAction::TakeRefunds => {
                let (counted, moved) = potok_core::counterparty::take_refunds(&mut self.conn)?;
                self.status = Some(tr!(
                    "refunds now: {counted}, moved to their shop: {moved} · undo — u",
                    "теперь возвратами: {counted}, перенесены к магазину: {moved} · отменить — u"
                ));
            }
            MenuAction::TakeHints => {
                let taken = potok_core::counterparty::take_confident_hints(&mut self.conn, HINT_PERCENT)?;
                self.status = Some(tr!("categorised by their hint: {taken} · undo — u", "размечено по подсказке: {taken} · отменить — u"));
            }
            MenuAction::DropReview => {
                self.cancel_review();
                return Ok(());
            }
            MenuAction::RemoveOperation(id) => {
                let removed = potok_core::edit::remove_operation(&mut self.conn, id)?;
                self.status = Some(tr!(
                    "operations removed: {}{} · restore — x on it", "убрано операций: {}{} · вернуть — x на ней",
                    removed.operations,
                    if removed.debt_movements > 0 { trs!(" · debt recounted", " · долг пересчитан") } else { "" }
                ));
            }
            MenuAction::RestoreOperation(id) => {
                let back = potok_core::edit::restore_operation(&mut self.conn, id)?;
                self.status = Some(tr!("operations restored: {back}", "возвращено операций: {back}"));
            }
            MenuAction::UnlinkMovement(id) => {
                let unlinked = potok_core::edit::unlink_movement(&mut self.conn, id)?;
                self.status = Some(match unlinked.role_now {
                    Some(role) => tr!(
                        "unlinked · debt recounted · the operation is now {}", "отвязано · долг пересчитан · операция теперь {}",
                        if role == "expense" { trs!("an expense and shows in the flows", "расход и видна в потоках") } else { trs!("an income", "доход") }
                    ),
                    None => trs!("unlinked · debt recounted", "отвязано · долг пересчитан").into(),
                });
            }
            MenuAction::OwnCategory(id) => {
                self.pick_target = PickTarget::Operation(id);
                self.start_pick();
                return Ok(());
            }
            MenuAction::PromoteCategory { operation, who, category, from } => {
                // A counterparty's category is the rule itself (SPEC §4): it
                // covers all of its operations, past and future. This
                // operation's own category is surplus after that — it would
                // stay a separate record of the same thing.
                potok_core::edit::set_own_category(&self.conn, operation, None)?;
                let (affected, _) = potok_core::counterparty::take_over(&self.conn, &who, &category, from.as_deref())?;
                self.status = Some(tr!(
                    "{who} → {category}: {affected} operations, and all future ones",
                    "{who} → {category}: {affected} операций, и все будущие"
                ));
            }
            MenuAction::DropAlias(alias) => {
                potok_core::counterparty::drop_alias(&self.conn, &alias)?;
                self.status = Some(tr!("spelling «{alias}» removed", "написание «{alias}» убрано"));
            }
            MenuAction::ClearOwnCategory(id) => {
                potok_core::edit::set_own_category(&self.conn, id, None)?;
                self.status = Some(trs!("own category dropped — the counterparty's applies", "своя категория снята — берётся у контрагента").into());
            }
            MenuAction::CategorizeCounterparty(name) => {
                self.pick_target = PickTarget::Named(name);
                self.start_pick();
                return Ok(());
            }
            MenuAction::CounterpartyOperations(name) => {
                self.pinned = false;
                self.zoom = false;
                self.focus = Focus::List;
                self.return_to = Some(self.screen);
                self.ops_view = OpsView::Counterparty { name };
                self.refresh_view()?;
                self.screen = Screen::Operations;
                self.cursors[Screen::Operations as usize] = 0;
                return Ok(());
            }
            MenuAction::UnpairTransfer(id) => {
                potok_core::edit::unpair_transfer(&mut self.conn, id)?;
                self.status = Some(trs!("pair split — now an expense and an income", "пара разорвана — теперь это расход и доход").into());
            }
            MenuAction::FindPair(id) => {
                let found = potok_core::edit::transfer_candidates(&self.conn, id)?;
                if found.is_empty() {
                    self.status = Some(
                        trs!("no other leg found: it needs the same amount with the opposite sign on another account, ±3 days", "второй ноги не нашлось: нужна та же сумма с обратным знаком на другом счёте, ±3 дня").into(),
                    );
                    return Ok(());
                }
                self.menu = found
                    .into_iter()
                    .map(|other| MenuItem {
                        label: format!("{} {} {} {}", other.occurred_on, other.account, other.amount, other.description),
                        action: MenuAction::PairWith(id, other.id),
                        confirm: Some(tr!("Pair as a transfer with {} on {}?", "Связать в перевод с {} на {}?", other.amount, other.account)),
                    })
                    .collect();
                self.mode = Mode::Menu { selected: 0 };
                return Ok(());
            }
            MenuAction::PairWith(a, b) => {
                potok_core::edit::pair_transfer(&mut self.conn, a, b)?;
                self.status = Some(trs!("now a transfer between own accounts — out of the flows", "теперь это перевод между своими — из потоков ушёл").into());
            }
            MenuAction::ImportFile(path) => {
                let text = potok_core::import::pdf::text_of(&path)?;
                let statement = potok_core::import::parse(&text)?;
                match potok_core::import::review::account_for(&self.conn, &statement)? {
                    Some(account) => return self.start_review(path, statement, &account),
                    None => {
                        // There is no account named as the bank — let a person choose.
                        self.menu = self
                            .accounts
                            .iter()
                            .map(|a| MenuItem {
                                label: tr!("to account {}", "на счёт {}", a.name),
                                action: MenuAction::ImportInto(path.clone(), a.name.clone()),
                                confirm: None,
                            })
                            .collect();
                        self.mode = Mode::Menu { selected: 0 };
                    }
                }
                return Ok(());
            }
            MenuAction::ImportInto(path, account) => {
                let text = potok_core::import::pdf::text_of(&path)?;
                let statement = potok_core::import::parse(&text)?;
                return self.start_review(path, statement, &account);
            }
            MenuAction::ImportPath => {
                self.input = format!("{}/", self.statements_folder().trim_end_matches('/'));
                self.mode = Mode::Path;
                return Ok(());
            }
            MenuAction::WriteImport => {
                let Some(review) = self.review.take() else { return Ok(()) };
                let archive = self.archive_dir();
                let outcome = potok_core::import::store::apply(
                    &mut self.conn,
                    &review.path,
                    &review.statement,
                    &review.plan,
                    &archive,
                )?;
                self.status = Some(tr!(
                    "written {} · transfers {} · debt payments {} · duplicates skipped {} · roll back — x on the statements screen", "записано {} · переводов {} · по долгам {} · дублей пропущено {} · откатить — x на экране выписок",
                    outcome.operations_created,
                    outcome.transfers_paired,
                    outcome.debt_movements,
                    outcome.duplicates_skipped
                ));
                // iCard prints no balance, so nothing in the statement can
                // say a row is missing or doubled; only the bank can. Ask
                // now, while the app is open next to it — the 2026-10-18
                // minus went unnoticed for months for want of this.
                if review.statement.bank == potok_core::import::Bank::Icard && outcome.operations_created > 0 {
                    self.reload()?;
                    if let Some(at) = self.accounts.iter().position(|a| a.id == review.plan.account_id) {
                        self.screen = Screen::Accounts;
                        self.cursors[Screen::Accounts as usize] = at;
                        self.start_reconcile();
                    }
                    return Ok(());
                }
            }
            MenuAction::Unimport(id) => {
                let done = potok_core::import::store::rollback(&mut self.conn, id)?;
                self.status = Some(tr!(
                    "rolled back «{}»: {} operations, {} transfers split", "откачено «{}»: операций {}, переводов разорвано {}",
                    done.filename, done.operations, done.partners_unpaired
                ));
            }
            MenuAction::Fix(id) => {
                let Some(op) = detail::operation(&self.conn, id)? else { return Ok(()) };
                let line = &op.line;
                let account = self.accounts.iter().position(|a| a.name == line.account).unwrap_or(0);
                let who = line.counterparty.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| line.description.clone());
                let category = match op.category_source {
                    Some(detail::CategorySource::Own) => line.category.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                self.form = Some(self.operation_form(
                    Some(id),
                    account,
                    line.amount.minor() > 0,
                    line.amount.abs().decimal(),
                    line.occurred_on.clone(),
                    who,
                    category,
                ));
                self.mode = Mode::Form;
                return Ok(());
            }
            MenuAction::PickDebt(op) => {
                let Some(line) = query::operation_by_id(&self.conn, op)? else { return Ok(()) };
                let currency = line.amount.currency();
                self.menu = self
                    .debts
                    .iter()
                    .filter(|d| d.remaining.currency() == currency && d.remaining.minor() > 0)
                    .map(|d| {
                        let kind = potok_core::debt::kind_for(&d.direction, line.amount.minor());
                        let kind = if kind == "ISSUE" { trs!("issue", "выдача") } else { trs!("repayment", "возврат") };
                        MenuItem {
                            label: tr!("{} — {} left · as {kind}", "{} — осталось {} · как {kind}", d.counterparty, d.remaining),
                            action: MenuAction::LinkDebt(op, d.id),
                            confirm: Some(tr!("Link {} to the debt «{}» as {kind}?", "Привязать {} к долгу «{}» как {kind}?", line.amount, d.counterparty)),
                        }
                    })
                    .collect();
                self.menu.push(MenuItem {
                    label: trs!("New debt from this operation…", "Новый долг с этой операцией…").into(),
                    action: MenuAction::NewDebtWith(op),
                    confirm: None,
                });
                self.mode = Mode::Menu { selected: 0 };
                return Ok(());
            }
            MenuAction::LinkDebt(op, debt) => {
                let kind = potok_core::debt::link(&mut self.conn, debt, op)?;
                self.status = Some(tr!(
                    "linked as {} · debt remainder recounted", "привязано как {} · остаток долга пересчитан",
                    if kind == "ISSUE" { trs!("issue", "выдача") } else { trs!("repayment", "возврат") }
                ));
            }
            MenuAction::NewDebtWith(op) => {
                let Some(line) = query::operation_by_id(&self.conn, op)? else { return Ok(()) };
                let who = line.counterparty.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| line.description.clone());
                // Money came in — most likely borrowed; went out — lent.
                let owe = line.amount.minor() > 0;
                self.form = Some(self.debt_form(Some(op), who, owe, line.amount.currency().as_str(), line.occurred_on.clone()));
                self.mode = Mode::Form;
                return Ok(());
            }
            MenuAction::Count(id, counting) => {
                potok_core::edit::set_counting(&mut self.conn, id, counting)?;
                self.status = Some(
                    match counting {
                        potok_core::edit::Counting::Counted => trs!("counts again — in the flows", "считается снова — в потоках"),
                        potok_core::edit::Counting::Excluded => trs!("not counted — in the balance, not in the flows", "не считается — в балансе есть, в потоках нет"),
                        potok_core::edit::Counting::Passthrough => trs!("transit — in the balance, not in the flows", "транзит — в балансе есть, в потоках нет"),
                        potok_core::edit::Counting::Refund => trs!("a refund — its category's spending is down by it", "возврат — траты его категории меньше на эту сумму"),
                    }
                    .into(),
                );
            }
            MenuAction::Ask(ask) => {
                self.input = match &ask {
                    Ask::Rename(name) => name.clone(),
                    // What is already written is edited, not written from scratch.
                    Ask::Note(name) => potok_core::detail::counterparty(&self.conn, name)?.and_then(|c| c.note).unwrap_or_default(),
                    Ask::MergeInto(_) | Ask::Respell(_) | Ask::Alias(_) => String::new(),
                    // What is already written is edited.
                    Ask::OperationNote(id) => detail::operation(&self.conn, *id)?.and_then(|op| op.note).unwrap_or_default(),
                    Ask::StatementsDir => self.statements_folder(),
                    Ask::BackupDir => self.backup_dir().map(|d| d.display().to_string()).unwrap_or_default(),
                    Ask::DeviceName => crate::cloud::device_name(&self.conn),
                    Ask::RenameCategory(name) => name.clone(),
                    Ask::Rate | Ask::LockSet | Ask::LockOff => String::new(),
                    Ask::MonthStart(key) => {
                        let start = self.month_rule.overrides.get(key).copied().unwrap_or_else(|| self.month_rule.by_rule(*key));
                        start.to_string()
                    }
                    Ask::Range => match &self.ops_view {
                        OpsView::Range { from, to } => format!("{from} {to}"),
                        _ => self.today.format("%Y-%m").to_string(),
                    },
                    Ask::MergeCategory(_) => String::new(),
                    Ask::Budget(name) => potok_core::budget::get(&self.conn, name)
                        .ok()
                        .flatten()
                        .map(|m| m.decimal())
                        .unwrap_or_default(),
                    Ask::Split(id) => potok_core::edit::split_of(&self.conn, *id)
                        .unwrap_or_default()
                        .iter()
                        .map(|(c, a)| format!("{c} {}", potok_core::money::format_minor(*a)))
                        .collect::<Vec<_>>()
                        .join("; "),
                    Ask::RenameAccount(id) => {
                        self.accounts.iter().find(|a| a.id == *id).map(|a| a.name.clone()).unwrap_or_default()
                    }
                };
                self.asking = Some(ask);
                self.mode = Mode::Input;
                return Ok(());
            }
            MenuAction::Merge(from, into) => {
                let moved = potok_core::counterparty::merge(&mut self.conn, &from, &into)?;
                self.status = Some(tr!("«{from}» merged into «{into}»: {moved} operations", "«{from}» сведён в «{into}»: операций {moved}"));
            }
            MenuAction::Undo(id) => {
                self.status = Some(potok_core::journal::undo(&mut self.conn, id)?);
            }
            MenuAction::WriteOff(debt) => {
                let minor = potok_core::debt::write_off(&self.conn, debt, self.today)?;
                self.status = Some(tr!("wrote off {} — debt settled", "списано {} — долг закрыт", potok_core::money::format_minor(minor)));
            }
            MenuAction::Opening(id) => {
                let current = self.accounts.iter().find(|a| a.id == id).map(|a| a.name.clone());
                let opening: i64 =
                    potok_core::detail::account(&self.conn, id)?.map(|d| d.opening.minor()).unwrap_or(0);
                self.opening_for = current.map(|_| id);
                self.input = potok_core::Money::new(opening, potok_core::Currency::new("XXX")?).decimal();
                self.mode = Mode::Opening;
                return Ok(());
            }
            MenuAction::DeleteCategory(name) => {
                potok_core::category::delete(&mut self.conn, &name)?;
                self.status = Some(tr!("category «{name}» deleted", "категория «{name}» удалена"));
            }
            MenuAction::MergeCategory(from, into) => {
                let moved = potok_core::category::merge(&mut self.conn, &from, &into)?;
                self.status = Some(tr!("«{from}» merged into «{into}»: {moved} places", "«{from}» сведена в «{into}»: мест {moved}"));
            }
            MenuAction::MarkedCategory => {
                self.pick_target = PickTarget::Marked;
                self.start_pick();
                return Ok(());
            }
            MenuAction::MarkedCount(counting) => {
                self.on_marked(|conn, id| potok_core::edit::set_counting(conn, id, counting))?;
            }
            MenuAction::MarkedRemove => {
                self.on_marked(|conn, id| potok_core::edit::remove_operation(conn, id).map(|_| ()))?;
            }
            MenuAction::RatesMenu => {
                let auto = potok_core::settings::get(&self.conn, potok_core::settings::RATES_AUTO)?.as_deref() == Some("on");
                let mut menu = vec![
                    MenuItem { label: tr!("Fetch now — ECB and Bank of Russia", "Загрузить сейчас — ЕЦБ и ЦБ РФ"), action: MenuAction::RatesUpdate, confirm: None },
                    MenuItem {
                        label: if auto { tr!("Stop fetching daily on start", "Не загружать каждый день при запуске") } else { tr!("Fetch daily on start", "Загружать каждый день при запуске") },
                        action: MenuAction::RatesAuto(!auto),
                        confirm: None,
                    },
                    MenuItem { label: tr!("Add a rate by hand…", "Добавить курс руками…"), action: MenuAction::Ask(Ask::Rate), confirm: None },
                ];
                for row in &self.rates {
                    for (from, to, day, source) in &row.stored {
                        menu.push(MenuItem {
                            label: tr!("Delete {from}→{to} of {day} ({source})", "Удалить {from}→{to} от {day} ({source})"),
                            action: MenuAction::RateDelete(from.clone(), to.clone(), day.clone()),
                            confirm: Some(tr!("Delete the rate {from}→{to} of {day}?", "Удалить курс {from}→{to} от {day}?")),
                        });
                    }
                }
                self.menu = menu;
                self.mode = Mode::Menu { selected: 0 };
                return Ok(());
            }
            MenuAction::RatesUpdate => {
                let done = potok_core::rate::update(&self.conn)?;
                self.status = Some(if done.failed.is_empty() {
                    tr!("rates stored: {}", "курсов сохранено: {}", done.stored)
                } else {
                    tr!("rates stored: {} · not fetched: {}", "курсов сохранено: {} · не загружено: {}", done.stored, done.failed.join("; "))
                });
            }
            MenuAction::RatesAuto(on) => {
                potok_core::settings::set(&self.conn, potok_core::settings::RATES_AUTO, if on { "on" } else { "off" })?;
                self.status = Some(if on { tr!("rates will be fetched once a day on start", "курсы будут загружаться раз в день при запуске") } else { tr!("no fetching on start", "без загрузки при запуске") });
            }
            MenuAction::RateDelete(from, to, day) => {
                potok_core::rate::delete(&self.conn, &from, &to, &day)?;
                self.status = Some(tr!("rate deleted", "курс удалён"));
            }
            MenuAction::MarkedClear => {
                self.marked.clear();
                self.status = Some(tr!("marks cleared", "отметки сняты"));
                return Ok(());
            }
            MenuAction::ExportCsv => {
                let dir = potok_core::backup::expand(&self.statements_folder());
                let path = dir.join(format!("potok-{}.csv", chrono::Local::now().format("%Y%m%d-%H%M%S")));
                std::fs::write(&path, potok_core::export::csv(self.visible_operations()))?;
                self.status = Some(tr!("written: {}", "записано: {}", path.display()));
                return Ok(());
            }
            MenuAction::GoCategories => {
                self.go_to(Screen::Categories);
                return Ok(());
            }
            MenuAction::NewTransfer => {
                let names: Vec<String> = self.accounts.iter().map(|a| a.name.clone()).collect();
                if names.len() < 2 {
                    self.status = Some(tr!("a transfer needs two accounts", "для перевода нужны два счёта"));
                    return Ok(());
                }
                self.form = Some(Form {
                    kind: FormKind::Transfer,
                    title: trs!("New transfer between own accounts", "Новый перевод между своими").into(),
                    fields: vec![
                        Field::choice("from", trs!("from", "откуда"), names.clone(), 0),
                        Field::choice("to", trs!("to", "куда"), names, 1),
                        Field::text("amount", trs!("sent", "ушло"), ""),
                        Field::optional("received", trs!("arrived", "пришло"), "", trs!("— the same, unless currencies differ", "— столько же, если валюта одна")),
                        Field::text("date", trs!("date", "дата"), self.today.to_string()),
                        Field::optional("note", trs!("note", "заметка"), "", trs!("— transfer", "— перевод")),
                    ],
                    field: 2,
                    error: None,
                });
                self.mode = Mode::Form;
                return Ok(());
            }
            MenuAction::EditDebt(id) => {
                let now = potok_core::debt::fields(&self.conn, id)?;
                self.form = Some(Form {
                    kind: FormKind::DebtEdit(id),
                    title: tr!("Edit debt #{id}", "Правка долга #{id}"),
                    fields: vec![
                        Field::choice(
                            "direction",
                            trs!("direction", "кто кому"),
                            vec![trs!("I owe", "я должен").into(), trs!("owed to me", "мне должны").into()],
                            usize::from(now.direction != "I_OWE"),
                        ),
                        Field::optional("due", trs!("due", "срок"), now.due_on.unwrap_or_default(), trs!("— no due date", "— без срока")),
                        Field::optional("note", trs!("note", "заметка"), now.note.unwrap_or_default(), "—"),
                    ],
                    field: 1,
                    error: None,
                });
                self.mode = Mode::Form;
                return Ok(());
            }
            MenuAction::DeleteDebt(id) => {
                potok_core::debt::delete(&mut self.conn, id)?;
                self.zoom = false;
                self.status = Some(tr!("debt deleted", "долг удалён"));
            }
            MenuAction::NewAccount => {
                let mut currencies: Vec<String> =
                    self.accounts.iter().map(|a| a.balance.currency().as_str().to_string()).collect();
                for code in ["EUR", "USD", "GBP", "BGN", "RUB"] {
                    currencies.push(code.into());
                }
                let mut seen = std::collections::HashSet::new();
                currencies.retain(|c| seen.insert(c.clone()));
                let kinds: Vec<String> = [trs!("bank account", "счёт в банке"), trs!("card", "карта"), trs!("cash", "наличные"), trs!("other", "другое")]
                    .iter()
                    .map(|k| k.to_string())
                    .collect();
                self.form = Some(Form {
                    kind: FormKind::Account,
                    title: trs!("New account", "Новый счёт").into(),
                    fields: vec![
                        Field::text("name", trs!("name", "имя"), ""),
                        Field::choice("currency", trs!("currency", "валюта"), currencies, 0),
                        Field::choice("kind", trs!("kind", "вид"), kinds, 0),
                        Field::optional("opening", trs!("opening", "начальный"), "", "0.00"),
                    ],
                    field: 0,
                    error: None,
                });
                self.mode = Mode::Form;
                return Ok(());
            }
            MenuAction::Archive(id, on) => {
                potok_core::account::set_archived(&self.conn, id, on)?;
                self.status = Some(if on {
                    tr!("archived · back — x → «Archived accounts…»", "в архиве · вернуть — x → «Счета в архиве…»")
                } else {
                    tr!("back from the archive", "возвращён из архива")
                });
            }
            MenuAction::ShowArchived => {
                self.menu = potok_core::account::archived(&self.conn)?
                    .into_iter()
                    .map(|(id, name)| MenuItem {
                        label: tr!("Bring back «{name}»", "Вернуть «{name}»"),
                        action: MenuAction::Archive(id, false),
                        confirm: None,
                    })
                    .collect();
                self.mode = Mode::Menu { selected: 0 };
                return Ok(());
            }
            MenuAction::DismissAll(keys) => {
                for key in &keys {
                    potok_core::reconcile::dismiss_duplicates(&self.conn, key)?;
                }
                self.status = Some(tr!("not duplicates: {} groups", "не дубли: {} групп", keys.len()));
            }
            MenuAction::DismissDuplicates(key) => {
                potok_core::reconcile::dismiss_duplicates(&self.conn, &key)?;
                self.status = Some(trs!("not a duplicate — will not show it again", "не дубль — больше не покажу").into());
            }
            MenuAction::AccountOperations(_) => return self.open(),
            MenuAction::Drill if matches!(self.screen, Screen::Categories | Screen::Budget | Screen::Analytics) => return self.open(),
            MenuAction::Drill => return self.drill(),
            MenuAction::Zoom => {
                self.zoom = !self.zoom;
                if self.zoom {
                    self.focus_detail();
                }
                return Ok(());
            }
        }
        self.reload()
    }

}
