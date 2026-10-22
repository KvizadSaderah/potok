//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

pub(super) fn draw_flows(frame: &mut Frame, area: Rect, app: &App) {
    let (from, to) = app.period.bounds_on(app.today, &app.month_rule);
    let title = match app.period.previous_bounds_on(app.today, &app.month_rule) {
        Some((before_from, before_to)) => tr!(
            "Flows — {} · {}–{} vs {}–{} · Enter — operations", "Потоки — {} · {}–{} против {}–{} · Enter — операции",
            app.period.label(),
            short(&from),
            short(&to),
            short(&before_from),
            short(&before_to)
        ),
        None => tr!("Flows — {} · Enter — operations", "Потоки — {} · Enter — операции", app.period.label()),
    };
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.flows.is_empty() {
        empty(frame, inner, trs!("no posted operations for this period", "за этот период проведённых операций нет"));
        return;
    }
    let bag = |b: &potok_core::MoneyBag| -> String {
        let all: Vec<String> = b.in_display_order(None).iter().map(|m| m.to_string()).collect();
        if all.is_empty() { "—".into() } else { all.join(" / ") }
    };
    // Wide — everything; narrow — only what the screen is for: where it went
    // and how much more or less than before.
    let wide = inner.width >= 100;
    // The width of everything right of the name is added up from the same
    // formats as below rather than estimated: an estimate one short ate the
    // «s» of «ops». " {:>17}" out = 18, " {:>17}" was = 18, " {:>9}" change = 10,
    // " {:>17}" in = 18, "  {:>4} ops" = 9.
    // The counter goes by its own format: «pcs» is one character longer than «шт».
    let count = tr!("  {:>4} ops", "  {:>4} опер", 0).chars().count();
    let right = if wide { 18 + 18 + 10 + 18 + count } else { 18 + 10 + count };
    let label_width = (inner.width as usize).saturating_sub(right);
    let (first, last) = window(app.cursor(), app.flows.len(), inner.height as usize);
    let lines: Vec<Line> = app.flows[first..last]
        .iter()
        .enumerate()
        .map(|(offset, flow)| {
            let selected = first + offset == app.cursor();
            let (delta, delta_style) = change_text(flow.change);
            let base = if selected {
                theme::selected()
            } else if flow.count == 0 {
                theme::label()
            } else {
                theme::value()
            };
            // Narrow, there is no «in» column: a row that only brought money
            // in (a salary) shows what came, marked, instead of a bare «—».
            let only_in = !wide && flow.out.is_empty() && !flow.inn.is_empty();
            let mut spans = vec![Span::styled(format!("{:<w$} ", fit(&flow.label, label_width), w = label_width), base)];
            if only_in {
                spans.push(Span::styled(format!("{:>17}", format!("+{}", bag(&flow.inn))), if selected { base } else { theme::earning() }));
            } else {
                spans.push(Span::styled(format!("{:>17}", bag(&flow.out)), base));
            }
            if wide {
                spans.push(Span::styled(format!(" {:>17}", bag(&flow.out_before)), theme::label()));
            }
            spans.push(Span::styled(format!(" {delta:>9}"), if selected { base } else { delta_style }));
            if wide {
                spans.push(Span::styled(format!(" {:>17}", bag(&flow.inn)), base));
            }
            spans.push(Span::styled(tr!("  {:>4} ops", "  {:>4} опер", flow.count), base));
            Line::from(spans)
        })
        .collect();
    rows_of(frame, inner, lines);
}

/// A row of «where it went»: the bar against the biggest row in the base
/// currency, the share from the core.
pub(super) fn share_line(f: &potok_core::query::Flow, max: i64, label_width: usize, bar_width: usize) -> Line<'static> {
    let amount = f.out_base.map(|m| m.minor()).unwrap_or(0);
    Line::from(vec![
        Span::styled(format!("{:<w$} ", fit(&f.label, label_width), w = label_width), theme::value()),
        Span::styled(bar(amount, max, bar_width), theme::spending()),
        // Several currencies do not fit the column: their sum in the base one.
        Span::styled(
            format!(
                " {:>14}",
                match (f.out.in_display_order(None).len(), f.out_base) {
                    (2.., Some(base)) => format!("≈ {base}"),
                    _ => bag_text(&f.out),
                }
            ),
            theme::value(),
        ),
        Span::styled(format!(" {:>4}", f.share.map(|p| format!("{p}%")).unwrap_or_default()), theme::label()),
    ])
}
