//! The input forms: an operation, a transfer, an account, a debt.
use super::*;

impl App {
    /// `/`: on the import screen it searches the counterparties, on any other
    /// screen the operations, and then moves to the operations screen.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn operation_form(
        &self,
        replacing: Option<i64>,
        account: usize,
        income: bool,
        amount: String,
        date: String,
        who: String,
        category: String,
    ) -> Form {
        Form {
            kind: FormKind::Operation { replacing },
            title: match replacing {
                Some(id) => tr!("Fix operation #{id} — the old one is removed; it can be restored", "Исправить операцию #{id} — старая уберётся, вернуть можно"),
                None => trs!("New operation", "Новая операция").into(),
            },
            fields: vec![
                Field::choice("account", trs!("account", "счёт"), self.accounts.iter().map(|a| a.name.clone()).collect(), account),
                Field::choice("kind", trs!("kind", "что"), vec![trs!("expense — money out", "расход — ушло").into(), trs!("income — money in", "доход — пришло").into()], usize::from(income)),
                Field::text("amount", trs!("amount", "сумма"), amount),
                Field::text("date", trs!("date", "дата"), date),
                Field::text("who", trs!("who", "кто"), who),
                Field::optional("category", trs!("category", "категория"), category, trs!("— from the counterparty", "— от контрагента")),
            ],
            field: 2,
            error: None,
        }
    }

    pub(super) fn debt_form(&self, with_operation: Option<i64>, who: String, owe: bool, currency: &str, date: String) -> Form {
        let mut currencies: Vec<String> = self.accounts.iter().map(|a| a.balance.currency().as_str().to_string()).collect();
        currencies.sort();
        currencies.dedup();
        let chosen = currencies.iter().position(|c| c == currency).unwrap_or(0);
        Form {
            kind: FormKind::Debt { with_operation },
            title: match with_operation {
                Some(id) => tr!("New debt — operation #{id} becomes its first issue", "Новый долг — операция #{id} станет первой выдачей"),
                None => trs!("New debt", "Новый долг").into(),
            },
            fields: vec![
                Field::text("who", trs!("who", "кто"), who),
                Field::choice("direction", trs!("direction", "кто кому"), vec![trs!("I owe", "я должен").into(), trs!("owed to me", "мне должны").into()], usize::from(!owe)),
                Field::optional("amount", trs!("amount", "сумма"),
                    "",
                    if with_operation.is_some() { trs!("— on top of the operation, if any", "— сверх операции, если было") } else { trs!("— how much is owed already", "— сколько уже должно") },
                ),
                Field::choice("currency", trs!("currency", "валюта"), currencies, chosen),
                Field::text("opened", trs!("opened", "открыт"), date),
                Field::optional("due", trs!("due", "срок"), "", trs!("— no due date", "— без срока")),
                Field::optional("note", trs!("note", "заметка"), "", "—"),
            ],
            field: if with_operation.is_some() { 1 } else { 0 },
            error: None,
        }
    }

    /// `n`: a new operation. The account is the one under the cursor or in the
    /// journal's filter, else cash: that is what is most often entered by hand.
    /// On the debts screen — a new debt.
    pub fn start_form(&mut self) {
        if self.screen == Screen::Debts {
            self.form = Some(self.debt_form(None, String::new(), true, "EUR", self.today.to_string()));
            self.mode = Mode::Form;
            return;
        }
        if self.accounts.is_empty() {
            self.status = Some(trs!("no accounts — first potok account add <name>", "счетов нет — сначала potok account add <имя>").into());
            return;
        }
        let wanted = self.default_account.or(match self.screen {
            Screen::Accounts => self.accounts.get(self.cursor()).map(|a| a.id),
            _ => self.account_filter,
        });
        let account = wanted
            .and_then(|id| self.accounts.iter().position(|a| a.id == id))
            .or_else(|| self.accounts.iter().position(|a| a.name.eq_ignore_ascii_case("cash")))
            .unwrap_or(0);
        self.form = Some(self.operation_form(
            None,
            account,
            false,
            String::new(),
            self.today.to_string(),
            String::new(),
            String::new(),
        ));
        self.mode = Mode::Form;
    }

    pub fn form_move(&mut self, by: isize) {
        if let Some(form) = &mut self.form {
            form.field = (form.field as isize + by).rem_euclid(form.fields.len() as isize) as usize;
        }
    }

    /// ←→ on a choice field.
    pub fn form_choose(&mut self, by: isize) {
        if let Some(field) = self.form.as_mut().and_then(|f| f.fields.get_mut(f.field)) {
            if !field.choices.is_empty() {
                field.chosen = (field.chosen as isize + by).rem_euclid(field.choices.len() as isize) as usize;
            }
        }
    }

    pub fn form_type(&mut self, c: char) {
        if let Some(text) = self.form.as_mut().and_then(Form::text_mut) {
            text.push(c);
        }
    }

    pub fn form_backspace(&mut self) {
        if let Some(text) = self.form.as_mut().and_then(Form::text_mut) {
            text.pop();
        }
    }

    pub fn cancel_form(&mut self) {
        self.form = None;
        self.mode = Mode::Normal;
        self.status = Some(trs!("nothing written", "ничего не записано").into());
    }

    /// Enter in a form: write it. An error stays in the form, beside the fields
    /// — what was typed is not lost.
    pub fn save_form(&mut self) -> Result<()> {
        let Some(form) = self.form.clone() else { return Ok(()) };
        let saved = match &form.kind {
            FormKind::Operation { replacing } => self.save_operation(&form, *replacing),
            FormKind::Debt { with_operation } => self.save_debt(&form, *with_operation),
            FormKind::Account => self.save_account(&form),
            FormKind::DebtEdit(id) => self.save_debt_edit(&form, *id),
            FormKind::Transfer => self.save_transfer(&form),
        };
        match saved {
            Ok(message) => {
                self.form = None;
                self.mode = Mode::Normal;
                self.status = Some(message);
                self.pinned = false;
                self.zoom = false;
                self.reload()
            }
            Err(error) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(error.to_string());
                }
                Ok(())
            }
        }
    }

    pub(super) fn save_operation(&mut self, form: &Form, replacing: Option<i64>) -> Result<String> {
        let account = self.accounts.get(form.chosen("account")).ok_or_else(|| anyhow::anyhow!(tr!("no account", "нет счёта")))?;
        let minor = potok_core::reconcile::parse_input(form.value("amount").trim_start_matches('+'))?.abs();
        let new = potok_core::edit::NewOperation {
            account_id: account.id,
            occurred_on: potok_core::edit::parse_date(form.value("date"), self.today)?,
            effect_minor: if form.chosen("kind") == 1 { minor } else { -minor },
            who: form.value("who").to_string(),
            category: Some(form.value("category").to_string()),
        };
        Ok(match replacing {
            Some(old) => {
                let id = potok_core::edit::replace_operation(&mut self.conn, old, &new)?;
                tr!("fixed: #{old} removed, #{id} written · restore the old one — x on it", "исправлено: #{old} убрана, записана #{id} · вернуть старую — x на ней")
            }
            None => tr!("written #{}", "записана #{}", potok_core::edit::add_operation(&mut self.conn, &new)?),
        })
    }

    pub(super) fn save_debt_edit(&mut self, form: &Form, id: i64) -> Result<String> {
        let due = form.value("due").trim();
        let due_on = if due.is_empty() { None } else { Some(potok_core::edit::parse_date(due, self.today)?.to_string()) };
        let fields = potok_core::debt::DebtFields {
            direction: if form.chosen("direction") == 0 { "I_OWE" } else { "OWED_TO_ME" }.into(),
            due_on,
            note: Some(form.value("note").to_string()),
        };
        potok_core::debt::update(&self.conn, id, &fields)?;
        Ok(tr!("debt #{id} saved", "долг #{id} сохранён"))
    }

    pub(super) fn save_transfer(&mut self, form: &Form) -> Result<String> {
        let from = self.accounts.get(form.chosen("from")).map(|a| a.id).ok_or_else(|| anyhow::anyhow!("no account"))?;
        let to = self.accounts.get(form.chosen("to")).map(|a| a.id).ok_or_else(|| anyhow::anyhow!("no account"))?;
        let sent = potok_core::reconcile::parse_input(form.value("amount"))?.abs();
        let received = match form.value("received").trim() {
            "" => sent,
            text => potok_core::reconcile::parse_input(text)?.abs(),
        };
        let on = potok_core::edit::parse_date(form.value("date"), self.today)?;
        potok_core::edit::add_transfer(&mut self.conn, from, to, sent, received, on, form.value("note"))?;
        Ok(tr!("transfer written", "перевод записан"))
    }

    pub(super) fn save_account(&mut self, form: &Form) -> Result<String> {
        let opening = form.value("opening").trim();
        let opening = if opening.is_empty() { 0 } else { potok_core::reconcile::parse_input(opening)? };
        let kind = potok_core::account::KINDS[form.chosen("kind")];
        let name = form.value("name").trim().to_string();
        potok_core::account::create(&self.conn, &name, form.choice("currency"), kind, opening)?;
        Ok(tr!("account «{name}» created", "создан счёт «{name}»"))
    }

    pub(super) fn save_debt(&mut self, form: &Form, with_operation: Option<i64>) -> Result<String> {
        let amount = form.value("amount").trim();
        let due = form.value("due").trim();
        let note = form.value("note").trim();
        let new = potok_core::debt::NewDebt {
            who: form.value("who").to_string(),
            direction: if form.chosen("direction") == 0 { "I_OWE" } else { "OWED_TO_ME" }.into(),
            currency: form.choice("currency").to_string(),
            opened_on: potok_core::edit::parse_date(form.value("opened"), self.today)?,
            due_on: if due.is_empty() { None } else { Some(potok_core::edit::parse_date(due, self.today)?) },
            opening_minor: if amount.is_empty() { 0 } else { potok_core::reconcile::parse_input(amount)?.abs() },
            note: (!note.is_empty()).then(|| note.to_string()),
        };
        let id = potok_core::debt::open(&mut self.conn, &new, with_operation)?;
        Ok(tr!("debt #{id} opened: {}", "открыт долг #{id}: {}", new.who.trim()))
    }

    /// Enter while typing an opening balance.
    pub fn apply_opening(&mut self) -> Result<()> {
        self.mode = Mode::Normal;
        let Some(id) = self.opening_for.take() else { return Ok(()) };
        let done = potok_core::reconcile::parse_input(&self.input)
            .and_then(|minor| potok_core::edit::set_opening(&self.conn, id, minor).map(|before| (before, minor)));
        self.status = Some(match done {
            Ok((before, now)) => tr!(
                "opening balance: was {}, now {}",
                "начальный остаток: был {}, стал {}",
                potok_core::money::format_minor(before),
                potok_core::money::format_minor(now)
            ),
            Err(error) => tr!("not changed: {error}", "не изменено: {error}"),
        });
        self.reload()
    }

}
