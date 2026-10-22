//! The «Statements» screen: what was loaded and what came of it, plus the
//! parsing of a statement before writing.
use super::*;

/// The journal of loads: the bank, the file, how many rows it gave and how many
/// operations came out. It answers «have I loaded this already» — and only that.
pub(super) fn draw_statements(frame: &mut Frame, area: Rect, app: &App) {
    let title = tr!("Statements · {} loaded", "Выписки · загружено {}", app.imports.len());
    let block = pane_focused(&title, app.screen == Screen::Statements && app.focus == Focus::List);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.imports.is_empty() {
        empty(frame, inner, trs!("nothing imported yet · i — load one", "ничего не загружалось · i — загрузить"));
        return;
    }
    let width = inner.width as usize;
    let (first, last) = window(app.cursor(), app.imports.len(), inner.height as usize);
    let mut lines = Vec::new();
    for (at, file) in app.imports.iter().enumerate().take(last).skip(first) {
        let name_width = width.saturating_sub(10 + 1 + 1 + 34).clamp(8, 34);
        let mut line = Line::from(Span::styled(
            tr!(
                "{:<10} {:<w$} {:>5} rows → {:>5} ops  {}", "{:<10} {:<w$} {:>5} строк → {:>5} опер  {}",
                file.bank,
                fit(&file.filename, name_width),
                file.rows,
                file.operations,
                file.imported_at,
                w = name_width
            ),
            theme::value(),
        ));
        if at == app.cursor() && app.focus == Focus::List {
            for span in &mut line.spans {
                span.style = theme::on_cursor(span.style);
            }
        }
        lines.push(line);
    }
    rows_of(frame, inner, lines);
}

/// Parsing a statement before writing — as a window over the screen. Only the
/// new rows are shown: duplicates and pending ones need no decision, only a count.
pub(super) fn draw_review(frame: &mut Frame, body: Rect, app: &App) {
    use potok_core::import::review::{Flag, Hint, RowStatus};
    let Some(review) = &app.review else { return };
    let area = centered(body, body.width * 19 / 20, body.height * 19 / 20);
    frame.render_widget(Clear, area);
    let plan = &review.plan;
    let title = tr!(
        "{} → {} · new {} (to write {}) · duplicates {} · pending {}", "{} → {} · новых {} (запишется {}) · дублей {} · ожидающих {}",
        review.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        plan.account,
        plan.count(RowStatus::New),
        plan.to_write(),
        plan.count(RowStatus::Duplicate),
        plan.count(RowStatus::Pending),
    );
    let block = pane_focused(&title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = review.new_rows();
    if rows.is_empty() {
        empty(frame, inner, trs!("no new rows — everything is in the ledger already. Esc", "новых строк нет — всё уже в леджере. Esc"));
        return;
    }
    let width = inner.width as usize;
    // "[✓] " 4, the date 11, the amount " {:>14}" 15, the mark up to 34
    let note_width = 34.min(width / 3);
    let what_width = width.saturating_sub(4 + 11 + 15 + 1 + note_width).max(6);
    let (from, to) = window(review.cursor, rows.len(), inner.height as usize);
    let lines: Vec<Line> = rows[from..to]
        .iter()
        .enumerate()
        .map(|(offset, &at)| {
            let row = &plan.rows[at];
            let mut notes: Vec<String> = Vec::new();
            match (&row.hint, row.accept_hint) {
                (Some(Hint::Transfer { with }), true) => notes.push(tr!("transfer ↔ {}", "перевод ↔ {}", with.account)),
                (Some(Hint::Debt { counterparty, kind, .. }), true) => notes.push(tr!(
                    "debt {counterparty}: {}", "долг {counterparty}: {}",
                    if *kind == "REPAYMENT" { trs!("repaid", "вернул") } else { trs!("borrowed", "взял") }
                )),
                (Some(_), false) => notes.push(trs!("hint declined", "подсказка отклонена").into()),
                (None, _) => {}
            }
            for flag in &row.flags {
                notes.push(
                    match flag {
                        Flag::Future => trs!("future date", "дата из будущего"),
                        Flag::Large => trs!("larger than usual", "крупнее обычного"),
                        Flag::Person => trs!("between people — a debt?", "между людьми — долг?"),
                        Flag::ByHand => trs!("entered by hand already", "уже введена руками"),
                    }
                    .into(),
                );
            }
            let text = format!(
                "{} {:<11}{:<w$} {:>14} {}",
                if row.write { "[✓]" } else { "[ ]" },
                row.occurred_on.to_string(),
                fit(row.who.as_deref().unwrap_or(&row.description), what_width),
                row.amount.to_string(),
                fit(&notes.join(" · "), note_width),
                w = what_width,
            );
            let style = if from + offset == review.cursor {
                theme::selected()
            } else if !row.write {
                theme::label()
            } else if !row.flags.is_empty() {
                theme::attention()
            } else if row.hint.is_some() && row.accept_hint {
                theme::chart()
            } else {
                theme::value()
            };
            Line::from(Span::styled(text, style))
        })
        .collect();
    rows_of(frame, inner, lines);
}
