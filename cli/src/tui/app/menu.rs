//! The menu behind `x`: what can be done with what is under the cursor.
use super::*;

impl App {
    /// The menu items for what is under the cursor. Empty — nothing to do here.
    pub fn menu_items(&self) -> Result<Vec<MenuItem>> {
        let mut items = Vec::new();
        let item = |label: &str, action: MenuAction| MenuItem { label: label.into(), action, confirm: None };

        // Marked operations come first: marking is a decision to act on all.
        if self.screen == Screen::Operations && !self.marked.is_empty() && self.focus == Focus::List {
            let n = self.marked.len();
            items.push(item(&tr!("The {n} marked: a category…", "Отмеченные ({n}): категория…"), MenuAction::MarkedCategory));
            items.push(item(&tr!("The {n} marked: do not count in flows", "Отмеченные ({n}): не считать в потоках"), MenuAction::MarkedCount(potok_core::edit::Counting::Excluded)));
            // Transit and refund for the marked rows — the same roles as for one.
            // Without them a person reached for the item below, and that one
            // edited the row under the cursor: three marked, and a fourth became transit.
            items.push(item(&tr!("The {n} marked: transit — passed through me", "Отмеченные ({n}): транзит — прошло через меня"), MenuAction::MarkedCount(potok_core::edit::Counting::Passthrough)));
            items.push(item(&tr!("The {n} marked: a refund", "Отмеченные ({n}): возврат"), MenuAction::MarkedCount(potok_core::edit::Counting::Refund)));
            items.push(item(&tr!("The {n} marked: count again", "Отмеченные ({n}): считать снова"), MenuAction::MarkedCount(potok_core::edit::Counting::Counted)));
            let mut remove = item(&tr!("The {n} marked: remove", "Отмеченные ({n}): убрать"), MenuAction::MarkedRemove);
            remove.confirm = Some(tr!("Remove the {n} marked? Each can be restored", "Убрать {n} отмеченных? Каждую можно вернуть"));
            items.push(remove);
            items.push(item(trs!("Clear the marks", "Снять отметки"), MenuAction::MarkedClear));
        }
        // A debt movement or a month's flow row has its own menu, before the operation's.
        if self.focus == Focus::Detail {
            match self.detail_items().get(self.detail_cursor) {
                Some(DetailItem::Movement { id, operation }) => {
                    if let Some(op) = operation {
                        items.push(item(trs!("Open the operation", "Открыть операцию"), MenuAction::OpenOperation(*op)));
                    }
                    let mut unlink = item(trs!("Not a debt payment — unlink", "Это не платёж по долгу — отвязать"), MenuAction::UnlinkMovement(*id));
                    unlink.confirm = Some(
                        trs!("Unlink the movement from the debt? The remainder is recounted, the operation stays", "Отвязать движение от долга? Остаток долга пересчитается, операция останется").into(),
                    );
                    items.push(unlink);
                }
                Some(DetailItem::MonthFlow { label, .. }) => {
                    items.push(item(
                        &tr!("Operations of «{label}» in this month", "Операции «{label}» за этот месяц"),
                        MenuAction::Drill,
                    ));
                    return Ok(items);
                }
                _ => {}
            }
        }

        if let Some(id) = self.focused_operation() {
            let op = detail::operation(&self.conn, id)?;
            if let Some(op) = op {
                if !self.pinned && !items.iter().any(|i| i.action == MenuAction::OpenOperation(id)) {
                    items.push(item(trs!("Open the operation", "Открыть операцию"), MenuAction::OpenOperation(id)));
                }
                if op.state == "REVERSED" {
                    items.push(item(trs!("Restore the operation", "Вернуть операцию"), MenuAction::RestoreOperation(id)));
                } else {
                    let mut remove = item(trs!("Remove — a duplicate or a mistake", "Убрать — это дубль или ошибка"), MenuAction::RemoveOperation(id));
                    remove.confirm = Some(tr!(
                        "Remove {} of {}{}? It stops counting everywhere; it can be restored", "Убрать {} от {}{}? Перестанет считаться везде, вернуть можно",
                        op.line.amount,
                        op.line.occurred_on,
                        if op.pair.is_some() { trs!(" together with the other transfer leg", " вместе со второй ногой перевода") } else { "" }
                    ));
                    items.push(remove);
                }
                if op.state == "POSTED" && op.role == "transfer" {
                    let mut unpair = item(trs!("Not a transfer — split the pair", "Это не перевод — разорвать пару"), MenuAction::UnpairTransfer(id));
                    unpair.confirm = Some(
                        trs!("Split the transfer? Both legs become an expense and an income and enter the flows", "Разорвать перевод? Обе ноги станут расходом и доходом и попадут в потоки").into(),
                    );
                    items.push(unpair);
                } else if op.state == "POSTED" && matches!(op.role.as_str(), "income" | "expense") {
                    items.push(item(trs!("A transfer between own accounts — find the other leg…", "Это перевод между своими — найти вторую ногу…"), MenuAction::FindPair(id)));
                }
                if op.state == "POSTED" && matches!(op.role.as_str(), "income" | "expense") {
                    items.push(item(trs!("Fix the amount, date or who…", "Исправить сумму, дату или кто…"), MenuAction::Fix(id)));
                    items.push(item(trs!("Link to a debt…", "Привязать к долгу…"), MenuAction::PickDebt(id)));
                    items.push(item(trs!("Split by category…", "Разбить по категориям…"), MenuAction::Ask(Ask::Split(id))));
                }
                if op.state == "POSTED" {
                    use potok_core::edit::Counting;
                    let kind = potok_core::edit::kind(&op.role, op.line.amount.minor());
                    let counted = matches!(kind, "income" | "expense" | "refund");
                    let options: &[(&str, &str, Counting)] = &[
                        ("income", trs!("A refund — money back from a purchase, off its spending", "Возврат — деньги за покупку, уменьшает траты"), Counting::Refund),
                        ("refund", trs!("Not a refund — income", "Не возврат — доход"), Counting::Counted),
                        ("income|expense|refund", trs!("Do not count in flows — not spending, not earning", "Не считать в потоках — не трата и не заработок"), Counting::Excluded),
                        ("passthrough", trs!("Do not count in flows", "Не считать в потоках"), Counting::Excluded),
                        ("income|expense|refund|excluded", trs!("Transit — passed through me", "Транзит — прошло через меня"), Counting::Passthrough),
                        ("excluded|passthrough", trs!("Count again — expense or income", "Считать снова — расход или доход"), Counting::Counted),
                    ];
                    if counted || matches!(kind, "excluded" | "passthrough") {
                        for (roles, label, counting) in options {
                            if roles.split('|').any(|r| r == kind) {
                                // While there are marks, the item says whom it
                                // is for: otherwise «Transit» beside «Marked
                                // (3)…» reads as «make them transit».
                                let label = if self.marked.is_empty() {
                                    label.to_string()
                                } else {
                                    tr!("This row only: {}", "Только эта строка: {}", label.to_lowercase())
                                };
                                items.push(item(&label, MenuAction::Count(id, *counting)));
                            }
                        }
                    }
                }
                items.push(item(trs!("A note of your own…", "Своя заметка…"), MenuAction::Ask(Ask::OperationNote(id))));
                items.push(item(trs!("A category for this operation only…", "Категория только для этой операции…"), MenuAction::OwnCategory(id)));
                if op.category_source == Some(detail::CategorySource::Own) {
                    items.push(item(trs!("Drop its own category — take the counterparty's", "Снять свою категорию — брать у контрагента"), MenuAction::ClearOwnCategory(id)));
                }
                if let Some(name) = op.line.counterparty.clone().filter(|n| !n.is_empty()) {
                    // Marking the whole counterparty sits where marking one row
                    // sits: that is what people come here for, having seen
                    // forty-eight identical «Other»s.
                    items.push(item(
                        &tr!("A category for every operation of «{name}»…", "Категория всем операциям «{name}»…"),
                        MenuAction::CategorizeCounterparty(name.clone()),
                    ));
                    items.push(item(&tr!("All operations of «{name}»", "Все операции «{name}»"), MenuAction::CounterpartyOperations(name.clone())));
                    items.push(item(&tr!("Rename «{name}»…", "Переименовать «{name}»…"), MenuAction::Ask(Ask::Rename(name.clone()))));
                    items.push(item(&tr!("A note on «{name}»…", "Заметка на «{name}»…"), MenuAction::Ask(Ask::Note(name))));
                }
                items.push(item(trs!("This is another counterparty…", "Это другой контрагент…"), MenuAction::Ask(Ask::Respell(id))));
            }
            return Ok(items);
        }

        match self.screen {
            Screen::Statements => {
                items.push(item(trs!("Import a statement…", "Загрузить выписку…"), MenuAction::ImportPath));
                // The one under the cursor is rolled back: the move from saldo is
                // not covered — it is no statement file, and there is nowhere to return it.
                if let Some(file) = self.imports.get(self.cursor()).filter(|f| f.bank != "saldo-migration") {
                    let mut undo = item(&tr!("Roll back the import «{}»", "Откатить загрузку «{}»", file.filename), MenuAction::Unimport(file.id));
                    undo.confirm = Some(tr!(
                        "Roll back «{}» whole? {} operations disappear; the file can be imported again", "Откатить «{}» целиком? {} операций исчезнут, файл можно загрузить снова",
                        file.filename, file.operations
                    ));
                    items.push(undo);
                }
            }
            Screen::Counterparties => {
                items.push(item(trs!("Import a statement…", "Загрузить выписку…"), MenuAction::ImportPath));
                if let Some(line) = self.selected_counterparty() {
                    items.push(item(trs!("Categorise…", "Разметить…"), MenuAction::CategorizeCounterparty(line.name.clone())));
                    items.push(item(trs!("All its operations", "Все его операции"), MenuAction::CounterpartyOperations(line.name.clone())));
                    items.push(item(trs!("Rename…", "Переименовать…"), MenuAction::Ask(Ask::Rename(line.name.clone()))));
                    items.push(item(trs!("A note of your own…", "Своя заметка…"), MenuAction::Ask(Ask::Note(line.name.clone()))));
                    items.push(item(trs!("Merge into another…", "Свести в другого…"), MenuAction::Ask(Ask::MergeInto(line.name.clone()))));
                    items.push(item(trs!("Add a spelling…", "Добавить написание…"), MenuAction::Ask(Ask::Alias(line.name.clone()))));
                    if let Some(DetailItem::Alias(alias)) = self.detail_items().get(self.detail_cursor) {
                        if self.focus == Focus::Detail {
                            let mut drop = item(&tr!("Remove the spelling «{alias}»", "Убрать написание «{alias}»"), MenuAction::DropAlias(alias.clone()));
                            drop.confirm = Some(tr!(
                                "Remove «{alias}»? Its operations stay where they are; the next statement that writes it that way will make a counterparty of its own",
                                "Убрать «{alias}»? Операции останутся на месте; следующая выписка с таким написанием заведёт своего контрагента"
                            ));
                            items.push(drop);
                        }
                    }
                }
                let sure = potok_core::counterparty::confident_hints(&self.conn, HINT_PERCENT)?.len();
                if sure > 0 {
                    let mut take = item(
                        &tr!("Take every hint at {HINT_PERCENT}% or more — {sure} counterparties", "Принять все подсказки от {HINT_PERCENT}% — контрагентов: {sure}"),
                        MenuAction::TakeHints,
                    );
                    take.confirm = Some(tr!(
                        "Categorise {sure} counterparties by their hint? Each is a line in Changes and can be undone",
                        "Разметить {sure} контрагентов по подсказке? Каждый — строка в «Правках», отменяется"
                    ));
                    items.push(take);
                }
                let refunds = potok_core::counterparty::refunds_counted_as_income(&self.conn)?.len();
                if refunds > 0 {
                    let mut take = item(
                        &tr!("Count {refunds} refunds as refunds, not income", "Считать возвратами, а не доходом: {refunds}"),
                        MenuAction::TakeRefunds,
                    );
                    take.confirm = Some(tr!(
                        "Count {refunds} refunds against their shop's spending? Each is a line in Changes and can be undone",
                        "Вычесть {refunds} возвратов из трат их магазинов? Каждый — строка в «Правках», отменяется"
                    ));
                    items.push(take);
                }
            }
            Screen::Accounts => {
                if let Some(account) = self.accounts.get(self.cursor()) {
                    items.push(item(trs!("All operations of the account", "Все операции счёта"), MenuAction::AccountOperations(account.id)));
                    items.push(item(trs!("Opening balance…", "Начальный остаток…"), MenuAction::Opening(account.id)));
                    items.push(item(trs!("Rename…", "Переименовать…"), MenuAction::Ask(Ask::RenameAccount(account.id))));
                    if account.balance.minor() == 0 {
                        let mut archive = item(trs!("Archive — hide it", "В архив — убрать с глаз"), MenuAction::Archive(account.id, true));
                        archive.confirm = Some(tr!(
                            "Archive «{}»? Its operations stay; it comes back from «Archived accounts…»",
                            "Убрать «{}» в архив? Операции останутся; вернуть — «Счета в архиве…»",
                            account.name
                        ));
                        items.push(archive);
                    }
                }
                items.push(item(trs!("New account…", "Новый счёт…"), MenuAction::NewAccount));
                items.push(item(trs!("New transfer between own accounts…", "Новый перевод между своими…"), MenuAction::NewTransfer));
                let archived = potok_core::account::archived(&self.conn).unwrap_or_default();
                if !archived.is_empty() {
                    items.push(item(
                        &tr!("Archived accounts ({})…", "Счета в архиве ({})…", archived.len()),
                        MenuAction::ShowArchived,
                    ));
                }
            }
            Screen::Flows => items.push(item(trs!("Operations of this row", "Операции этой строки"), MenuAction::Drill)),
            Screen::Home if self.home_pane == HomePane::Months && self.focus == Focus::List => {
                items.push(item(trs!("Operations of this month", "Операции этого месяца"), MenuAction::Drill));
                if let Some(m) = self.home_months().get(self.home_month_at()) {
                    let key = self.month_rule.cycle(m.start, 0).key;
                    items.push(item(trs!("Correct when this month starts…", "Поправить начало этого месяца…"), MenuAction::Ask(Ask::MonthStart(key))));
                }
            }
            Screen::Home => items.push(item(trs!("Operations of this row", "Операции этой строки"), MenuAction::Drill)),
            Screen::Debts => {
                items.push(item(trs!("Expand", "Развернуть"), MenuAction::Zoom));
                if let Some(debt) = self.debts.get(self.cursor()) {
                    items.push(item(trs!("Edit — direction, due date, note…", "Править — направление, срок, заметку…"), MenuAction::EditDebt(debt.id)));
                    let mut delete = item(trs!("Delete the debt", "Удалить долг"), MenuAction::DeleteDebt(debt.id));
                    delete.confirm = Some(tr!(
                        "Delete the debt «{}»? Its operations stay, as plain expenses and incomes",
                        "Удалить долг «{}»? Операции останутся обычными расходами и доходами",
                        debt.counterparty
                    ));
                    items.push(delete);
                }
                if let Some(debt) = self.debts.get(self.cursor()).filter(|d| d.remaining.minor() > 0) {
                    let mut off = item(trs!("Write off what is left — forgiven", "Списать остаток — простили или простил"), MenuAction::WriteOff(debt.id));
                    off.confirm = Some(tr!(
                        "Write off {} on the debt «{}»? No money moved — the debt closes", "Списать {} по долгу «{}»? Денег не было — долг закроется",
                        debt.remaining, debt.counterparty
                    ));
                    items.push(off);
                }
            }
            Screen::Duplicates => {
                if let Some(group) = self.duplicates.get(self.cursor()) {
                    // Which copy to remove is a person's call: copies come with
                    // different descriptions and roles, and the label shows it.
                    for op in &group.operations {
                        let mut remove = item(
                            &tr!("Remove #{} — {} · {}", "Убрать #{} — {} · {}", op.id, op.description, op.role),
                            MenuAction::RemoveOperation(op.id),
                        );
                        remove.confirm = Some(tr!(
                            "Remove #{} {} of {}? It stops counting everywhere; it can be restored", "Убрать #{} {} от {}? Перестанет считаться везде, вернуть можно",
                            op.id, op.amount, op.occurred_on
                        ));
                        items.push(remove);
                    }
                    items.push(item(
                        trs!("Not a duplicate — stop showing it", "Это не дубль — больше не показывать"),
                        MenuAction::DismissDuplicates(group.key.clone()),
                    ));
                    // Monthly fees and the like: the same pair every month is
                    // one decision, not twelve.
                    let first = &group.operations[0];
                    let like: Vec<String> = self
                        .duplicates
                        .iter()
                        .filter(|g| {
                            let o = &g.operations[0];
                            o.account == first.account && o.amount == first.amount && o.counterparty == first.counterparty
                        })
                        .map(|g| g.key.clone())
                        .collect();
                    if like.len() > 1 {
                        items.push(item(
                            &tr!(
                                "None of the {} groups like this is a duplicate",
                                "Ни одна из {} таких групп не дубль",
                                like.len()
                            ),
                            MenuAction::DismissAll(like),
                        ));
                    }
                }
            }
            Screen::Journal => {
                if let Some(change) = self.changes.get(self.cursor()).filter(|c| c.can_undo && c.undone_at.is_none()) {
                    let mut undo = item(trs!("Undo this change", "Отменить эту правку"), MenuAction::Undo(change.id));
                    undo.confirm = Some(tr!("Undo «{}»?", "Отменить «{}»?", change.what));
                    items.push(undo);
                }
            }
            Screen::Categories => {
                if let Some(line) = self.category_lines.get(self.cursor()) {
                    let name = line.name.clone();
                    items.push(item(trs!("All its operations", "Все её операции"), MenuAction::Drill));
                    items.push(item(trs!("Rename…", "Переименовать…"), MenuAction::Ask(Ask::RenameCategory(name.clone()))));
                    items.push(item(trs!("Merge into another…", "Свести в другую…"), MenuAction::Ask(Ask::MergeCategory(name.clone()))));
                    items.push(item(trs!("Monthly budget…", "Бюджет на месяц…"), MenuAction::Ask(Ask::Budget(name.clone()))));
                    let mut delete = item(trs!("Delete the category", "Удалить категорию"), MenuAction::DeleteCategory(name.clone()));
                    delete.confirm = Some(tr!(
                        "Delete «{name}»? {} counterparties and the operations in it are left without a category",
                        "Удалить «{name}»? {} контрагентов и её операции останутся без категории",
                        line.counterparties
                    ));
                    items.push(delete);
                }
            }
            Screen::Budget => {
                if let Some(line) = self.budget_lines.get(self.cursor()) {
                    items.push(item(trs!("Operations this month", "Операции этого месяца"), MenuAction::Drill));
                    items.push(item(trs!("Change the limit…", "Поменять лимит…"), MenuAction::Ask(Ask::Budget(line.category.clone()))));
                }
                items.push(item(trs!("Budgets are set on a category — 9, x, «Monthly budget…»", "Бюджет ставится на категорию — 9, x, «Бюджет на месяц…»"), MenuAction::GoCategories));
            }
            Screen::Analytics if self.analytics_pane == AnalyticsPane::Regular => {
                if let Some(r) = self.regular.get(self.regular_cursor) {
                    items.push(item(
                        &tr!("All operations of «{}»", "Все операции «{}»", r.who),
                        MenuAction::CounterpartyOperations(r.who.clone()),
                    ));
                    items.push(item(
                        &tr!("Category for «{}»…", "Категория для «{}»…", r.who),
                        MenuAction::CategorizeCounterparty(r.who.clone()),
                    ));
                    items.push(item(
                        &tr!("Rename «{}»…", "Переименовать «{}»…", r.who),
                        MenuAction::Ask(Ask::Rename(r.who.clone())),
                    ));
                    items.push(item(trs!("A note of your own…", "Своя заметка…"), MenuAction::Ask(Ask::Note(r.who.clone()))));
                    items.push(item(trs!("Add a spelling…", "Добавить написание…"), MenuAction::Ask(Ask::Alias(r.who.clone()))));
                }
            }
            Screen::Analytics => {
                if let Some(m) = self.months.iter().rev().nth(self.month_cursor()) {
                    let key = self.month_rule.cycle(m.start, 0).key;
                    items.push(item(trs!("Operations of this month", "Операции этого месяца"), MenuAction::Drill));
                    items.push(item(trs!("Correct when this month starts…", "Поправить начало этого месяца…"), MenuAction::Ask(Ask::MonthStart(key))));
                }
            }
            Screen::Operations if self.fold => {
                if let Some(group) = self.groups().get(self.cursor()).filter(|g| !g.name.is_empty() && g.name != "—") {
                    items.push(item(trs!("All its operations", "Все его операции"), MenuAction::CounterpartyOperations(group.name.clone())));
                    items.push(item(trs!("Categorise…", "Разметить…"), MenuAction::CategorizeCounterparty(group.name.clone())));
                    items.push(item(trs!("Rename…", "Переименовать…"), MenuAction::Ask(Ask::Rename(group.name.clone()))));
                    items.push(item(trs!("A note of your own…", "Своя заметка…"), MenuAction::Ask(Ask::Note(group.name.clone()))));
                    items.push(item(trs!("Add a spelling…", "Добавить написание…"), MenuAction::Ask(Ask::Alias(group.name.clone()))));
                }
            }
            Screen::Operations | Screen::Settings => {}
        }
        if self.screen != Screen::Counterparties && self.focus == Focus::Detail {
            if let Some(DetailItem::Alias(alias)) = self.detail_items().get(self.detail_cursor) {
                let mut drop = item(&tr!("Remove the spelling «{alias}»", "Убрать написание «{alias}»"), MenuAction::DropAlias(alias.clone()));
                drop.confirm = Some(tr!(
                    "Remove «{alias}»? Its operations stay where they are; the next statement that writes it that way will make a counterparty of its own",
                    "Убрать «{alias}»? Операции останутся на месте; следующая выписка с таким написанием заведёт своего контрагента"
                ));
                items.push(drop);
            }
        }
        // Last: x was pressed on one row, so what acts on the whole list
        // comes after what acts on the row — it used to come first.
        if self.screen == Screen::Operations && self.focus == Focus::List {
            items.push(item(&tr!("Export these {} to CSV", "Выгрузить эти {} в CSV", self.visible_operations().len()), MenuAction::ExportCsv));
        }
        Ok(items)
    }

    /// `x`: open the menu. Empty — say so rather than open an empty window.
    pub fn open_menu(&mut self) -> Result<()> {
        self.menu = self.menu_items()?;
        if self.read_only.is_some() {
            let offered = self.menu.len();
            self.menu.retain(|item| !item.action.writes());
            if self.menu.is_empty() && offered > 0 {
                self.status = self.read_only_note();
                return Ok(());
            }
        }
        if self.menu.is_empty() {
            self.status = Some(trs!("nothing to act on here", "здесь действовать не над чем").into());
        } else {
            self.mode = Mode::Menu { selected: 0 };
        }
        Ok(())
    }

    pub fn move_menu(&mut self, by: isize) {
        if let Mode::Menu { selected } = self.mode {
            let count = self.menu.len().max(1);
            self.mode = Mode::Menu { selected: (selected as isize + by).clamp(0, count as isize - 1) as usize };
        }
    }

    /// Enter in the menu: do it, or ask «yes or no».
    pub fn choose_menu(&mut self) -> Result<()> {
        let Mode::Menu { selected } = self.mode else { return Ok(()) };
        self.mode = Mode::Normal;
        let Some(item) = self.menu.get(selected).cloned() else { return Ok(()) };
        if item.confirm.is_some() {
            self.pending = Some(item);
            self.mode = Mode::Confirm;
            return Ok(());
        }
        self.run_reporting(item.action)
    }

    /// A refusal from the core — «these two cannot be a transfer», «already
    /// removed» — is an answer in the status line, not a reason to quit.
    pub(super) fn run_reporting(&mut self, action: MenuAction) -> Result<()> {
        if self.read_only.is_some() && action.writes() {
            self.status = self.read_only_note();
            return Ok(());
        }
        if let Err(error) = self.run(action) {
            self.status = Some(self.explain(&error));
        }
        Ok(())
    }

    /// The answer to «yes or no».
    pub fn answer(&mut self, yes: bool) -> Result<()> {
        self.mode = Mode::Normal;
        let pending = self.pending.take();
        match (yes, pending) {
            (true, Some(item)) => self.run_reporting(item.action),
            // «No» to «move the rest?» — this operation's edit is already done,
            // and saying «nothing changed» would be untrue.
            (false, Some(item)) if matches!(item.action, MenuAction::PromoteCategory { .. }) => {
                self.status = Some(trs!("left on this operation only", "оставлено только на этой операции").into());
                Ok(())
            }
            // «No» to «write it?» — back into the parsing rather than out of it.
            (false, Some(item)) if matches!(item.action, MenuAction::WriteImport | MenuAction::DropReview) => {
                self.mode = Mode::Review;
                Ok(())
            }
            _ => {
                self.status = Some(trs!("cancelled — nothing changed", "отменено — ничего не изменилось").into());
                Ok(())
            }
        }
    }

    /// While another device writes: who, and how to take writing over.
    pub fn read_only_note(&self) -> Option<String> {
        self.read_only.as_ref().map(|mark| {
            tr!(
                "read only — {} writes · W takes writing over",
                "только чтение — пишет {} · W — забрать запись",
                mark.device
            )
        })
    }

}
