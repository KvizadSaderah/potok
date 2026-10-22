//! Drawing. Not one query and not one computation over money — the layout and
//! the strings that are already ready, nothing else (§5).
#[allow(unused_imports)]
use potok_core::{tr, trs};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use potok_core::query::Change;
use ratatui::style::Style;

use super::app::{AnalyticsPane, App, Detail, Focus, HomePane, Mode, OpsView, Pick, PickTarget};
use super::{action, theme, Screen};

mod accounts;
mod analytics;
mod budget;
mod categories;
mod debts;
mod detail;
mod duplicates;
mod flows;
mod home;
mod counterparties;
mod statements;
mod journal;
mod operations;
mod settings;
use accounts::*;
use analytics::*;
use budget::*;
use categories::*;
use debts::*;
use detail::*;
use duplicates::*;
use flows::*;
use home::*;
use counterparties::*;
use statements::*;
use journal::*;
use operations::*;
use settings::*;

thread_local! {
    /// How far the bars are drawn in this frame. It lives beside the theme and
    /// for the same reason: otherwise the factor would go into every function.
    static GROWTH: std::cell::Cell<f64> = const { std::cell::Cell::new(1.0) };
}

/// The share of its length a bar takes in this frame.
pub(super) fn growth() -> f64 {
    GROWTH.with(std::cell::Cell::get)
}

pub fn draw(frame: &mut Frame, app: &App) {
    GROWTH.with(|cell| cell.set(app.growth()));
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    if app.mode == Mode::Locked {
        draw_lock(frame, area, app);
        return;
    }
    draw_tabs(frame, rows[0], app);
    // Another device writes: one line across the top says so, and how to
    // take over, for as long as it is true.
    let mut rows = rows.to_vec();
    if let Some(mark) = &app.read_only {
        let split = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(1), Constraint::Min(1)]).split(rows[1]);
        let line = Line::from(vec![
            Span::styled(trs!(" read only ", " только чтение "), theme::badge(theme::warn())),
            Span::styled(
                tr!("  {} writes since {} — changes here would not be kept", "  пишет {} с {} — правки здесь не сохранятся", mark.device, mark.since_local()),
                theme::attention(),
            ),
            Span::styled(trs!("  ·  W — take over writing here", "  ·  W — писать здесь"), theme::key()),
        ]);
        frame.render_widget(Paragraph::new(line), split[0]);
        rows[1] = split[1];
    }
    if app.screen == Screen::Home {
        draw_home(frame, rows[1], app);
        draw_status(frame, rows[2], app);
        match app.mode {
            Mode::Help => draw_help(frame, area, app),
            Mode::Menu { selected } => draw_menu(frame, area, app, selected),
            _ => {}
        }
        return;
    }
    // A statement has no details — the list takes the whole width rather than
    // sharing it with an empty pane.
    let (list, details) = if app.screen == Screen::Statements { (rows[1], rows[1]) } else { split_body(rows[1]) };
    match app.screen {
        Screen::Home => {}
        Screen::Analytics => draw_analytics(frame, list, app),
        Screen::Budget => draw_budget(frame, list, app),
        Screen::Accounts => draw_accounts(frame, list, app),
        Screen::Operations => draw_operations(frame, list, app),
        Screen::Flows => draw_flows(frame, list, app),
        Screen::Debts => draw_debts(frame, list, app),
        Screen::Counterparties => draw_counterparties(frame, list, app),
        Screen::Statements => draw_statements(frame, list, app),
        Screen::Duplicates => draw_duplicates(frame, list, app),
        Screen::Journal => draw_journal(frame, list, app),
        Screen::Categories => draw_categories(frame, list, app),
        Screen::Settings => draw_settings(frame, list, app),
    }
    if app.screen != Screen::Statements {
        draw_detail(frame, details, app, false);
    }
    // The opened one as a window over the screen rather than instead of it: it
    // shows where one came from and where Esc returns.
    if app.zoom {
        let body = rows[1];
        let modal = centered(body, body.width * 9 / 10, body.height * 9 / 10);
        frame.render_widget(Clear, modal);
        draw_detail(frame, modal, app, true);
    }
    // The parsed statement shows while «write it?» is asked: the question is clear.
    if app.review.is_some() && matches!(app.mode, Mode::Review | Mode::Confirm) {
        draw_review(frame, rows[1], app);
    }
    draw_status(frame, rows[2], app);

    match app.mode {
        Mode::Locked => {}
        Mode::Help => draw_help(frame, area, app),
        Mode::PickCategory { selected } => draw_picker(frame, area, app, selected),
        Mode::Menu { selected } => draw_menu(frame, area, app, selected),
        // A «yes or no» question goes in the status line, and the screen under
        // it stays visible: it shows which operation is being asked about.
        Mode::Confirm => {}
        // Typing the remainder goes in the status line, like the search: the
        // account shows above it.
        Mode::Reconcile | Mode::Path => {}
        Mode::Review | Mode::Opening | Mode::Input => {}
        Mode::Form => draw_form(frame, rows[1], app),
        // The search line lives at the bottom, where the hints are — the list
        // above it updates on every letter and needs no window over it.
        Mode::Search | Mode::Normal => {}
    }
}

/// The list and the details side by side, as in lazygit: on a wide terminal to
/// the right, on a narrow one below.
fn split_body(area: Rect) -> (Rect, Rect) {
    let (direction, list) = if area.width >= 110 { (Direction::Horizontal, 58) } else { (Direction::Vertical, 55) };
    let parts = Layout::default()
        .direction(direction)
        .constraints([Constraint::Percentage(list), Constraint::Percentage(100 - list)])
        .split(area);
    (parts[0], parts[1])
}

/// The strip of screens, drawn the widest way that fits: full names, short
/// ones, then bare keys with only the current screen spelled out. Measured in
/// columns, not guessed — saldo's strip once drew the period on top of its
/// last tab because two widgets shared a row and nothing said so.
fn draw_tabs(frame: &mut Frame, area: Rect, app: &App) {
    const BRAND: &str = " potok ";
    let total = area.width as usize;
    type Namer = fn(Screen) -> &'static str;
    // Wide: « 1 Home »; tight: «1 Home ». Names shrink before the spacing
    // does, and the brand goes first of all.
    let tab = |screen: Screen, name: &str, tight: bool| {
        if tight {
            format!("{} {} ", screen.key(), name)
        } else {
            format!(" {} {} ", screen.key(), name)
        }
    };
    let width = |name: Namer, tight: bool| -> usize { Screen::ALL.iter().map(|s| tab(*s, name(*s), tight).chars().count()).sum::<usize>() };
    let brand = BRAND.chars().count();
    let tiers: [(Namer, bool, bool); 7] = [
        (Screen::title, false, true),
        (Screen::title, false, false),
        (Screen::short, false, true),
        (Screen::short, false, false),
        (Screen::short, true, false),
        (Screen::tiny, false, false),
        (Screen::tiny, true, false),
    ];
    let chosen = tiers.iter().find(|(name, tight, with_brand)| width(*name, *tight) + if *with_brand { brand } else { 0 } <= total);
    let mut spans = Vec::new();
    if chosen.is_some_and(|(_, _, with_brand)| *with_brand) {
        spans.push(Span::styled(BRAND, theme::heading()));
    }
    for screen in Screen::ALL {
        let current = screen == app.screen;
        let text = match chosen {
            Some((name, tight, _)) => tab(screen, name(screen), *tight),
            None if current => tab(screen, screen.short(), true),
            None => format!("{} ", screen.key()),
        };
        // A screen was switched — its tab lights up whole for a moment and
        // settles into the ordinary highlight. It shows where one moved to
        // without looking away from the middle of the screen.
        let style = match (current, growth() < 0.45) {
            (true, true) => theme::badge(theme::accent()),
            (true, false) => theme::selected().fg(theme::accent()),
            (false, _) => theme::label(),
        };
        spans.push(Span::styled(text, style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// How to show a key in a hint: as an arrow rather than a word.
fn key_label(key: &str) -> &str {
    match key {
        "right" => "→",
        "left" => "←",
        "up" => "↑",
        "down" => "↓",
        "enter" => "Enter",
        "tab" => "Tab",
        "esc" => "Esc",
        " " => "Space",
        other => other,
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    if app.mode == Mode::Confirm {
        let question = app.pending.as_ref().and_then(|p| p.confirm.clone()).unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(question, theme::strong().fg(theme::bad())),
            Span::styled(trs!("   y — yes · any other — no", "   y — да · любая другая — нет"), theme::label()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Input {
        use super::app::Ask;
        let (prompt, hint) = match &app.asking {
            Some(Ask::Rename(name)) => (tr!("new name for «{name}»: ", "новое имя «{name}»: "), trs!("Enter — rename · Esc", "Enter — переименовать · Esc")),
            Some(Ask::MergeInto(name)) => (tr!("merge «{name}» into: ", "свести «{name}» в: "), trs!("part of a name · Enter — choose · Esc", "часть имени · Enter — выбрать · Esc")),
            Some(Ask::OperationNote(id)) => (
                tr!("note on #{id}: ", "заметка на #{id}: "),
                trs!("what the bank did not write · empty — remove · Enter · Esc", "то, чего банк не написал · пусто — снять · Enter · Esc"),
            ),
            Some(Ask::Alias(name)) => (
                tr!("a spelling for «{name}»: ", "написание для «{name}»: "),
                trs!("as the bank writes it — the key is made of it · Enter · Esc", "как пишет банк — из этого выйдет ключ · Enter · Esc"),
            ),
            Some(Ask::Note(name)) => (
                tr!("note on «{name}»: ", "заметка на «{name}»: "),
                trs!("what this place really is · empty — remove · Enter · Esc", "чем на самом деле является точка · пусто — снять · Enter · Esc"),
            ),
            Some(Ask::LockSet) => (
                trs!("new passphrase: ", "новый пароль: ").into(),
                trs!("at least 6 characters · not the password of anything else · Enter · Esc", "не меньше 6 символов · не пароль от чего-то ещё · Enter · Esc"),
            ),
            // Named as Home names it — by the month it ends in. The key is the
            // calendar month of the rule's start, which read as the month before.
            Some(Ask::MonthStart((year, month))) => (
                {
                    let end = app.month_rule.cycle(app.month_rule.by_rule((*year, *month)), 0).end;
                    tr!(
                        "{} (to {}) starts on: ", "{} (до {}) начинается: ",
                        month_name(chrono::Datelike::month(&end)),
                        end.format("%d.%m")
                    )
                },
                trs!("a date · empty — back to the rule · Enter · Esc", "дата · пусто — по правилу · Enter · Esc"),
            ),
            Some(Ask::LockOff) => (trs!("current passphrase: ", "нынешний пароль: ").into(), "Enter · Esc"),
            Some(Ask::Range) => (
                trs!("dates: ", "даты: ").into(),
                trs!("2026-09 — a month, or two dates · Enter · Esc", "2026-09 — месяц, или две даты · Enter · Esc"),
            ),
            Some(Ask::Rate) => (
                trs!("rate: ", "курс: ").into(),
                trs!("RUB 0.0098 [date] — 1 RUB in the base currency · Enter · Esc", "RUB 0.0098 [дата] — 1 RUB в основной валюте · Enter · Esc"),
            ),
            Some(Ask::RenameCategory(name)) => (tr!("new name for «{name}»: ", "новое имя «{name}»: "), "Enter · Esc"),
            Some(Ask::MergeCategory(name)) => (
                tr!("merge «{name}» into: ", "свести «{name}» в: "),
                trs!("part of a name · Enter — choose · Esc", "часть имени · Enter — выбрать · Esc"),
            ),
            Some(Ask::Budget(name)) => (
                tr!("monthly budget for «{name}»: ", "бюджет на месяц «{name}»: "),
                trs!("amount in the base currency · empty — none · Enter · Esc", "сумма в основной валюте · пусто — без бюджета · Enter · Esc"),
            ),
            Some(Ask::Split(_)) => (
                trs!("split: ", "разбить: ").into(),
                trs!("Food 30; Household — one part without a sum takes the rest · empty — no split", "Еда 30; Хозяйство — часть без суммы берёт остаток · пусто — без разбивки"),
            ),
            Some(Ask::RenameAccount(_)) => (trs!("account name: ", "имя счёта: ").into(), "Enter · Esc"),
            Some(Ask::DeviceName) => (
                trs!("this device's name: ", "имя этого устройства: ").into(),
                trs!("empty — the Mac's own name · Enter · Esc", "пусто — имя мака · Enter · Esc"),
            ),
            Some(Ask::BackupDir) => (
                trs!("backups folder: ", "папка бэкапов: ").into(),
                trs!("empty — next to the ledger · Enter · Esc", "пусто — рядом с леджером · Enter · Esc"),
            ),
            Some(Ask::StatementsDir) => (
                trs!("statements folder: ", "папка выписок: ").into(),
                trs!("empty — ~/Downloads · Enter · Esc", "пусто — ~/Downloads · Enter · Esc"),
            ),
            Some(Ask::Respell(_)) => (trs!("actually it is: ", "на самом деле это: ").into(), trs!("a counterparty name, existing or not · Enter · Esc", "имя контрагента, есть он или нет · Enter · Esc")),
            None => (String::new(), ""),
        };
        // A passphrase is never drawn, only counted.
        let typed = if matches!(app.asking, Some(Ask::LockSet | Ask::LockOff)) {
            "•".repeat(app.input.chars().count())
        } else {
            app.input.clone()
        };
        let line = Line::from(vec![
            Span::styled(prompt, theme::key()),
            Span::styled(typed, theme::strong()),
            Span::styled("▌", theme::label()),
            Span::styled(format!("   {hint}"), theme::label()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Opening {
        let who = app
            .opening_for
            .and_then(|id| app.accounts.iter().find(|a| a.id == id))
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(tr!("opening balance {who}: ", "начальный остаток {who}: "), theme::key()),
            Span::styled(app.input.clone(), theme::strong()),
            Span::styled("▌", theme::label()),
            Span::styled(trs!("   what there was before the first operation · Enter — save · Esc", "   то, что было до первой операции · Enter — сохранить · Esc"), theme::label()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Form {
        let line = Line::from(Span::styled(
            trs!("Tab ↑↓ — field · ←→ — choose in a ‹ › field · Enter — write · Esc — drop", "Tab ↑↓ — поле · ←→ — выбрать в поле ‹ › · Enter — записать · Esc — бросить"),
            theme::label(),
        ));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Path {
        let line = Line::from(vec![
            Span::styled(trs!("statement: ", "выписка: "), theme::key()),
            Span::styled(app.input.clone(), theme::strong()),
            Span::styled("▌", theme::label()),
            Span::styled(trs!("   path to a PDF · Enter — review · Esc", "   путь к PDF · Enter — разобрать · Esc"), theme::label()),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Review {
        // `?` answers here, in the one line the review leaves free.
        let text = app.status.clone().unwrap_or_else(|| {
            trs!("↑↓ · Space — write the row or not · Enter — take the hint or not · w — write · Esc — drop · ? — what it means", "↑↓ · Space — писать строку или нет · Enter — принять подсказку или нет · w — записать · Esc — бросить · ? — что значит").into()
        });
        let line = Line::from(Span::styled(text, theme::label()));
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Reconcile {
        let who = app.accounts.get(app.cursor()).map(|a| a.name.clone()).unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(tr!("bank balance on {who}: ", "в банке на {who}: "), theme::key()),
            Span::styled(app.input.clone(), theme::strong()),
            Span::styled("▌", theme::label()),
            Span::styled(
                tr!("   amount [YYYY-MM-DD], no date — {} · Enter — reconcile · Esc", "   сумма [ГГГГ-ММ-ДД], без даты — {} · Enter — сверить · Esc", app.today),
                theme::label(),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    if app.mode == Mode::Search {
        let what = if app.screen == Screen::Counterparties { trs!("counterparties", "контрагенты") } else { trs!("operations", "операции") };
        let line = Line::from(vec![
            Span::styled("/", theme::key()),
            Span::styled(app.input.clone(), theme::strong()),
            Span::styled("▌", theme::label()),
            Span::styled(
                tr!("   searching: {what} · Enter — keep · Esc — cancel", "   ищу: {what} · Enter — оставить · Esc — отменить"),
                theme::label(),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }
    let line = match &app.status {
        Some(message) => Line::from(Span::styled(message.clone(), theme::strong())),
        None => {
            // As many as fit, whole — a hint cut in half helps nobody — and
            // `?` always last: it is where the rest are.
            let all: Vec<String> = action::hints(app.screen)
                .into_iter()
                .map(|(key, label)| format!("{} {label}", key_label(key)))
                .collect();
            let help = all.iter().find(|h| h.starts_with("? ")).cloned().unwrap_or_default();
            let room = (area.width as usize).saturating_sub(help.chars().count() + 5);
            let mut shown: Vec<&str> = Vec::new();
            let mut used = 0;
            for hint in all.iter().filter(|h| **h != help) {
                let cost = hint.chars().count() + if shown.is_empty() { 0 } else { 5 };
                if used + cost > room {
                    break;
                }
                used += cost;
                shown.push(hint);
            }
            shown.push(&help);
            Line::from(Span::styled(shown.join("  ·  "), theme::label()))
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// The list's window so that the cursor is always inside it. Without this a
/// long journal scrolls while the cursor goes off the edge and is lost.
fn window(cursor: usize, rows: usize, height: usize) -> (usize, usize) {
    if height == 0 || rows == 0 {
        return (0, 0);
    }
    let half = height / 2;
    let start = cursor.saturating_sub(half).min(rows.saturating_sub(height));
    (start, (start + height).min(rows))
}

/// A window over the screen, guaranteed to be inside it.
///
/// Without clipping by height the input window on a terminal of six rows went
/// outside the buffer and ratatui panicked — caught by a test at 20x6. A
/// terminal comes in any size, and «that does not happen» is not a check.
fn centered(area: Rect, want_width: u16, want_height: u16) -> Rect {
    let width = want_width.min(area.width);
    let height = want_height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn pane(title: &str) -> Block<'_> {
    pane_focused(title, false)
}

/// The pane the cursor is in — with a bright border: it shows where the arrows go.
fn pane_focused(title: &str, focused: bool) -> Block<'_> {
    let (border, heading) = if focused { (theme::border_focus(), theme::heading()) } else { (theme::border(), theme::label()) };
    Block::default()
        .borders(Borders::ALL)
        .border_type(theme::frame(focused))
        .title(Span::styled(format!(" {title} "), heading))
        .border_style(border)
}

fn empty(frame: &mut Frame, area: Rect, what: &str) {
    frame.render_widget(Paragraph::new(Span::styled(what.to_string(), theme::label())), area);
}

fn rows_of(frame: &mut Frame, area: Rect, mut lines: Vec<Line<'_>>) {
    // While the screen appears, the rows run in from top to bottom over the
    // same third of a second the bars grow in: no list arrives as a finished block.
    let growth = growth();
    if growth < 1.0 {
        let shown = ((lines.len() as f64 * growth).ceil() as usize).max(1);
        lines.truncate(shown);
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// `2026-10-01` → `01.10`. For headings where ISO dates would eat half a line.
fn short(date: &str) -> String {
    match (date.get(5..7), date.get(8..10)) {
        (Some(month), Some(day)) => format!("{day}.{month}"),
        _ => date.to_string(),
    }
}

/// Clips by characters rather than bytes: Cyrillic and PDF ligatures would
/// otherwise tear in the middle.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars().take(width.saturating_sub(1)).chain("…".chars()).collect()
}

/// «≈ 1 234.00 EUR», and which currencies have no rate to it.
fn total_text(app: &App) -> String {
    match &app.total {
        Some(t) if t.missing.is_empty() => tr!("total ≈ {}", "итого ≈ {}", t.total),
        Some(t) => {
            let codes: Vec<String> = t.missing.iter().map(|c| c.to_string()).collect();
            tr!("total ≈ {} without {} — no rate", "итого ≈ {} без {} — нет курса", t.total, codes.join(", "))
        }
        None => String::new(),
    }
}

fn kv(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10} "), theme::label()),
        Span::styled(value.into(), theme::value()),
    ])
}

/// What a detail pane is about: white and bold, as content is.
fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), theme::subject()))
}

/// A heading inside a pane: blue, as on every screen.
fn section(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), theme::section()))
}

fn hint_line(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), theme::label()))
}

fn bag_text(bag: &potok_core::MoneyBag) -> String {
    let all: Vec<String> = bag.in_display_order(None).iter().map(|m| m.to_string()).collect();
    if all.is_empty() { "—".into() } else { all.join(" / ") }
}

/// What to write in the category column: for what does not enter the flows —
/// what it is, rather than a category.
fn kind_column(operation: &potok_core::query::OperationLine) -> String {
    if operation.removed {
        return trs!("removed", "убрана").into();
    }
    match (operation.role.as_str(), &operation.pair_account) {
        ("transfer", Some(other)) if operation.amount.minor() < 0 => format!("→ {other}"),
        ("transfer", Some(other)) => format!("← {other}"),
        ("transfer", None) => trs!("transfer", "перевод").into(),
        ("debt_movement", _) => trs!("debt", "долг").into(),
        ("excluded", _) => trs!("not counted", "не считается").into(),
        ("passthrough", _) => trs!("transit", "транзит").into(),
        _ => operation.category.clone().unwrap_or_else(|| "—".into()),
    }
}

/// An operation's or a debt's form — as a window over the screen.
fn draw_form(frame: &mut Frame, body: Rect, app: &App) {
    let Some(form) = &app.form else { return };
    let area = centered(body, 68, form.fields.len() as u16 + 6);
    frame.render_widget(Clear, area);
    let block = pane_focused(&form.title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines: Vec<Line> = form
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let here = index == form.field;
            let typing = here && field.choices.is_empty();
            let value = if field.choices.is_empty() && field.text.is_empty() && !here {
                Span::styled(field.empty.to_string(), theme::label())
            } else {
                Span::styled(
                    format!("{}{}", field.shown(), if typing { "▌" } else { "" }),
                    if here { theme::strong() } else { theme::value() },
                )
            };
            Line::from(vec![
                Span::styled(
                    format!("{:<11}", field.label),
                    if here { theme::key() } else { theme::label() },
                ),
                value,
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(match &form.error {
        Some(error) => Line::from(Span::styled(fit(error, inner.width as usize), theme::strong().fg(theme::bad()))),
        None => hint_line(trs!("date: 2026-10-18, 18.10 or «yesterday»", "дата: 2026-10-18, 18.10 или «вчера»")),
    });
    rows_of(frame, inner, lines);
}

/// The first amount of a bag — the one bars are scaled by.
// ponytail: bars follow the first currency only; a month spent in two
// currencies shows both in figures, but one bar. Convert by rates if that
// ever matters.
/// The amount of a bag in its one currency, for a bar or a colour; 0 for a
/// bag of several — their minor units are not one scale (§2).
fn first_minor(bag: &potok_core::MoneyBag) -> i64 {
    match bag.in_display_order(None).as_slice() {
        [one] => one.minor(),
        _ => 0,
    }
}

/// `████▌     ` — `value` out of `max` over `width` columns, eighths for the
/// last cell.
fn bar(value: i64, max: i64, width: usize) -> String {
    // A slight overshoot halfway through the growth and back: a bar looks
    // thrown rather than pushed out. Its length is clipped by the width anyway.
    let t = growth();
    let overshoot = 1.0 + 0.08 * (t * (1.0 - t) * 4.0);
    let value = (value as f64 * t * overshoot) as i64;
    if max <= 0 || width == 0 {
        return " ".repeat(width);
    }
    let eighths = (value.max(0) as i128 * width as i128 * 8 / max as i128) as usize;
    let full = (eighths / 8).min(width);
    // Seven eighths high, not the full cell: bars on rows next to each other
    // (out and in, a list of categories, of months) ran together into one
    // block. The gap at the top keeps each row its own.
    let mut out = "▇".repeat(full);
    if full < width {
        out.push(["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"][eighths % 8].chars().next().unwrap_or(' '));
        out = format!("{out:<width$}");
    }
    out.chars().take(width).collect()
}

fn budget_color(line: &potok_core::budget::BudgetLine) -> ratatui::style::Color {
    if line.left.minor() < 0 {
        theme::bad()
    } else if line.left.minor() * 5 < line.limit.minor() {
        theme::warn()
    } else {
        theme::good()
    }
}

/// The lock: the brand, the time, a masked field — nothing of the ledger.
fn draw_lock(frame: &mut Frame, area: Rect, app: &App) {
    const LOGO: [&str; 2] = ["█▀█ █▀█ ▀█▀ █▀█ █▄▀", "█▀▀ █▄█  █  █▄█ █ █"];
    let now = chrono::Local::now();
    let wait = app.lock_wait();
    let box_area = centered(area, 46, 14);
    let failed = app.lock_failures > 0;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(theme::frame(true))
        .border_style(if failed { theme::error() } else { theme::border_focus() });
    let inner = block.inner(box_area);
    frame.render_widget(Clear, box_area);
    frame.render_widget(block, box_area);
    let center = |text: String, style: Style| Line::from(Span::styled(text, style)).alignment(ratatui::layout::Alignment::Center);
    let mut lines = vec![Line::raw("")];
    // The logo runs in by columns — the first thing seen at startup.
    let columns = (LOGO[0].chars().count() as f64 * growth()).ceil() as usize;
    for row in LOGO {
        lines.push(center(row.chars().take(columns).collect::<String>(), theme::heading()));
    }
    lines.push(Line::raw(""));
    lines.push(center(now.format("%H:%M:%S").to_string(), theme::strong()));
    lines.push(center(now.format("%A, %d %B %Y").to_string(), theme::label()));
    lines.push(Line::raw(""));
    let dots = "•".repeat(app.input.chars().count());
    lines.push(center(format!("{dots}▌"), theme::strong()));
    lines.push(Line::raw(""));
    lines.push(if wait > 0 {
        center(tr!("wrong — wait {wait} s", "не тот — подожди {wait} с"), theme::error())
    } else if let Some(message) = &app.status {
        center(message.clone(), theme::error())
    } else {
        center(trs!("passphrase · Enter", "пароль · Enter").to_string(), theme::label())
    });
    if let Some(at) = app.locked_at {
        lines.push(center(tr!("locked at {}", "заблокировано в {}", at.format("%H:%M")), theme::label()));
    }
    lines.push(center(trs!("Ctrl+C — quit", "Ctrl+C — выйти").to_string(), theme::label()));
    rows_of(frame, inner, lines);
}

fn month_name(month: u32) -> &'static str {
    let names = match potok_core::i18n::lang() {
        potok_core::i18n::Lang::Ru => ["Январь", "Февраль", "Март", "Апрель", "Май", "Июнь", "Июль", "Август", "Сентябрь", "Октябрь", "Ноябрь", "Декабрь"],
        _ => ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"],
    };
    names[(month as usize).clamp(1, 12) - 1]
}

fn month_short(month: u32) -> &'static str {
    let names = match potok_core::i18n::lang() {
        potok_core::i18n::Lang::Ru => ["янв", "фев", "мар", "апр", "май", "июн", "июл", "авг", "сен", "окт", "ноя", "дек"],
        _ => ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    };
    names[(month as usize).clamp(1, 12) - 1]
}

/// The help reads the same list as the keyboard (`action::BINDINGS`), so it
/// cannot fall out of step with it.
fn draw_help(frame: &mut Frame, area: Rect, app: &App) {
    // This screen's own keys first, then the ones that work everywhere; keys
    // of other screens are not listed. Before, the list ran past the bottom
    // of a 24- or 30-row terminal with this screen's keys below the edge.
    let own = action::BINDINGS.iter().filter(|b| b.screens.contains(&app.screen));
    let everywhere = action::BINDINGS.iter().filter(|b| b.screens.is_empty());
    let shown: Vec<&action::Binding> = own.chain(everywhere).collect();
    let keys_of = |b: &action::Binding| b.keys.iter().map(|k| key_label(k)).collect::<Vec<_>>().join(" / ");
    let key_width = shown.iter().map(|b| keys_of(b).chars().count()).max().unwrap_or(0) + 2;
    let label_width = shown.iter().map(|b| b.label().chars().count()).max().unwrap_or(0);
    let column = (key_width + label_width) as u16;
    let room = area.height.saturating_sub(4).max(1) as usize;
    let columns = if shown.len() > room && area.width >= column * 2 + 5 { 2 } else { 1 };
    let per_column = shown.len().div_ceil(columns);
    let box_area = centered(area, column * columns as u16 + 3 * columns as u16 + 1, per_column as u16 + 2);
    frame.render_widget(Clear, box_area);
    let block = pane(trs!("What the keys do", "Что умеют клавиши"));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    let line_of = |b: &action::Binding| {
        Line::from(vec![
            Span::styled(format!("{:<w$}", keys_of(b), w = key_width), theme::key()),
            Span::styled(b.label().to_string(), theme::value()),
        ])
    };
    let parts = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, columns as u32); columns])
        .split(inner);
    for (part, chunk) in parts.iter().zip(shown.chunks(per_column)) {
        let mut lines: Vec<Line> = chunk.iter().map(|b| line_of(b)).collect();
        if lines.len() > part.height as usize {
            lines.truncate((part.height as usize).saturating_sub(1));
            lines.push(hint_line(trs!("… more on a taller terminal", "… остальное — на окне повыше")));
        }
        rows_of(frame, *part, lines);
    }
}

/// The menu of actions — what can be done with what is under the cursor.
fn draw_menu(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    let box_area = centered(area, 60, app.menu.len() as u16 + 4);
    frame.render_widget(Clear, box_area);
    let block = pane_focused(trs!("What to do", "Что сделать"), true);
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);
    let width = inner.width as usize;
    let mut lines: Vec<Line> = app
        .menu
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let style = if index == selected {
                theme::selected()
            } else if item.confirm.is_some() {
                theme::error()
            } else {
                theme::value()
            };
            Line::from(Span::styled(fit(&format!(" {}", item.label), width), style))
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(hint_line(trs!("↑↓ choose · Enter — do · Esc", "↑↓ выбрать · Enter — сделать · Esc")));
    rows_of(frame, inner, lines);
}

/// Choosing a category: the typing on top, the list under it, the hint first.
fn draw_picker(frame: &mut Frame, area: Rect, app: &App, selected: usize) {
    // In the queue the name alone is often not enough to decide: say how much
    // and how often.
    let who = match &app.pick_target {
        PickTarget::Operation(id) => app
            .operations
            .iter()
            .find(|o| o.id == *id)
            .map(|o| format!("#{} {} {} {}", o.id, o.occurred_on, o.amount, o.description))
            .unwrap_or_else(|| format!("#{id}")),
        PickTarget::Marked => tr!("{} marked operations", "помечено операций: {}", app.marked.len()),
        PickTarget::Counterparty => match app.selected_counterparty() {
            Some(line) if app.queue => tr!(
                "{} — {} operations, spent {}", "{} — {} операций, потрачено {}",
                line.name,
                line.operations,
                line.spent.in_display_order(None).iter().map(|m| m.to_string()).collect::<Vec<_>>().join(" / ")
            ),
            Some(line) => line.name.clone(),
            None => String::new(),
        },
        // The named one is marked — the one the journal row paid.
        PickTarget::Named(who) => match app.counterparties.iter().find(|line| &line.name == who) {
            Some(line) => tr!("{} — {} operations", "{} — {} операций", line.name, line.operations),
            None => who.clone(),
        },
    };
    let picks = app.picks();
    let box_area = centered(area, 64, (picks.len() as u16).clamp(1, 12) + 5);
    frame.render_widget(Clear, box_area);
    let block = pane(trs!("Category", "Категория"));
    let inner = block.inner(box_area);
    frame.render_widget(block, box_area);

    let width = inner.width as usize;
    let mut lines = vec![
        Line::from(Span::styled(fit(&who, width), theme::label())),
        Line::from(vec![
            Span::styled("> ", theme::label()),
            Span::styled(app.input.clone(), theme::strong()),
            Span::styled("▌", theme::label()),
        ]),
    ];
    let room = (inner.height as usize).saturating_sub(lines.len() + 1);
    let (first, last) = window(selected, picks.len(), room);
    for (offset, pick) in picks[first..last].iter().enumerate() {
        let here = first + offset == selected;
        let text = match pick {
            Pick::Hint { category, count, of } => tr!("↳ {category} — {count} of {of}", "↳ {category} — у {count} из {of}"),
            Pick::Existing(category) => format!("  {category}"),
            Pick::Create(category) => tr!("+ create «{category}»", "+ создать «{category}»"),
        };
        let style = match (here, pick) {
            (true, _) => theme::selected(),
            (false, Pick::Hint { .. }) => theme::key(),
            (false, Pick::Create(_)) => theme::label(),
            (false, Pick::Existing(_)) => theme::value(),
        };
        lines.push(Line::from(Span::styled(fit(&text, width), style)));
    }
    if picks.is_empty() {
        lines.push(Line::from(Span::styled(trs!("type a category name", "набери название категории"), theme::label())));
    }
    let keys = if app.queue {
        trs!("↑↓ choose · Enter — apply, next · Tab — skip · Esc — stop", "↑↓ выбрать · Enter — применить, дальше · Tab — пропустить · Esc — стоп")
    } else {
        match &app.pick_target {
            PickTarget::Operation(_) => trs!("↑↓ choose · Enter — apply to this operation · Esc", "↑↓ выбрать · Enter — применить к этой операции · Esc"),
            PickTarget::Marked => trs!("↑↓ choose · Enter — apply to marked operations · Esc", "↑↓ выбрать · Enter — применить к помеченным · Esc"),
            PickTarget::Counterparty | PickTarget::Named(_) => {
                trs!("↑↓ choose · Enter — apply to all its operations · Esc", "↑↓ выбрать · Enter — применить ко всем его операциям · Esc")
            }
        }
    };
    lines.push(Line::from(Span::styled(fit(keys, width), theme::label())));
    rows_of(frame, inner, lines);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn app() -> App {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor)
                 values ('Revolut','EUR','BANK',40212);
             insert into counterparty (name) values ('Kaufland');
             insert into operation
                 (account_id,role,occurred_at,amount_minor,currency,effect_minor,
                  description,counterparty_id)
                 values (1,'expense','2026-09-01T00:00:00Z',624,'EUR',-624,'Kaufland',1);
             insert into debt (counterparty_id,direction,currency,opened_on)
                 values (1,'OWED_TO_ME','EUR','2026-09-03');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
                 values (1,'ISSUE',80000,'2026-09-03');",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.period = super::super::app::Period::Everything;
        app.reload().unwrap();
        app
    }

    /// The three redrawn screens show what they were redrawn for: the month
    /// and its columns, the journal by day with a transfer on one line, the
    /// account's year and its last thirty days.
    #[test]
    fn the_redrawn_screens_show_what_they_are_for() {
        let conn = potok_core::db::open_memory().unwrap();
        let today = chrono::Local::now().date_naive();
        let day = |back: i64| (today - chrono::Duration::days(back)).to_string();
        conn.execute_batch(&format!(
            "insert into account (id,name,currency,kind,opening_minor) values (1,'iCard','EUR','CARD',100000), (2,'Revolut','EUR','BANK',0), (3,'Cash','EUR','CASH',0);
             insert into counterparty (id,name) values (1,'Kaufland');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'transfer','{d0}',1000,'EUR',-1000,'Revolut** Dublin',null),
               (2,2,'transfer','{d0}',1000,'EUR',1000,'Apple Pay deposit',null),
               (3,2,'expense','{d0}',807,'EUR',-807,'KAUFLAND',1),
               (4,1,'expense','{d40}',3115,'EUR',-3115,'KAUFLAND',1);
             update operation set pair_id = 2 where id = 1;
             update operation set pair_id = 1 where id = 2;",
            d0 = day(0),
            d40 = day(40)
        ))
        .unwrap();
        let mut app = App::new(conn).unwrap();

        app.screen = Screen::Home;
        let home = render(&app, 140, 40);
        assert!(home.contains("out by month") && home.contains("where it went") && home.contains("Coming up"), "{home}");
        assert!(!home.contains("Cash"), "an account with nothing on it is not «not reconciled»: {home}");

        app.screen = Screen::Operations;
        let journal = render(&app, 140, 40);
        assert_eq!(journal.matches("iCard → Revolut").count(), 1, "one line per transfer: {journal}");
        assert!(journal.contains('⇄') && journal.contains("8.07 EUR"), "{journal}");
        assert!(journal.contains(&day_label(&day(0))), "the day heads its rows: {journal}");

        app.screen = Screen::Accounts;
        app.refresh_detail().unwrap();
        let accounts = render(&app, 140, 40);
        assert!(accounts.contains("checked with the bank") && accounts.contains("last 30 days"), "{accounts}");
        assert!(accounts.contains("not used yet"), "Cash: {accounts}");
        let rows: Vec<&str> = accounts.lines().filter(|l| l.contains("iCard") || l.contains("Cash")).collect();
        assert!(rows.first().is_some_and(|l| l.contains("iCard")), "used accounts first: {rows:?}");
    }

    /// Every screen asks the theme for a role and builds no style of its
    /// own: that is what keeps a colour meaning the same thing everywhere.
    #[test]
    fn screens_take_their_styles_from_the_theme_roles() {
        // Every drawing file, or the rule holds only where it is remembered.
        let files: [(&str, &str); 15] = [
            ("mod.rs", include_str!("mod.rs")),
            ("home.rs", include_str!("home.rs")),
            ("accounts.rs", include_str!("accounts.rs")),
            ("operations.rs", include_str!("operations.rs")),
            ("flows.rs", include_str!("flows.rs")),
            ("analytics.rs", include_str!("analytics.rs")),
            ("budget.rs", include_str!("budget.rs")),
            ("debts.rs", include_str!("debts.rs")),
            ("counterparties.rs", include_str!("counterparties.rs")),
            ("statements.rs", include_str!("statements.rs")),
            ("duplicates.rs", include_str!("duplicates.rs")),
            ("categories.rs", include_str!("categories.rs")),
            ("settings.rs", include_str!("settings.rs")),
            ("journal.rs", include_str!("journal.rs")),
            ("detail.rs", include_str!("detail.rs")),
        ];
        for (name, source) in files {
            let body = &source[..source.find("#[cfg(test)]").unwrap_or(source.len())];
            for forbidden in ["Style::default()", "Color::", "Modifier::"] {
                let at: Vec<usize> = body.lines().enumerate().filter(|(_, l)| l.contains(forbidden)).map(|(i, _)| i + 1).collect();
                assert!(at.is_empty(), "{forbidden} in ui/{name} on lines {at:?} — add a role to theme.rs instead");
            }
        }
    }


    #[test]
    fn a_drilled_list_shows_who_is_inside_it() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('R','EUR','BANK');
             insert into category (id,name) values (1,'Food');
             insert into counterparty (id,name) values (1,'KAUFLAND'), (2,'BILLA');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,1,'expense','2026-10-09T00:00:00Z',10000,'EUR',-10000,'K',1,1),
               (2,1,'expense','2026-10-08T00:00:00Z', 2000,'EUR', -2000,'B',2,1);",
        ).unwrap();
        let mut app = App::new(conn).unwrap();
        app.ops_view = OpsView::Drill { label: "Food".into(), from: "2026-10-01".into(), to: "2026-10-31".into() };
        app.refresh_view().unwrap();
        app.screen = Screen::Operations;
        app.refresh_detail().unwrap();
        let drawn = render(&app, 120, 30);
        assert!(drawn.contains("who is inside · 2 ops"), "{drawn}");
        let kaufland = drawn.lines().find(|l| l.contains("who is inside")).is_some()
            && drawn.lines().any(|l| l.contains("KAUFLAND") && l.contains('▇') && l.contains("-100.00 EUR"));
        assert!(kaufland, "the largest gets a bar the whole width: {drawn}");
        assert!(drawn.lines().any(|l| l.contains("BILLA") && l.contains("-20.00 EUR")), "{drawn}");

        // The whole journal has no split: there «who is inside» is the ledger.
        app.ops_view = OpsView::All;
        app.refresh_view().unwrap();
        app.refresh_detail().unwrap();
        assert!(!render(&app, 120, 30).contains("who is inside"));
    }

    fn render(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every screen draws at every size. That is the test which in the old
    /// client caught nearly every drawing panic.
    #[test]
    fn every_screen_draws_at_every_size() {
        let mut app = app();
        for lang in potok_core::i18n::Lang::ALL {
            potok_core::i18n::set_lang(lang);
            draws_everywhere(&mut app);
        }
        potok_core::i18n::set_lang(potok_core::i18n::Lang::En);
    }

    fn draws_everywhere(app: &mut App) {
        for screen in Screen::ALL {
            app.screen = screen;
            for (width, height) in [(150, 44), (100, 30), (80, 24), (40, 10), (20, 6)] {
                let drawn = render(app, width, height);
                assert!(!drawn.is_empty(), "{screen:?} at {width}x{height} drew nothing");
            }
        }
    }

    /// The details pane draws on every screen at every size, beside the list
    /// and opened — a narrow terminal puts it below rather than breaking it.
    #[test]
    fn the_detail_pane_draws_everywhere_split_and_zoomed() {
        let mut app = app();
        // The Overview and «Statements» show no details: the first has its own
        // three lists, the second says everything in its row.
        for screen in Screen::ALL.into_iter().filter(|s| !matches!(s, Screen::Home | Screen::Statements)) {
            app.screen = screen;
            app.refresh_detail().unwrap();
            for zoom in [false, true] {
                app.zoom = zoom;
                for (width, height) in [(200, 50), (150, 44), (110, 30), (109, 30), (80, 24), (40, 10), (20, 6)] {
                    let drawn = render(&app, width, height);
                    // The pane is named for what is in it now, not «Details».
                    let named = ["Details", "Operation", "Transfer", "Account", "Flow", "Debt", "Counterparty", "Look-alikes", "Change", "Setting", "Category", "Month", "Budget"];
                    assert!(named.iter().any(|n| drawn.contains(n)), "{screen:?} {width}x{height} zoom={zoom}");
                }
            }
        }
    }

    /// A debt shows how it was paid — what people come there for.
    #[test]
    fn a_debt_shows_how_it_was_paid() {
        let mut app = app();
        app.screen = Screen::Debts;
        app.refresh_detail().unwrap();
        let drawn = render(&app, 150, 30);
        assert!(drawn.contains("how it was paid"), "{drawn}");
        assert!(drawn.contains("lent"), "the lending shows in the ribbon: {drawn}");
    }

    /// The help and the category input — over the screen, and at any size too.
    #[test]
    fn the_overlays_draw_too() {
        let mut app = app();
        app.screen = Screen::Counterparties;
        for mode in [Mode::Help, Mode::PickCategory { selected: 0 }, Mode::Search, Mode::Menu { selected: 0 }, Mode::Confirm] {
            app.mode = mode.clone();
            if matches!(mode, Mode::Menu { .. }) {
                app.screen = Screen::Operations;
                app.menu = app.menu_items().unwrap();
            }
            for (width, height) in [(150, 44), (80, 24), (20, 6)] {
                assert!(!render(&app, width, height).is_empty(), "{mode:?} at {width}x{height}");
            }
        }
    }

    #[test]
    fn the_balance_is_on_the_screen_it_opens_with() {
        let app = app();
        let drawn = render(&app, 100, 20);
        assert!(drawn.contains("Revolut"), "{drawn}");
        assert!(drawn.contains("395.88"), "402.12 - 6.24 — {drawn}");
    }

    /// The cursor must be inside the window at any list length — otherwise a
    /// person scrolls and cannot see where they are.
    #[test]
    fn the_cursor_is_always_inside_the_window() {
        for rows in [0usize, 1, 7, 500] {
            for height in [0usize, 1, 3, 20] {
                for cursor in [0, rows / 2, rows.saturating_sub(1)] {
                    let (from, to) = window(cursor, rows, height);
                    assert!(from <= to, "the window turned inside out: {from}..{to}");
                    assert!(to <= rows, "the window is outside the list");
                    if rows > 0 && height > 0 {
                        assert!(cursor >= from && cursor < to, "the cursor {cursor} is outside {from}..{to}");
                    }
                }
            }
        }
    }

    /// A window over the screen must lie inside the buffer at any size — that is
    /// what ratatui panicked on.
    #[test]
    fn an_overlay_never_leaves_the_screen() {
        for (width, height) in [(150u16, 44u16), (80, 24), (20, 6), (10, 3), (1, 1)] {
            let area = Rect { x: 0, y: 0, width, height };
            for (want_width, want_height) in [(56u16, 24u16), (60, 4), (200, 200)] {
                let box_area = centered(area, want_width, want_height);
                assert!(box_area.x + box_area.width <= area.width, "{box_area:?} inside {area:?}");
                assert!(box_area.y + box_area.height <= area.height, "{box_area:?} inside {area:?}");
            }
        }
    }

    /// No flow row sticks out past the border: the first version ate the «s» of
    /// «ops», because the column widths were estimated rather than added up.
    /// The unit is «ops»/«опер» on every screen now, as Categories had it.
    #[test]
    fn no_flow_row_runs_past_its_frame() {
        let mut app = app();
        app.screen = Screen::Flows;
        for lang in potok_core::i18n::Lang::ALL {
            potok_core::i18n::set_lang(lang);
            let unit = trs!("ops", "опер");
            for width in [80u16, 99, 100, 120, 150] {
                let drawn = render(&app, width, 12);
                for line in drawn.lines().skip(2).take(3) {
                    assert!(
                        // The border is part of the theme: «terminal» has it
                        // straight, the rest rounded, and both end the line.
                        line.ends_with(['│', '║']) || line.trim().is_empty(),
                        "{lang:?} at {width}: a row without its right border — {line:?}"
                    );
                    let cut: String = unit.chars().take(unit.chars().count() - 1).collect();
                    assert!(line.contains(unit) || !line.contains(&cut), "{lang:?} at {width}: «{unit}» cut — {line:?}");
                }
            }
        }
        potok_core::i18n::set_lang(potok_core::i18n::Lang::En);
    }

    /// Clipping must not tear a character in the middle.
    #[test]
    fn fitting_counts_characters_not_bytes() {
        assert_eq!(fit("Kaufland", 20), "Kaufland");
        assert_eq!(fit("НОРТУИНД БЪЛГАРИЯ ЕООД", 10).chars().count(), 10);
        assert_eq!(fit("So\u{fb01}a", 3).chars().count(), 3);
    }
}
