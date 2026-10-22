//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

pub(super) fn draw_debts(frame: &mut Frame, area: Rect, app: &App) {
    let block = pane_focused(trs!("Debts · Enter — expand", "Долги · Enter — развернуть"), app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.debts.is_empty() {
        empty(frame, inner, trs!("no debts", "долгов нет"));
        return;
    }
    // " {:<12}" = 13, " {:>17}" = 18, " из " = 4, " {:>17}" = 18; a bar of
    // what is left gets what remains, if there is room for one.
    let fixed = 13 + 18 + 4 + 17;
    let bar_width = (inner.width as usize).saturating_sub(fixed + 16 + 2).min(20);
    let name_width = (inner.width as usize).saturating_sub(fixed + bar_width + if bar_width > 0 { 2 } else { 0 }).max(8);
    let (from, to) = window(app.cursor(), app.debts.len(), inner.height as usize);
    let lines: Vec<Line> = app.debts[from..to]
        .iter()
        .enumerate()
        .map(|(offset, debt)| {
            let selected = from + offset == app.cursor();
            let closed = debt.remaining.minor() == 0;
            let who = match (closed, debt.direction.as_str()) {
                (true, _) => trs!("settled", "закрыт"),
                (false, "OWED_TO_ME") => trs!("owes me", "мне должен"),
                (false, _) => trs!("I owe", "я должен"),
            };
            let row = tr!(
                "{:<w$} {:<12} {:>17} of {:>17}", "{:<w$} {:<12} {:>17} из {:>17}",
                fit(&debt.counterparty, name_width),
                who,
                debt.remaining.to_string(),
                debt.issued.to_string(),
                w = name_width,
            );
            let style = match (selected, closed) {
                (true, _) => theme::selected(),
                (false, true) => theme::label(),
                (false, false) => theme::value(),
            };
            let mut spans = vec![Span::styled(row, style)];
            if bar_width > 0 {
                // What is left of what was borrowed: a debt shrinks along it.
                let left = bar(debt.remaining.minor(), debt.issued.minor(), bar_width);
                spans.push(Span::styled(format!("  {left}"), if selected { style } else { theme::attention() }));
            }
            Line::from(spans)
        })
        .collect();
    rows_of(frame, inner, lines);
}
