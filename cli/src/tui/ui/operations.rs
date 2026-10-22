//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

pub(super) fn draw_operations(frame: &mut Frame, area: Rect, app: &App) {
    let shown = app.visible_operations();
    let title = match &app.ops_view {
        OpsView::All => tr!("Operations — {} ({})", "Операции — {} ({})", app.account_filter_label(), shown.len()),
        OpsView::Import { name, .. } => {
            tr!("Operations — {name} ({}) · Esc — to the statements", "Операции — {name} ({}) · Esc — к выпискам", shown.len())
        }
        // Esc leads where one drilled in from: from the Overview — to the Overview.
        OpsView::Drill { label, from, to } => tr!(
            "Operations — {label} · {}–{} ({}) · Esc — {}", "Операции — {label} · {}–{} ({}) · Esc — {}",
            short(from),
            short(to),
            shown.len(),
            if app.return_to == Some(Screen::Home) {
                trs!("to the overview", "на обзор")
            } else {
                trs!("to flows", "к потокам")
            }
        ),
        OpsView::Search { text } => tr!(
            "Operations — search «{text}» ({}{}) · Esc — whole journal", "Операции — поиск «{text}» ({}{}) · Esc — весь журнал",
            shown.len(),
            if app.view_truncated { "+" } else { "" }
        ),
        OpsView::Account { name, .. } => tr!("Operations — {name} ({}) · Esc — to accounts", "Операции — {name} ({}) · Esc — к счетам", shown.len()),
        OpsView::Counterparty { name } => tr!("Operations — {name} ({}) · Esc — back", "Операции — {name} ({}) · Esc — назад", shown.len()),
        OpsView::Range { from, to } => tr!("Operations — {}–{} ({}) · Esc — back", "Операции — {}–{} ({}) · Esc — назад", short(from), short(to), shown.len()),
    };
    let mut title = title;
    match app.order {
        potok_core::query::Order::Date => {}
        potok_core::query::Order::Largest => title.push_str(trs!(" · largest first", " · крупные сверху")),
        potok_core::query::Order::Smallest => title.push_str(trs!(" · smallest first", " · мелкие сверху")),
    }
    if !app.marked.is_empty() {
        title.push_str(&tr!(" · marked {} — x", " · отмечено {} — x", app.marked.len()));
    }
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.fold {
        draw_groups(frame, inner, app);
        return;
    }
    if shown.is_empty() {
        let why = match &app.ops_view {
            OpsView::All => trs!("no operations. potok import <file> --account <account>", "операций нет. potok import <файл> --account <счёт>"),
            OpsView::Drill { .. } => trs!("no operations in this row for this period", "в этой строке за этот период операций нет"),
            OpsView::Search { .. } => trs!("nothing found — search looks at description, counterparty and amount", "ничего не нашлось — ищется по описанию, контрагенту и сумме"),
            OpsView::Account { .. } | OpsView::Counterparty { .. } | OpsView::Range { .. } => trs!("no operations", "операций нет"),
            OpsView::Import { .. } => trs!("this statement wrote nothing", "эта выписка ничего не записала"),
        };
        empty(frame, inner, why);
        return;
    }
    let width = inner.width as usize;
    // Days head the journal when it is in date order: the date is said once,
    // with what the day cost, instead of ten columns on every row.
    let by_day = app.order == potok_core::query::Order::Date;
    let spent = if by_day { potok_core::query::spent_by_day(shown) } else { Default::default() };
    enum Row {
        Day(usize),
        Op(usize),
    }
    let mut rows = Vec::with_capacity(shown.len() + 64);
    let mut cursor_row = 0;
    for (index, op) in shown.iter().enumerate() {
        if by_day && (index == 0 || shown[index - 1].occurred_on != op.occurred_on) {
            rows.push(Row::Day(index));
        }
        if index == app.cursor() {
            cursor_row = rows.len();
        }
        rows.push(Row::Op(index));
    }
    // " ⇄" after the amount is reserved on every row, so amounts line up.
    let amount_width = 14;
    let account_width = if width >= 70 { 11 } else { 0 };
    let category_width = if width >= 60 { 19 } else { 0 };
    let who_width = width.saturating_sub(3 + category_width + account_width + amount_width + 2).max(6);
    let (from, to) = window(cursor_row, rows.len(), inner.height as usize);
    let lines: Vec<Line> = rows[from..to]
        .iter()
        .map(|row| match *row {
            Row::Day(index) => {
                let day = &shown[index].occurred_on;
                let label = day_label(day);
                let total = spent.get(day).filter(|b| !b.is_empty()).map(bag_text).unwrap_or_default();
                // The day's total ends where the amounts end, before « ⇄».
                let rule = width.saturating_sub(label.chars().count() + total.chars().count() + 4);
                Line::from(vec![
                    Span::styled(format!("{label} "), theme::heading()),
                    Span::styled("─".repeat(rule), theme::border()),
                    Span::styled(format!(" {total}  "), theme::label()),
                ])
            }
            Row::Op(index) => {
                let op = &shown[index];
                let selected = index == app.cursor();
                // The row's own colours, and on the cursor the same colours on
                // the cursor's background.
                let pick = |base: Style| if selected { theme::on_cursor(base) } else { base };
                let mark = if app.marked.contains(&op.id) { "•" } else { " " };
                let transfer = op.role == "transfer" && !op.removed;
                let (who, who_style) = if transfer {
                    let route = match (&op.pair_account, op.amount.minor() < 0) {
                        (Some(other), true) => format!("{} → {other}", op.account),
                        (Some(other), false) => format!("{other} → {}", op.account),
                        (None, _) => op.account.clone(),
                    };
                    (route, theme::label())
                } else {
                    let name = op.counterparty.as_deref().filter(|n| !n.is_empty()).unwrap_or(&op.description).to_string();
                    (name, if op.removed { theme::label() } else { theme::value() })
                };
                let category = if transfer { trs!("transfer", "перевод").to_string() } else { kind_column(op) };
                let category_style = if !op.removed && !transfer && op.category.is_none() && op.role == "expense" {
                    // Not sorted yet: the one category worth noticing.
                    theme::attention()
                } else {
                    theme::label()
                };
                let (amount, amount_style, tail) = if transfer {
                    (op.amount.abs().to_string(), theme::label(), " ⇄")
                } else if op.removed {
                    (op.amount.to_string(), theme::label(), "  ")
                } else {
                    (op.amount.to_string(), theme::money(op.amount.minor()), "  ")
                };
                let mut spans = vec![
                    Span::styled(format!("{mark}  "), pick(theme::label())),
                    Span::styled(format!("{:<w$}", fit(&who, who_width), w = who_width), pick(who_style)),
                ];
                if category_width > 0 {
                    spans.push(Span::styled(format!(" {:<w$}", fit(&category, category_width - 1), w = category_width - 1), pick(category_style)));
                }
                if account_width > 0 {
                    let account = if transfer { String::new() } else { op.account.clone() };
                    spans.push(Span::styled(format!(" {:<w$}", fit(&account, account_width - 1), w = account_width - 1), pick(theme::label())));
                }
                spans.push(Span::styled(format!("{amount:>amount_width$}"), pick(amount_style)));
                spans.push(Span::styled(tail, pick(theme::label())));
                Line::from(spans)
            }
        })
        .collect();
    rows_of(frame, inner, lines);
}

/// The journal folded by counterparty: who, how many times, how much.
pub(super) fn draw_groups(frame: &mut Frame, inner: Rect, app: &App) {
    let groups = app.groups();
    let count = tr!("  {:>4} ops", "  {:>4} опер", 0).chars().count();
    let name_width = (inner.width as usize).saturating_sub(18 + count).max(6);
    let (from, to) = window(app.cursor(), groups.len(), inner.height as usize);
    let lines: Vec<Line> = groups[from..to]
        .iter()
        .enumerate()
        .map(|(offset, g)| {
            let selected = from + offset == app.cursor();
            let pick = |base: Style| if selected { theme::selected() } else { base };
            Line::from(vec![
                Span::styled(format!("{:<w$}", fit(&g.name, name_width), w = name_width), pick(theme::value())),
                Span::styled(format!(" {:>17}", bag_text(&g.sum)), pick(theme::money(first_minor(&g.sum)))),
                Span::styled(tr!("  {:>4} ops", "  {:>4} опер", g.count), pick(theme::label())),
            ])
        })
        .collect();
    rows_of(frame, inner, lines);
}

/// `2026-10-16` → «Fri 16 Oct» / «пт 16 окт».
pub(super) fn day_label(date: &str) -> String {
    let Ok(day) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else { return date.to_string() };
    use chrono::Datelike;
    let weekday = day.weekday().num_days_from_monday() as usize;
    let month = day.month0() as usize;
    let (days, months) = match potok_core::i18n::lang() {
        potok_core::i18n::Lang::Ru => (
            ["пн", "вт", "ср", "чт", "пт", "сб", "вс"],
            ["янв", "фев", "мар", "апр", "мая", "июн", "июл", "авг", "сен", "окт", "ноя", "дек"],
        ),
        _ => (
            ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
        ),
    };
    if day.year() == chrono::Local::now().year() {
        format!("{} {} {}", days[weekday], day.day(), months[month])
    } else {
        format!("{} {} {} {}", days[weekday], day.day(), months[month], day.year())
    }
}

/// The change in spend against the previous period — and the colour: spending
/// more is red, less is green. The sign of a number, not a money computation:
/// the core counted the per cent.
pub(super) fn change_text(change: Change) -> (String, Style) {
    match change {
        Change::Percent(0) => ("0%".into(), theme::label()),
        Change::Percent(p) if p > 0 => (format!("+{p}%"), theme::error()),
        Change::Percent(p) => (format!("−{}%", -p), theme::ok()),
        Change::New => (trs!("new", "новое").into(), theme::error()),
        Change::Gone => (trs!("gone", "пропало").into(), theme::ok()),
        Change::Nothing => (String::new(), theme::label()),
        Change::NotComparable => (trs!("≠ currencies", "≠ валюты").into(), theme::label()),
    }
}
