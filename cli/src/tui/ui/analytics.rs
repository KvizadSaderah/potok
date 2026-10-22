//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Analytics: twelve months, newest first, and the payments that repeat.
pub(super) fn draw_analytics(frame: &mut Frame, area: Rect, app: &App) {
    let months_focused = app.focus == Focus::List && app.analytics_pane == AnalyticsPane::Months;
    let regular_focused = app.focus == Focus::List && app.analytics_pane == AnalyticsPane::Regular;
    // Header + up to 12 months + summary line + 2 border rows.
    let months_height = ((app.months.len() + 4) as u16).min(area.height.saturating_sub(5)).max(5);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(months_height), Constraint::Min(3)])
        .split(area);
    let block = pane_focused(
        trs!("Months · Enter — operations · Tab — pane", "Месяцы · Enter — операции · Tab — панель"),
        months_focused,
    );
    let inner = block.inner(parts[0]);
    frame.render_widget(block, parts[0]);
    let width = inner.width as usize;
    // In the base currency where every currency of the month has a rate.
    let out_of = |m: &potok_core::analytics::Month| m.out_base.map(|x| x.minor()).unwrap_or_else(|| first_minor(&m.out));
    let in_of = |m: &potok_core::analytics::Month| m.inn_base.map(|x| x.minor()).unwrap_or_else(|| first_minor(&m.inn));
    // Without a rate a month of several currencies does not fit a column:
    // say so rather than print «10 111.95 EUR / 3 000.00 RUB» across the next.
    let shown = |base: Option<potok_core::Money>, bag: &potok_core::MoneyBag| match (base, bag.in_display_order(None).len()) {
        (Some(m), _) => m.to_string(),
        (None, 0 | 1) => bag_text(bag),
        (None, _) => trs!("no rate", "нет курса").to_string(),
    };
    let max_out = app.months.iter().map(out_of).max().unwrap_or(0);
    // "dd.mm–dd.mm.yy✎ " 16, " {:>14}" ×3 = 45
    let bar_width = width.saturating_sub(16 + 45).max(4);
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{:<16}", trs!("period", "период")), theme::label()),
        Span::styled(format!("{:<bar_width$}", ""), theme::label()),
        Span::styled(format!(" {:>14}", trs!("out", "ушло")), theme::label()),
        Span::styled(format!(" {:>14}", trs!("in", "пришло")), theme::label()),
        Span::styled(format!(" {:>14}", trs!("net", "итог")), theme::label()),
    ]));
    let body_room = (inner.height as usize).saturating_sub(2).max(1);
    let (m_from, m_to) = window(app.month_cursor(), app.months.len(), body_room);
    for (offset, m) in app.months.iter().rev().skip(m_from).take(m_to.saturating_sub(m_from)).enumerate() {
        let index = m_from + offset;
        let selected = index == app.month_cursor() && app.analytics_pane == AnalyticsPane::Months;
        let (out, inn) = (out_of(m), in_of(m));
        let net = m.net_base.map(|n| n.minor()).unwrap_or(0);
        let style = |base: Style| if selected && months_focused { theme::selected() } else if selected { theme::strong() } else { base };
        let period = format!("{}–{}{}", m.start.format("%d.%m"), m.end.format("%d.%m.%y"), if m.corrected { "✎ " } else { "  " });
        // A month before the ledger began: a dash, not three zeros that
        // read as a month with nothing spent.
        if m.out.is_empty() && m.inn.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(period, style(theme::label())),
                Span::styled(format!("{:<bar_width$} {:>14}", "", "—"), style(theme::label())),
            ]));
            continue;
        }
        // The net in the month's own currency, signed — it said +195.87
        // with no currency while the two columns beside it had one.
        let sign = if net > 0 { "+" } else if net < 0 { "−" } else { "" };
        let net_text = match m.net_base {
            Some(x) => format!("{sign}{}", potok_core::Money::new(net.abs(), x.currency())),
            None => trs!("no rate", "нет курса").to_string(),
        };
        lines.push(Line::from(vec![
            Span::styled(period, style(theme::label())),
            // The current month in blue, as on Home.
            Span::styled(bar(out, max_out, bar_width), style(if index == 0 { theme::chart() } else { theme::spending() })),
            Span::styled(format!(" {:>14}", shown(m.out_base, &m.out)), style(theme::money(-out.min(1)))),
            Span::styled(format!(" {:>14}", shown(m.inn_base, &m.inn)), style(theme::money(inn.min(1)))),
            Span::styled(format!(" {:>14}", net_text), style(theme::money(net))),
        ]));
    }
    // Average across completed active months (all except the current incomplete month at index 0).
    let completed: Vec<&potok_core::analytics::Month> = app
        .months
        .iter()
        .rev()
        .skip(1)
        .filter(|m| (!m.out.is_empty() || !m.inn.is_empty()) && m.out_base.is_some() && m.inn_base.is_some())
        .collect();
    if !completed.is_empty() && lines.len() < inner.height as usize {
        let n = completed.len() as i64;
        let cur = completed[0].out_base.unwrap().currency();
        let avg_out = (completed.iter().map(|m| m.out_base.unwrap().minor()).sum::<i64>() + n / 2) / n;
        let avg_in = (completed.iter().map(|m| m.inn_base.unwrap().minor()).sum::<i64>() + n / 2) / n;
        let avg_net = avg_in - avg_out;
        let sign = if avg_net > 0 { "+" } else if avg_net < 0 { "−" } else { "" };
        let label = fit(&tr!("avg ({n} mo.)", "среднее ({n} мес.)"), 16);
        lines.push(Line::from(vec![
            Span::styled(format!("{:<16}", label), theme::label()),
            Span::styled(format!("{:<bar_width$}", ""), theme::label()),
            Span::styled(format!(" {:>14}", potok_core::Money::new(avg_out, cur).to_string()), theme::label()),
            Span::styled(format!(" {:>14}", potok_core::Money::new(avg_in, cur).to_string()), theme::label()),
            Span::styled(format!(" {:>14}", format!("{sign}{}", potok_core::Money::new(avg_net.abs(), cur))), theme::label()),
        ]));
    }
    rows_of(frame, inner, lines);

    let mut monthly_regular = potok_core::MoneyBag::new();
    for r in &app.regular {
        let per_month = if r.every_days > 0 && r.every_days <= 10 {
            (r.typical.minor() * 30 + r.every_days / 2) / r.every_days
        } else {
            r.typical.minor()
        };
        monthly_regular.add(potok_core::Money::new(per_month, r.typical.currency()));
    }
    let reg_title = if app.regular.is_empty() {
        trs!("Regular payments — found, not entered", "Регулярные платежи — найдены, не заведены").to_string()
    } else {
        tr!(
            "Regular payments · ~{}/mo · Tab — pane · Enter — ops",
            "Регулярные платежи · ~{}/мес · Tab — панель · Enter — опер.",
            bag_text(&monthly_regular)
        )
    };
    let block = pane_focused(&reg_title, regular_focused);
    let inner = block.inner(parts[1]);
    frame.render_widget(block, parts[1]);
    let width = inner.width as usize;
    if app.regular.is_empty() {
        empty(frame, inner, trs!("none yet — it takes three months of the same payment", "пока нет — нужно три месяца одного и того же платежа"));
        return;
    }
    let who_width = width.saturating_sub(15 + 12 + 14).max(6);
    let (r_from, r_to) = window(app.regular_cursor, app.regular.len(), inner.height as usize);
    let lines: Vec<Line> = app.regular[r_from..r_to]
        .iter()
        .enumerate()
        .map(|(offset, r)| {
            let index = r_from + offset;
            let selected = index == app.regular_cursor && app.analytics_pane == AnalyticsPane::Regular;
            let style = |base: Style| if selected && regular_focused { theme::selected() } else if selected { theme::strong() } else { base };
            Line::from(vec![
                Span::styled(format!("{:<w$}", fit(&r.who, who_width), w = who_width), style(theme::value())),
                Span::styled(format!(" {:>14}", r.typical.to_string()), style(theme::value())),
                Span::styled(format!("  {:<10}", tr!("every {}d", "раз в {}д", r.every_days)), style(theme::label())),
                Span::styled(format!(" {} {}", trs!("next", "дальше"), r.next.format("%d.%m")), style(theme::date())),
            ])
        })
        .collect();
    rows_of(frame, inner, lines);
}
