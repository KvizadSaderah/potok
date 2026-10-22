//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// Similar operations: one account, one day, one amount, one counterparty.
/// Candidates rather than a verdict — a person decides.
pub(super) fn draw_duplicates(frame: &mut Frame, area: Rect, app: &App) {
    let title = tr!("Look-alike operations ({}) · Enter — expand · x — remove or «not a duplicate»", "Похожие операции ({}) · Enter — развернуть · x — убрать или «не дубль»", app.duplicates.len());
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.duplicates.is_empty() {
        empty(frame, inner, trs!("no look-alike operations", "похожих операций нет"));
        return;
    }
    // "{:<11}" the date, " {:<10}" the account = 11, " {:>17}" = 18, " ×N" = 4
    let who_width = (inner.width as usize).saturating_sub(11 + 11 + 18 + 4).max(6);
    let (from, to) = window(app.cursor(), app.duplicates.len(), inner.height as usize);
    let lines: Vec<Line> = app.duplicates[from..to]
        .iter()
        .enumerate()
        .map(|(offset, group)| {
            let first = &group.operations[0];
            let who = first.counterparty.as_deref().filter(|n| !n.is_empty()).unwrap_or(&first.description);
            // Colour on the amount only: a whole red row says nothing a red
            // figure does not, and thirty of them make red mean nothing.
            let selected = from + offset == app.cursor();
            let pick = |base: Style| if selected { theme::selected() } else { base };
            Line::from(vec![
                Span::styled(format!("{:<11}", first.occurred_on), pick(theme::label())),
                Span::styled(format!("{:<w$} ", fit(who, who_width), w = who_width), pick(theme::value())),
                Span::styled(format!("{:<10}", fit(&first.account, 10)), pick(theme::label())),
                Span::styled(format!(" {:>17}", first.amount.to_string()), pick(theme::money(first.amount.minor()))),
                Span::styled(format!(" ×{}", group.operations.len()), pick(theme::label())),
            ])
        })
        .collect();
    rows_of(frame, inner, lines);
}
