//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Budget: this month's spending against each limit.
pub(super) fn draw_budget(frame: &mut Frame, area: Rect, app: &App) {
    // The accounting month the limits count over, named as Home names it.
    let cycle = app.month_rule.cycle(app.today, 0);
    let title = tr!(
        "Budget · {} · {}–{} · x — more", "Бюджет · {} · {}–{} · x — ещё",
        month_name(chrono::Datelike::month(&cycle.end)),
        cycle.start.format("%d.%m"),
        cycle.end.format("%d.%m")
    );
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.budget_lines.is_empty() {
        empty(frame, inner, trs!("no budgets — 9 Categories, x, «Monthly budget…»", "бюджетов нет — 9 Категории, x, «Бюджет на месяц…»"));
        return;
    }
    let width = inner.width as usize;
    let label_width = (width / 4).clamp(8, 22);
    let bar_width = width.saturating_sub(label_width + 1 + 30).max(4);
    let (from, to) = window(app.cursor(), app.budget_lines.len(), inner.height as usize);
    let lines: Vec<Line> = app.budget_lines[from..to]
        .iter()
        .enumerate()
        .map(|(offset, b)| {
            let selected = from + offset == app.cursor();
            let style = |base: Style| if selected { theme::selected() } else { base };
            let mut spans = vec![
                Span::styled(format!("{:<w$} ", fit(&b.category, label_width), w = label_width), style(theme::value())),
                Span::styled(bar(b.spent.minor(), b.limit.minor(), bar_width), style(theme::tone(budget_color(b)))),
                Span::styled(format!(" {:>14} / {:>13}", b.spent.decimal(), b.limit.to_string()), style(theme::value())),
            ];
            if !b.missing.is_empty() {
                let codes = b.missing.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ");
                spans.push(Span::styled(tr!(" (no {codes} rate)", " (нет курса {codes})"), style(theme::label())));
            }
            Line::from(spans)
        })
        .collect();
    rows_of(frame, inner, lines);
}
