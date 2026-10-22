//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Accounts: each with its year as a line and when the bank last agreed;
/// below, the one under the cursor over the last thirty days.
pub(super) fn draw_accounts(frame: &mut Frame, area: Rect, app: &App) {
    let list_height = (app.accounts.len() as u16 + 4).min(area.height.saturating_sub(6).max(4));
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(list_height), Constraint::Min(0)])
        .split(area);
    let title = tr!("Accounts · {}", "Счета · {}", total_text(app));
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(parts[0]);
    frame.render_widget(block, parts[0]);
    if app.accounts.is_empty() {
        empty(frame, inner, trs!("no accounts. potok account add <name> --currency EUR", "счетов нет. potok account add <имя> --currency EUR"));
        return;
    }
    let width = inner.width as usize;
    let (name_width, balance_width, line_width) = (14usize, 16usize, 12usize);
    let check_width = width.saturating_sub(name_width + balance_width + line_width + 4);
    let first = app.months.iter().rev().take(10).min_by_key(|m| m.start).map(|m| month_short(chrono::Datelike::month(&m.end))).unwrap_or("");
    let last = app.months.last().map(|m| month_short(chrono::Datelike::month(&m.end))).unwrap_or("");
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{:<name_width$}{:>balance_width$}  ", "", trs!("balance", "остаток")), theme::label()),
        Span::styled(format!("{:<line_width$}  ", format!("{first} ─ {last}")), theme::label()),
        Span::styled(fit(trs!("checked with the bank", "сверен с банком"), check_width), theme::label()),
    ])];
    let rows = (inner.height as usize).saturating_sub(1);
    let (from, to) = window(app.cursor(), app.accounts.len(), rows);
    for (index, account) in app.accounts.iter().enumerate().take(to).skip(from) {
        let selected = index == app.cursor();
        let pick = |base: Style| if selected { theme::on_cursor(base) } else { base };
        let view = app.account_views.get(index);
        let unused = account.operations == 0;
        let mut spans = vec![
            Span::styled(format!("{:<name_width$}", fit(&account.name, name_width - 1)), pick(if unused { theme::label() } else { theme::value() })),
            Span::styled(format!("{:>balance_width$}  ", account.balance.to_string()), pick(if unused { theme::label() } else { theme::strong() })),
        ];
        if unused {
            spans.push(Span::styled(format!("{:<w$}", trs!("not used yet · x — archive", "не используется · x — в архив"), w = line_width + 2 + check_width), pick(theme::label())));
        } else {
            let minors: Vec<i64> = view.map(|v| v.months.iter().map(|m| m.minor()).collect()).unwrap_or_default();
            spans.push(Span::styled(format!("{:<line_width$}  ", spark(&minors)), pick(theme::chart())));
            let (text, color) = match view.and_then(|v| v.reconciled.as_ref()) {
                Some(r) if r.matches() => (tr!("✓ {}", "✓ {}", r.on_date), theme::good()),
                Some(r) => (tr!("off by {} on {}", "расходится на {} на {}", r.difference.abs(), r.on_date), theme::bad()),
                None => (trs!("● never — s", "● не сверялся — s").to_string(), theme::warn()),
            };
            spans.push(Span::styled(fit(&text, check_width), pick(theme::tone(color))));
        }
        lines.push(Line::from(spans));
    }
    rows_of(frame, inner, lines);

    // The account under the cursor, its last thirty days.
    if parts[1].height < 6 {
        return;
    }
    let (Some(account), Some(view)) = (app.accounts.get(app.cursor()), app.account_views.get(app.cursor())) else { return };
    let title = tr!("{} · last 30 days", "{} · последние 30 дней", account.name);
    let block = pane(&title);
    let inner = block.inner(parts[1]);
    frame.render_widget(block, parts[1]);
    if account.operations == 0 {
        empty(frame, inner, trs!("no operations on it yet", "операций на нём пока нет"));
        return;
    }
    let width = inner.width as usize;
    let height = (inner.height as usize).saturating_sub(3).clamp(1, 10);
    let points = view.days.len().max(1);
    let column = (width / points).clamp(3, 9);
    let thick = column.saturating_sub(2).max(1);
    let top = view.days.iter().map(|(_, m)| m.minor()).max().unwrap_or(0).max(1);
    const LEVELS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let mut lines = Vec::new();
    for row in 0..height {
        let spans: Vec<Span> = view
            .days
            .iter()
            .map(|(_, m)| {
                let value = m.minor();
                // Below zero is drawn as a red floor: the one thing to see.
                if value < 0 {
                    let cell = if row == height - 1 { "▁" } else { " " };
                    return Span::styled(format!("{:<column$}", cell.repeat(thick)), theme::spending());
                }
                let eighths = (value as i128 * (height * 8) as i128 / top as i128) as usize;
                let level = eighths.saturating_sub((height - 1 - row) * 8).min(8);
                Span::styled(format!("{:<column$}", LEVELS[level].repeat(thick)), theme::chart())
            })
            .collect();
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(
        view.days.iter().map(|(day, _)| Span::styled(format!("{:<column$}", fit(&day.format("%d.%m").to_string(), column)), theme::label())).collect::<Vec<_>>(),
    ));
    lines.push(Line::raw(""));
    let m = &view.moves;
    let mut moved = vec![
        Span::styled(trs!("in ", "пришло "), theme::label()),
        Span::styled(m.inn.to_string(), theme::ok()),
        Span::styled(trs!("   out ", "   ушло "), theme::label()),
        Span::styled(m.out.abs().to_string(), theme::error()),
    ];
    if m.transfers.minor() != 0 {
        moved.push(Span::styled(trs!("   own accounts ", "   свои счета "), theme::label()));
        moved.push(Span::styled(format!("{}{}", if m.transfers.minor() > 0 { "+" } else { "" }, m.transfers), theme::value()));
    }
    if m.debts.minor() != 0 {
        moved.push(Span::styled(trs!("   debts ", "   долги "), theme::label()));
        moved.push(Span::styled(format!("{}{}", if m.debts.minor() > 0 { "+" } else { "" }, m.debts), theme::value()));
    }
    lines.push(Line::from(moved));
    rows_of(frame, inner, lines);
}

/// `▁▂▅▇▆` — a line through the values, lowest to highest.
pub(super) fn spark(values: &[i64]) -> String {
    // Up to seven eighths: one account's line under another's ran into it at
    // the top level.
    const LEVELS: [char; 7] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];
    let (Some(low), Some(high)) = (values.iter().min(), values.iter().max()) else { return String::new() };
    let span = (high - low).max(1) as i128;
    // The line is drawn left to right as the screen appears.
    let drawn = (values.len() as f64 * growth()).ceil() as usize;
    values.iter().take(drawn).map(|v| LEVELS[((v - low) as i128 * 6 / span) as usize]).collect()
}
