//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Home: the month in one look — the money, how fast it goes against last
/// month, the months before, where it went; beside it what is coming, what
/// needs a look, and the debts.
pub(super) fn draw_home(frame: &mut Frame, area: Rect, app: &App) {
    let Some(o) = &app.overview else { return };
    let halves = if area.width >= 100 {
        Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area)
    } else {
        Layout::default().direction(Direction::Vertical).constraints([Constraint::Percentage(60), Constraint::Percentage(40)]).split(area)
    };
    let cycle = app.month_rule.cycle(app.today, 0);
    let title = tr!(
        "{} · {} → {} · day {} of {}", "{} · {} → {} · день {} из {}",
        // Named by the month it ends in: 30.09–29.10 is October to whoever
        // reads it, whatever the rule calls it.
        month_name(chrono::Datelike::month(&cycle.end)),
        cycle.start.format("%d.%m"),
        cycle.end.format("%d.%m"),
        (app.today - cycle.start).num_days() + 1,
        (cycle.end - cycle.start).num_days() + 1
    );
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(halves[0]);
    frame.render_widget(block, halves[0]);
    let width = inner.width as usize;
    let mut left = Vec::new();

    let mut top = vec![Span::styled(o.total.total.to_string(), theme::heading())];
    if !o.i_owe.is_empty() {
        top.push(Span::styled(tr!("   · I owe {}", "   · я должен {}", bag_text(&o.i_owe)), theme::label()));
    }
    if !o.owed_to_me.is_empty() {
        top.push(Span::styled(tr!(" · owed to me {}", " · мне должны {}", bag_text(&o.owed_to_me)), theme::label()));
    }
    left.push(Line::from(top));
    if !o.total.missing.is_empty() {
        let codes: Vec<String> = o.total.missing.iter().map(|c| c.to_string()).collect();
        left.push(hint_line(&tr!("without {} — no rate · , → Exchange rates", "без {} — нет курса · , → Курсы валют", codes.join(", "))));
    }
    left.push(Line::raw(""));

    // Out and in on one scale, the larger of this month and the whole of the
    // last; a mark on «out» where last month was by this day.
    let minor = |base: Option<potok_core::Money>, bag: &potok_core::MoneyBag| base.map(|m| m.minor()).unwrap_or_else(|| first_minor(bag));
    let either = |base: Option<potok_core::Money>, bag: &potok_core::MoneyBag| base.map(|m| m.to_string()).unwrap_or_else(|| bag_text(bag));
    let (out_now, in_now) = (minor(o.this_month.out_base, &o.this_month.out), minor(o.this_month.inn_base, &o.this_month.inn));
    let before = minor(o.last_month_so_far.out_base, &o.last_month_so_far.out);
    let last_whole = app.months.iter().rev().nth(1).map(|m| minor(m.out_base, &m.out)).unwrap_or(0);
    let scale = out_now.max(in_now).max(last_whole).max(before);
    let figure_width = 16;
    let bar_width = width.saturating_sub(6 + figure_width + 2).clamp(4, 48);
    let mut out_bar: Vec<char> = bar(out_now, scale, bar_width).chars().collect();
    let mark = (before > 0 && scale > 0).then(|| ((before as i128 * bar_width as i128 / scale as i128) as usize).min(bar_width - 1));
    if let Some(at) = mark {
        out_bar[at] = '│';
    }
    let (head, tail) = match mark {
        Some(at) => (out_bar[..at].iter().collect::<String>(), out_bar[at + 1..].iter().collect::<String>()),
        None => (out_bar.iter().collect(), String::new()),
    };
    left.push(Line::from(vec![
        Span::styled(format!("{:<6}", trs!("out", "ушло")), theme::label()),
        Span::styled(format!("{:>w$}  ", either(o.this_month.out_base, &o.this_month.out), w = figure_width), theme::strong().fg(theme::bad())),
        Span::styled(head, theme::spending()),
        Span::styled(if mark.is_some() { "│" } else { "" }, theme::value()),
        Span::styled(tail, theme::spending()),
    ]));
    left.push(Line::from(vec![
        Span::styled(format!("{:<6}", trs!("in", "пришло")), theme::label()),
        Span::styled(format!("{:>w$}  ", either(o.this_month.inn_base, &o.this_month.inn), w = figure_width), theme::strong().fg(theme::good())),
        Span::styled(bar(in_now, scale, bar_width), theme::earning()),
    ]));
    if before > 0 {
        let change = (out_now - before) * 100 / before;
        let (text, color) = if change > 0 {
            (tr!("{change}% more than by this day last month", "на {change}% больше, чем к этому дню прошлого месяца"), theme::bad())
        } else {
            (tr!("{}% less than by this day last month", "на {}% меньше, чем к этому дню прошлого месяца", -change), theme::good())
        };
        left.push(Line::from(vec![
            Span::styled(format!("{:<6}", ""), theme::label()),
            Span::styled(text, theme::tone(color)),
            Span::styled(trs!("  · │ on the bar — then", "  · │ на полосе — тогда"), theme::label()),
        ]));
    }
    left.push(Line::raw(""));

    // The months before, as columns: the shape of the year at a glance.
    let shown = app.home_months();
    if shown.len() > 1 {
        // The cursor over the columns is a list like «where it went»: Tab moves
        // to it, Enter opens the month's operations.
        let on_months = app.screen == Screen::Home && app.focus == Focus::List && app.home_pane == HomePane::Months;
        let at = on_months.then_some(app.home_month_at().min(shown.len() - 1));
        left.push(section(trs!("out by month", "расход по месяцам")));
        let column = (width / shown.len()).clamp(3, 7);
        let thick = column.saturating_sub(2).max(1);
        let height = 6usize;
        let tallest = shown.iter().map(|m| minor(m.out_base, &m.out)).max().unwrap_or(0).max(1);
        const LEVELS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        for row in 0..height {
            let mut spans = Vec::new();
            for (index, m) in shown.iter().enumerate() {
                let grown = (minor(m.out_base, &m.out) as f64 * growth()) as i128;
                let eighths = (grown * (height * 8) as i128 / tallest as i128) as usize;
                let level = eighths.saturating_sub((height - 1 - row) * 8).min(8);
                let current = index == shown.len() - 1;
                let color = if current { theme::accent() } else { theme::bad() };
                let mut style = theme::tone(color);
                if at == Some(index) {
                    style = theme::on_cursor(style);
                }
                spans.push(Span::styled(format!("{:<column$}", LEVELS[level].repeat(thick)), style));
            }
            left.push(Line::from(spans));
        }
        let labels: Vec<Span> = shown
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let mut style = if index == shown.len() - 1 { theme::section() } else { theme::label() };
                if at == Some(index) {
                    style = theme::on_cursor(style);
                }
                Span::styled(format!("{:<column$}", fit(month_short(chrono::Datelike::month(&m.end)), column)), style)
            })
            .collect();
        left.push(Line::from(labels));
        // Under the columns are the figures of what is chosen: otherwise the
        // highlighted column says only «this one», not how much it is.
        if let Some(m) = at.and_then(|at| shown.get(at)) {
            // Several currencies do not fit a column — then their sum in the main one.
            let money = |base: Option<potok_core::Money>, bag: &potok_core::MoneyBag| match (bag.in_display_order(None).len(), base) {
                (2.., Some(base)) => format!("≈ {base}"),
                _ => bag_text(bag),
            };
            let name = month_name(chrono::Datelike::month(&m.end));
            left.push(Line::from(vec![
                Span::styled(format!("{name} "), theme::section()),
                Span::styled(tr!("out {}", "ушло {}", money(m.out_base, &m.out)), theme::tone(theme::bad())),
                Span::styled(tr!(" · in {}", " · пришло {}", money(m.inn_base, &m.inn)), theme::tone(theme::good())),
                Span::styled(trs!(" · Enter — its operations", " · Enter — его операции"), theme::label()),
            ]));
        }
        left.push(Line::raw(""));
    }

    // Where this month went — the one list on this screen the cursor walks,
    // and Enter opens its operations.
    let flows = app.home_flows();
    if !flows.is_empty() {
        // Whose month this is stands on the block itself. A finished month's
        // figures under this month's heading are exactly what must not happen.
        match app.home_past.as_ref().and_then(|past| shown.get(past.at).map(|m| (past, m))) {
            Some((past, month)) => left.push(Line::from(vec![
                Span::styled(format!("{} · ", trs!("where it went", "куда ушло")), theme::section()),
                Span::styled(month_name(chrono::Datelike::month(&month.end)).to_string(), theme::section()),
                Span::styled(
                    tr!(" — a month that is over, {}–{}", " — закончившийся месяц, {}–{}", short(&past.from), short(&past.to)),
                    theme::attention(),
                ),
            ])),
            None => left.push(section(trs!("where it went", "куда ушло"))),
        }
        let max = flows.iter().filter_map(|f| f.out_base.map(|m| m.minor())).max().unwrap_or(0);
        let label_width = (width / 4).clamp(8, 20);
        let bar_width = width.saturating_sub(label_width + 1 + 15 + 6).max(4);
        let room = (inner.height as usize).saturating_sub(left.len() + 1).max(3);
        let here = app.screen == Screen::Home && app.focus == Focus::List && app.home_pane == HomePane::Flows;
        let (first, last) = window(if here { app.cursor() } else { 0 }, flows.len(), room);
        for (at, f) in flows.iter().enumerate().take(last).skip(first) {
            let mut line = share_line(f, max, label_width, bar_width);
            if here && at == app.cursor() {
                for span in &mut line.spans {
                    span.style = theme::on_cursor(span.style);
                }
            }
            left.push(line);
        }
        if last < flows.len() {
            left.push(hint_line(&tr!("…{} more below", "…ещё {} ниже", flows.len() - last)));
        }
    }
    if !o.budgets.is_empty() {
        left.push(Line::raw(""));
        left.push(section(trs!("budgets this month", "бюджеты месяца")));
        let label_width = (width / 3).clamp(6, 18);
        let bar_width = width.saturating_sub(label_width + 1 + 14).max(4);
        for b in o.budgets.iter().take(6) {
            left.push(Line::from(vec![
                Span::styled(format!("{:<w$} ", fit(&b.category, label_width), w = label_width), theme::value()),
                Span::styled(bar(b.spent.minor(), b.limit.minor(), bar_width), theme::tone(budget_color(b))),
                Span::styled(format!(" {:>13}", b.left.to_string()), theme::tone(budget_color(b))),
            ]));
        }
    }
    rows_of(frame, inner, left);

    // Right: three panes stacked, and the cursor walks all three as one list
    // — what is coming, what needs a look, the debts. The rows and what Enter
    // does on them live in `app`; here they are only drawn.
    let look = app.home_look();
    let open_debts = app.home_debts();
    // Which row is under the cursor, counted across the three panes.
    let at = (app.screen == Screen::Home && app.focus == Focus::Detail).then_some(app.detail_cursor);
    let upcoming_rows = o.upcoming.len();
    let look_rows = look.len();
    let coming_height = (o.upcoming.len().max(1) + 4) as u16;
    let look_height = (look.len().max(1) + 2) as u16;
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(coming_height), Constraint::Length(look_height), Constraint::Min(3)])
        .split(halves[1]);

    let block = pane_focused(trs!("Coming up · 14 days", "Впереди · 14 дней"), at.is_some_and(|at| at < upcoming_rows));
    let inner = block.inner(right[0]);
    frame.render_widget(block, right[0]);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    if o.upcoming.is_empty() {
        lines.push(hint_line(trs!("no regular payment expected", "регулярных платежей не ждём")));
    }
    let who_width = width.saturating_sub(6 + 15);
    for (index, r) in o.upcoming.iter().enumerate() {
        let late = r.next < app.today;
        let mut line = Line::from(vec![
            Span::styled(format!("{} ", r.next.format("%d.%m")), if late { theme::attention() } else { theme::date() }),
            Span::styled(format!("{:<w$}", fit(&r.who, who_width), w = who_width), theme::value()),
            Span::styled(format!(" {:>14}", r.typical.to_string()), theme::value()),
        ]);
        if at == Some(index) {
            for span in &mut line.spans {
                span.style = theme::on_cursor(span.style);
            }
        }
        lines.push(line);
    }
    if !o.upcoming.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(format!("{:<w$}", trs!("due", "к оплате"), w = 6 + who_width), theme::label()),
            Span::styled(format!(" {:>14}", bag_text(&o.upcoming_total)), theme::strong().fg(theme::bad())),
        ]));
    }
    rows_of(frame, inner, lines);

    let block = pane_focused(
        trs!("Needs a look", "Посмотреть"),
        at.is_some_and(|at| at >= upcoming_rows && at < upcoming_rows + look_rows),
    );
    let inner = block.inner(right[1]);
    frame.render_widget(block, right[1]);
    let width = inner.width as usize;
    let lines: Vec<Line> = if look.is_empty() {
        vec![Line::from(Span::styled(trs!("all in order", "всё в порядке"), theme::ok()))]
    } else {
        look.iter()
            .enumerate()
            .map(|(index, row)| {
                let room = width.saturating_sub(2 + row.keys.chars().count() + 1);
                let mut line = Line::from(vec![
                    Span::styled("● ", theme::attention()),
                    Span::styled(format!("{:<room$} ", fit(&row.text, room)), theme::value()),
                    Span::styled(row.keys.to_string(), theme::key()),
                ]);
                if at == Some(upcoming_rows + index) {
                    for span in &mut line.spans {
                        span.style = theme::on_cursor(span.style);
                    }
                }
                line
            })
            .collect()
    };
    rows_of(frame, inner, lines);

    let block = pane_focused(trs!("Debts · what is left", "Долги · сколько осталось"), at.is_some_and(|at| at >= upcoming_rows + look_rows));
    let inner = block.inner(right[2]);
    frame.render_widget(block, right[2]);
    let width = inner.width as usize;
    let mut lines = Vec::new();
    if open_debts.is_empty() {
        lines.push(hint_line(trs!("no open debts", "открытых долгов нет")));
    }
    for (index, d) in open_debts.iter().enumerate() {
        // Mine to pay is yellow, theirs to pay me green; the figures right,
        // the name gets what is left of the line.
        let mine = d.direction != "OWED_TO_ME";
        let figures = tr!("{} of {}", "{} из {}", d.remaining, d.issued);
        let name_width = width.saturating_sub(figures.chars().count() + 1).max(4);
        let mut line = Line::from(vec![
            Span::styled(format!("{:<name_width$} ", fit(&d.counterparty, name_width)), theme::value()),
            Span::styled(figures, theme::label()),
        ]);
        if at == Some(upcoming_rows + look_rows + index) {
            for span in &mut line.spans {
                span.style = theme::on_cursor(span.style);
            }
        }
        lines.push(line);
        let tone = if mine { theme::warn() } else { theme::good() };
        lines.push(Line::from(Span::styled(bar(d.remaining.minor(), d.issued.minor(), width), theme::tone(tone))));
    }
    rows_of(frame, inner, lines);
}
