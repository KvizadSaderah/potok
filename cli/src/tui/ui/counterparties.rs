//! The «Counterparties» screen: a directory of who is paid and who pays.
//!
//! Marking is the work done here, so the cursor walks this list and the
//! category hint stands right in the row.
use super::*;

/// Who there is, how much they were paid and which category they are in.
pub(super) fn draw_counterparties(frame: &mut Frame, area: Rect, app: &App) {
    let shown = app.visible_counterparties();
    let unmarked = app.counterparties.iter().filter(|line| line.category.is_none()).count();
    let heading = if app.queue {
        tr!(
            "Counterparties — categorising: {} left · Enter apply · Tab skip · Esc stop", "Контрагенты — разметка: осталось {} · Enter применить · Tab пропустить · Esc стоп",
            shown.len()
        )
    } else if app.counterparty_filter.is_empty() {
        tr!(
            "Counterparties · {unmarked} of {} without a category", "Контрагенты · без категории {unmarked} из {}",
            app.counterparties.len()
        )
    } else {
        tr!("Counterparties — «{}» ({}) · Esc — all", "Контрагенты — «{}» ({}) · Esc — все", app.counterparty_filter, shown.len())
    };
    let block = pane_focused(&heading, app.screen == Screen::Counterparties && app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if shown.is_empty() {
        let why = if app.counterparty_filter.is_empty() {
            trs!("counterparties appear after an import", "контрагенты появятся после импорта")
        } else {
            trs!("nobody by that name", "никого с таким именем")
        };
        empty(frame, inner, why);
        return;
    }
    let (from, to) = window(app.cursor(), shown.len(), inner.height as usize);
    let lines: Vec<Line> = shown[from..to]
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            let selected = from + offset == app.cursor();
            let style = if selected { theme::selected() } else { theme::value() };
            let spent: Vec<String> =
                line.spent.in_display_order(None).iter().map(|m| m.to_string()).collect();
            // " {:>17}" = 18, "  {:>4} ops  " = 11; the categories get what is
            // left, but not less than twelve characters.
            let width = inner.width as usize;
            let count = tr!("  {:>4} ops  ", "  {:>4} опер  ", 0).chars().count();
            // Names up to 26: the hint beside an uncategorised one is what
            // this list is for, and at 34 it was cut to «Housing & …».
            let name_width = width.saturating_sub(18 + count + 12).clamp(8, 26);
            let category_width = width.saturating_sub(name_width + 18 + count);
            let mut spans = vec![Span::styled(
                tr!(
                    "{:<w$} {:>17}  {:>4} ops  ", "{:<w$} {:>17}  {:>4} опер  ",
                    fit(&line.name, name_width),
                    spent.join(" / "),
                    line.operations,
                    w = name_width,
                ),
                style,
            )];
            // No category, but its operations are nearly all in one already —
            // say so right in the list: it shows where one key press finishes it.
            match (&line.category, &line.hint) {
                (Some(category), _) => spans.push(Span::styled(fit(category, category_width), style)),
                (None, Some((hint, count))) => spans.push(Span::styled(
                    fit(&format!("— ↳ {hint} {count}/{}", line.operations), category_width),
                    if selected { style } else { theme::label() },
                )),
                (None, None) => spans.push(Span::styled("—", style)),
            }
            Line::from(spans)
        })
        .collect();
    rows_of(frame, inner, lines);
}

// ── The details ─────────────────────────────────────────────────────────────
