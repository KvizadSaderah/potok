//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Categories: what is spent in each over the flows period.
pub(super) fn draw_categories(frame: &mut Frame, area: Rect, app: &App) {
    let title = tr!("Categories ({}) · {} · Enter — operations · x — more", "Категории ({}) · {} · Enter — операции · x — ещё", app.category_lines.len(), app.period.label());
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.category_lines.is_empty() {
        empty(frame, inner, trs!("no categories yet — c on the counterparties screen gives a counterparty one", "категорий ещё нет — c на экране контрагентов даёт её контрагенту"));
        return;
    }
    // " {:>17}" = 18, "  {:>5} ops" — by its own format
    let count = tr!("  {:>5} ops", "  {:>5} опер", 0).chars().count();
    let name_width = (inner.width as usize).saturating_sub(18 + count).max(6);
    let (from, to) = window(app.cursor(), app.category_lines.len(), inner.height as usize);
    let lines: Vec<Line> = app.category_lines[from..to]
        .iter()
        .enumerate()
        .map(|(offset, c)| {
            let selected = from + offset == app.cursor();
            let style = if selected { theme::selected() } else { theme::value() };
            // A category that only brought money in shows what came, marked.
            let (amount, amount_style) = if c.spent.is_empty() && !c.received.is_empty() {
                (format!("+{}", bag_text(&c.received)), if selected { style } else { theme::earning() })
            } else {
                (bag_text(&c.spent), style)
            };
            Line::from(vec![
                Span::styled(format!("{:<w$} ", fit(&c.name, name_width), w = name_width), style),
                Span::styled(format!("{amount:>17}"), amount_style),
                Span::styled(tr!("  {:>5} ops", "  {:>5} опер", c.operations), style),
            ])
        })
        .collect();
    rows_of(frame, inner, lines);
}
