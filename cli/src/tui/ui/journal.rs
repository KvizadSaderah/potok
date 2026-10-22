//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

/// The journal of edits: what was changed by hand, the new on top.
pub(super) fn draw_journal(frame: &mut Frame, area: Rect, app: &App) {
    let title = tr!("Changes ({}) · x — undo", "Правки ({}) · x — отменить", app.changes.len());
    let block = pane_focused(&title, app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.changes.is_empty() {
        empty(frame, inner, trs!("no changes yet — everything changed by hand shows up here", "правок ещё не было — здесь появится всё, что меняется руками"));
        return;
    }
    // "{:<12}" the time, " {:<9}" the mark = 10
    let what_width = (inner.width as usize).saturating_sub(12 + 10).max(6);
    let (from, to) = window(app.cursor(), app.changes.len(), inner.height as usize);
    let lines: Vec<Line> = app.changes[from..to]
        .iter()
        .enumerate()
        .map(|(offset, change)| {
            let mark = match (&change.undone_at, change.can_undo) {
                (Some(_), _) => trs!("undone", "отменена"),
                (None, true) => "",
                (None, false) => trs!("permanent", "навсегда"),
            };
            let row = format!(
                "{:<12}{:<w$} {:<9}",
                // «10-18 20:10»: the year is the same, the seconds are not needed.
                change.at.get(5..16).unwrap_or(&change.at),
                fit(&change.what, what_width),
                mark,
                w = what_width
            );
            let style = if from + offset == app.cursor() {
                theme::selected()
            } else if change.undone_at.is_some() {
                theme::label()
            } else {
                theme::value()
            };
            Line::from(Span::styled(row, style))
        })
        .collect();
    rows_of(frame, inner, lines);
}
