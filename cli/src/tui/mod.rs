//! The interface: screens, the event loop, the terminal.
pub mod action;
pub mod app;
pub mod lock;
pub mod theme;
pub mod ui;

#[allow(unused_imports)]
use potok_core::{tr, trs};
use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use rusqlite::Connection;

use action::Action;
use app::{App, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Accounts,
    Operations,
    Flows,
    Analytics,
    Budget,
    Debts,
    Counterparties,
    Statements,
    Categories,
    Duplicates,
    Journal,
    Settings,
}

impl Screen {
    pub const ALL: [Screen; 13] = [
        Screen::Home,
        Screen::Accounts,
        Screen::Operations,
        Screen::Flows,
        Screen::Analytics,
        Screen::Budget,
        Screen::Debts,
        Screen::Counterparties,
        Screen::Categories,
        Screen::Duplicates,
        Screen::Statements,
        Screen::Journal,
        Screen::Settings,
    ];
    pub const COUNT: usize = 13;

    pub fn title(self) -> &'static str {
        match self {
            Screen::Home => trs!("Home", "Обзор"),
            Screen::Accounts => trs!("Accounts", "Счета"),
            Screen::Operations => trs!("Operations", "Операции"),
            Screen::Analytics => trs!("Analytics", "Аналитика"),
            Screen::Budget => trs!("Budget", "Бюджет"),
            Screen::Flows => trs!("Flows", "Потоки"),
            Screen::Debts => trs!("Debts", "Долги"),
            Screen::Counterparties => trs!("Counterparties", "Контрагенты"),
            Screen::Statements => trs!("Statements", "Выписки"),
            Screen::Duplicates => trs!("Duplicates", "Дубли"),
            Screen::Categories => trs!("Categories", "Категории"),
            Screen::Journal => trs!("Changes", "Правки"),
            Screen::Settings => trs!("Settings", "Настройки"),
        }
    }

    /// For a narrow terminal.
    pub fn short(self) -> &'static str {
        match self {
            Screen::Home => trs!("Home", "Обзор"),
            Screen::Accounts => trs!("Acc", "Счета"),
            Screen::Analytics => trs!("Stats", "Анал"),
            Screen::Budget => trs!("Budget", "Бюдж"),
            Screen::Operations => trs!("Ops", "Опер"),
            Screen::Flows => trs!("Flows", "Потоки"),
            Screen::Debts => trs!("Debts", "Долги"),
            Screen::Counterparties => trs!("Cpty", "Контр"),
            Screen::Statements => trs!("Stmt", "Вып"),
            Screen::Duplicates => trs!("Dup", "Дубли"),
            Screen::Categories => trs!("Cat", "Кат"),
            Screen::Journal => trs!("Chg", "Правки"),
            Screen::Settings => trs!("Set", "Настр"),
        }
    }

    /// Two letters, for when even the short names do not fit — 80 columns,
    /// or 100 in Russian, where the tabs used to be bare digits.
    pub fn tiny(self) -> &'static str {
        match self {
            Screen::Home => trs!("Ho", "Об"),
            Screen::Accounts => trs!("Ac", "Сч"),
            Screen::Analytics => trs!("An", "Ан"),
            Screen::Budget => trs!("Bu", "Бю"),
            Screen::Operations => trs!("Op", "Оп"),
            Screen::Flows => trs!("Fl", "По"),
            Screen::Debts => trs!("De", "До"),
            Screen::Counterparties => trs!("Co", "Ко"),
            Screen::Statements => trs!("St", "Вы"),
            Screen::Duplicates => trs!("Du", "Ду"),
            Screen::Categories => trs!("Ca", "Ка"),
            Screen::Journal => trs!("Ch", "Пр"),
            Screen::Settings => trs!("Se", "На"),
        }
    }

    /// The key that opens it: digits for the first ten, `0` for the tenth;
    /// the rest are reached with `[` and `]`.
    pub fn key(self) -> String {
        match Screen::ALL.iter().position(|s| *s == self).unwrap_or(0) {
            i @ 0..=8 => (i + 1).to_string(),
            9 => "0".into(),
            _ => "·".into(),
        }
    }
}

/// A key's name as `action::BINDINGS` knows it.
///
/// One translation from an event into a string, so that bindings are described
/// in words rather than by parsing `KeyEvent` in three places.
fn key_name(key: KeyEvent) -> Option<String> {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let name = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "shift+tab".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Enter => "enter".to_string(),
        _ => return None,
    };
    // `BackTab` already carries shift in it; for the rest add it explicitly, so
    // that `shift+tab` works where the terminal sends Tab with a modifier.
    if shift && name == "tab" {
        return Some("shift+tab".to_string());
    }
    Some(name)
}

/// The interface. `cloud` carries the Drive check running beside it: what it
/// answers reaches the status line, and a mark of another device turns the
/// screen read-only where it stands. Returns whether this device writes at
/// the end — the one that then sends and lets go of the mark.
pub fn run(conn: Connection, cloud: Option<std::sync::mpsc::Receiver<crate::cloud::Start>>) -> Result<bool> {
    let mut terminal = enter()?;
    let result = main_loop(&mut terminal, conn, cloud);
    leave(&mut terminal)?;
    result
}

fn enter() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    out.execute(EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

fn leave(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    conn: Connection,
    cloud: Option<std::sync::mpsc::Receiver<crate::cloud::Start>>,
) -> Result<bool> {
    let mut app = App::new(conn)?;
    // The program opens on the overview; tests build `App` and start where
    // they say.
    app.go_to(Screen::Home);
    if app.lock_hash().is_some() {
        app.mode = Mode::Locked;
        app.locked_at = Some(chrono::Local::now());
    }
    let mut last_key = std::time::Instant::now();
    // The first screen grows too: otherwise the interface opens with a jerk.
    app.grew_at = Some(std::time::Instant::now());
    let mut shown = app.screen;
    // Rates once a day, if asked — in the background on a connection of its
    // own: a slow bank must not hold the screen.
    // Not while read only, and not while waiting on Google Drive's start check:
    // fetched rates are a change, and it would be this device's against the writer's.
    let spawn_rates = |app: &App| {
        app.conn.path().filter(|p| !p.is_empty() && app.read_only.is_none()).map(std::path::PathBuf::from).map(|path| {
            let (send, receive) = std::sync::mpsc::channel();
            let today = app.today;
            std::thread::spawn(move || {
                let done = Connection::open(&path).map_err(anyhow::Error::from).and_then(|conn| {
                    // The screen may be writing at the same moment: wait for it.
                    conn.busy_timeout(std::time::Duration::from_secs(10))?;
                    potok_core::settings::apply_language(&conn)?;
                    potok_core::rate::update_if_due(&conn, today)
                });
                let _ = send.send(done.map_err(|e| e.to_string()));
            });
            receive
        })
    };
    let mut cloud_pending = cloud.is_some();
    let mut rates = if cloud_pending { None } else { spawn_rates(&app) };
    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        if app.quit {
            return Ok(app.read_only.is_none() && !cloud_pending);
        }
        if let Some(Ok(Ok(Some(done)))) = rates.as_ref().map(|r| r.try_recv()) {
            app.reload()?;
            app.status = Some(tr!("rates fetched: {}", "курсы загружены: {}", done.stored));
        }
        // Drive answered. Another device's mark turns this screen read-only
        // where it stands; a copy taken from Drive is already in place, so
        // everything on screen is re-read.
        if let Some(Ok(start)) = cloud.as_ref().map(|c| c.try_recv()) {
            cloud_pending = false;
            app.status = crate::cloud::start_note(&start);
            match &start {
                crate::cloud::Start::ReadOnly(mark) => {
                    let _ = app.conn.execute_batch("pragma query_only = 1;");
                    app.read_only = Some(mark.clone());
                    app.asking = None;
                    app.opening_for = None;
                    app.pending = None;
                    app.form = None;
                    app.review = None;
                    app.queue = false;
                    if app.mode != Mode::Locked {
                        app.mode = Mode::Normal;
                    }
                    app.status = Some(tr!("{} writes since {} — W takes writing over", "пишет {} с {} — W забрать запись", mark.device, mark.since_local()));
                    app.reload()?;
                }
                crate::cloud::Start::Writer { .. } => {
                    app.reload()?;
                    rates = spawn_rates(&app);
                }
                crate::cloud::Start::Unreachable(_) | crate::cloud::Start::Off => {
                    app.reload()?;
                    rates = spawn_rates(&app);
                }
            }
        }

        // Idle long enough: lock on its own.
        if app.mode != Mode::Locked {
            if let Some(minutes) = app.lock_after() {
                if last_key.elapsed() >= Duration::from_secs(minutes * 60) {
                    app.lock();
                }
            }
        }
        // While the bars grow, frames come oftener: a third of a second at four
        // frames looks jerky. At rest — as before, four times a second.
        let wait = if app.growth() < 1.0 { 16 } else { 250 };
        if !event::poll(Duration::from_millis(wait))? {
            continue;
        }
        match event::read()? {
            // A resize arrives here and by itself calls for a redraw. In the old
            // client the input stream forwarded key presses only and dropped
            // everything else, so a narrowed window showed the old frame until
            // the next key — on a screen made of figures that looks like real
            // figures. Here the redraw happens at the start of every turn, and
            // the event is needed only to make the turn happen.
            Event::Resize(_, _) => continue,
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                last_key = std::time::Instant::now();
                // Taking writing over goes to Drive and back: say so first,
                // or the screen looks as if the key did nothing.
                if key.code == KeyCode::Char('W') && app.read_only.is_some() && app.mode == Mode::Normal {
                    app.status = Some(trs!("taking writing over — Google Drive…", "забираю запись — Google Drive…").into());
                    terminal.draw(|frame| ui::draw(frame, &app))?;
                }
                // A refusal is a line on the screen, not the end of the
                // program: a failed write left the ledger as it was.
                if let Err(error) = handle_key(&mut app, key).and_then(|()| app.refresh_detail()) {
                    app.mode = app::Mode::Normal;
                    app.status = Some(app.explain(&error));
                }
                // Screen changed or reread — the bars grow from zero.
                if app.screen != shown || key.code == KeyCode::Char('r') {
                    shown = app.screen;
                    app.grew_at = Some(std::time::Instant::now());
                }
            }
            _ => continue,
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Typing takes the whole keyboard: otherwise a «q» in the search line or in
    // a category's name would close the program.
    if app.mode == Mode::Search {
        match key.code {
            KeyCode::Esc => app.cancel_search()?,
            KeyCode::Enter => app.mode = Mode::Normal,
            KeyCode::Backspace => {
                app.input.pop();
                app.apply_search()?;
            }
            KeyCode::Char(c) => {
                app.input.push(c);
                app.apply_search()?;
            }
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Locked {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') if ctrl => app.quit = true,
            KeyCode::Enter => app.unlock(),
            // While a wrong passphrase is being waited out, typing is ignored:
            // otherwise keys typed during the wait pile up into the next try.
            _ if app.lock_wait() > 0 => {}
            KeyCode::Esc => app.input.clear(),
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Form {
        match key.code {
            KeyCode::Esc => app.cancel_form(),
            KeyCode::Enter => app.save_form()?,
            KeyCode::Tab | KeyCode::Down => app.form_move(1),
            KeyCode::BackTab | KeyCode::Up => app.form_move(-1),
            KeyCode::Left => app.form_choose(-1),
            KeyCode::Right => app.form_choose(1),
            KeyCode::Backspace => app.form_backspace(),
            KeyCode::Char(c) => app.form_type(c),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Input {
        match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.asking = None;
                app.status = Some(trs!("nothing changed", "ничего не изменено").into());
            }
            KeyCode::Enter => app.apply_input()?,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Opening {
        match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.opening_for = None;
                app.status = Some(trs!("balance not changed", "остаток не изменён").into());
            }
            KeyCode::Enter => app.apply_opening()?,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Review {
        app.status = None;
        match key.code {
            KeyCode::Esc => app.ask_drop_review(),
            KeyCode::Char('?') => {
                app.status = Some(
                    trs!(
                        "[✓] written · [ ] left out (Space) · the hint after the amount is taken unless turned down (Enter)",
                        "[✓] запишется · [ ] нет (Space) · подсказка после суммы примется, если не отклонить (Enter)"
                    )
                    .into(),
                )
            }
            KeyCode::Up | KeyCode::Char('k') => app.move_review(-1),
            KeyCode::Down | KeyCode::Char('j') => app.move_review(1),
            KeyCode::PageUp => app.move_review(-10),
            KeyCode::PageDown => app.move_review(10),
            KeyCode::Char(' ') => app.toggle_review_write(),
            KeyCode::Enter => app.toggle_review_hint(),
            KeyCode::Char('w') | KeyCode::Char('ц') => app.ask_write_import(),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Path {
        match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.status = Some(trs!("import cancelled", "загрузка отменена").into());
            }
            KeyCode::Enter => app.apply_path()?,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Reconcile {
        match key.code {
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.status = Some(trs!("reconciliation cancelled", "сверка отменена").into());
            }
            KeyCode::Enter => app.apply_reconcile()?,
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => app.input.push(c),
            _ => {}
        }
        return Ok(());
    }

    if let Mode::PickCategory { .. } = app.mode {
        match key.code {
            KeyCode::Esc if app.queue => app.stop_queue(),
            KeyCode::Tab if app.queue => app.queue_skip(),
            KeyCode::Esc => {
                app.mode = Mode::Normal;
                app.input.clear();
            }
            KeyCode::Enter => app.apply_pick()?,
            KeyCode::Up => app.move_pick(-1),
            KeyCode::Down => app.move_pick(1),
            KeyCode::Backspace => {
                app.input.pop();
                app.type_into_pick();
            }
            KeyCode::Char(c) => {
                app.input.push(c);
                app.type_into_pick();
            }
            _ => {}
        }
        return Ok(());
    }

    if let Mode::Menu { .. } = app.mode {
        match key.code {
            KeyCode::Esc => app.mode = Mode::Normal,
            KeyCode::Enter => app.choose_menu()?,
            KeyCode::Up | KeyCode::Char('k') => app.move_menu(-1),
            KeyCode::Down | KeyCode::Char('j') => app.move_menu(1),
            _ => {}
        }
        return Ok(());
    }

    if app.mode == Mode::Confirm {
        // «y» alone means yes. Any other key means no: what moves money must
        // not happen because a finger missed.
        app.answer(matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('н')))?;
        return Ok(());
    }

    if app.mode == Mode::Help {
        // Any key closes the help — and only closes it: carrying out that key's
        // own action as well would be a surprise.
        app.mode = Mode::Normal;
        return Ok(());
    }

    // `q` once asks, `q` twice leaves; any other key stays. One stray key
    // must not close the ledger in the middle of work.
    let armed = std::mem::take(&mut app.quit_armed);
    let Some(name) = key_name(key) else { return Ok(()) };
    let Some(action) = action::resolve(&name, app.screen) else { return Ok(()) };

    app.status = None;
    match action {
        Action::Quit if armed => app.quit = true,
        Action::Quit => {
            app.quit_armed = true;
            app.status = Some(tr!("q again — quit · any other key — stay", "ещё раз q — выйти · любая другая клавиша — остаться"));
        }
        Action::Help => app.mode = Mode::Help,
        Action::Reload => {
            app.reload()?;
            app.status = Some(trs!("reloaded", "перечитано").to_string());
        }
        Action::NextScreen => app.next_screen(1),
        Action::PrevScreen => app.next_screen(-1),
        Action::GoTo(screen) => app.go_to(screen),
        // In the details the cursor is its own: the arrows move it, not the list.
        // The window is modal — while it is open the list under it stays put.
        Action::Up if app.focus == app::Focus::Detail || app.zoom => app.move_detail_cursor(-1),
        Action::Down if app.focus == app::Focus::Detail || app.zoom => app.move_detail_cursor(1),
        Action::PageUp if app.focus == app::Focus::Detail || app.zoom => app.move_detail_cursor(-10),
        Action::PageDown if app.focus == app::Focus::Detail || app.zoom => app.move_detail_cursor(10),
        Action::First if app.focus == app::Focus::Detail || app.zoom => app.move_detail_cursor(-(isize::MAX / 2)),
        Action::Last if app.focus == app::Focus::Detail || app.zoom => app.move_detail_cursor(isize::MAX / 2),
        Action::FocusDetail | Action::FocusList | Action::SwitchPane if app.zoom => {}
        Action::Up => {
            app.move_cursor(-1);
            app.detail_cursor = 0;
        }
        Action::Down => {
            app.move_cursor(1);
            app.detail_cursor = 0;
        }
        Action::PageUp => {
            app.move_cursor(-10);
            app.detail_cursor = 0;
        }
        Action::PageDown => {
            app.move_cursor(10);
            app.detail_cursor = 0;
        }
        Action::First => {
            app.go_first();
            app.detail_cursor = 0;
        }
        Action::Last => {
            app.go_last();
            app.detail_cursor = 0;
        }
        Action::CycleAccount => app.cycle_account()?,
        Action::CyclePeriod => app.cycle_period()?,
        Action::Reconcile
        | Action::ImportStatement
        | Action::NewOperation
        | Action::CategorizeAll
        | Action::Categorize
            if app.read_only.is_some() =>
        {
            app.status = app.read_only_note();
        }
        Action::Open if app.read_only.is_some() && app.screen == Screen::Settings => {
            app.status = app.read_only_note();
        }
        Action::Open => app.open()?,
        Action::Search => app.start_search()?,
        Action::Back => app.back()?,
        Action::Menu => app.open_menu()?,
        Action::FocusDetail => app.focus_detail(),
        Action::FocusList => app.focus_list(),
        Action::SwitchPane => app.switch_pane(name != "shift+tab")?,
        Action::Reconcile => app.start_reconcile(),
        Action::ImportStatement => app.start_import(),
        Action::NewOperation => app.start_form(),
        Action::Dates => app.run_action(app::MenuAction::Ask(app::Ask::Range))?,
        Action::Fold => app.toggle_fold(),
        Action::Order => app.next_order(),
        Action::Mark => app.toggle_mark(),
        Action::Transfers => app.toggle_transfers()?,
        Action::TakeWrite => app.take_write()?,
        Action::Lock => app.lock(),
        Action::CategorizeAll => app.start_queue(),
        Action::Categorize => {
            if app.selected_counterparty().is_some() {
                app.start_counterparty_pick();
            } else {
                app.status = Some(trs!("nobody to categorise", "некого разметить").to_string());
            }
        }
    }
    Ok(())
}

/// Draws every screen into text of a given size.
///
/// A tool for the eyes, not for tests: in the old client it was looking at all
/// the screens at 150x44, 100x30 and 80x24 that found most of the layout
/// defects — «found this way rather than reasoned out», as it is written there.
/// The tests check that a screen draws; this shows how it looks.
pub fn screens(conn: Connection, width: u16, height: u16) -> Result<String> {
    use ratatui::backend::TestBackend;

    let mut app = App::new(conn)?;
    let mut out = String::new();
    let mut shot = |app: &mut App, title: &str| -> Result<()> {
        app.refresh_detail()?;
        let app = &*app;
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| ui::draw(frame, app))?;
        let buffer = terminal.backend().buffer().clone();
        out.push_str(&format!("\n──── {title} ({width}x{height}) ────\n"));
        for y in 0..buffer.area.height {
            let row: String =
                (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect();
            out.push_str(row.trim_end());
            out.push('\n');
        }
        Ok(())
    };
    for screen in Screen::ALL {
        app.screen = screen;
        shot(&mut app, screen.title())?;
    }

    // States reached by a key press: they have to be seen too, not only the
    // screens at rest.
    app.screen = Screen::Flows;
    app.drill()?;
    shot(&mut app, trs!("Flows → Enter on the first row", "Потоки → Enter на первой строке"))?;
    app.back()?;

    app.start_search()?;
    app.input = "kaufland".into();
    app.apply_search()?;
    shot(&mut app, trs!("/ search «kaufland»", "/ поиск «kaufland»"))?;
    app.cancel_search()?;

    app.screen = Screen::Counterparties;
    let first_hinted = app
        .visible_counterparties()
        .iter()
        .position(|line| line.category.is_none() && line.hint.is_some());
    if let Some(at) = first_hinted {
        for _ in 0..at {
            app.move_cursor(1);
        }
        app.start_pick();
        shot(&mut app, trs!("Import → c on a counterparty with a hint", "Импорт → c на контрагенте с подсказкой"))?;
        app.mode = app::Mode::Normal;
    }

    // A debt opened up: the ribbon of payments is what people come there for.
    app.screen = Screen::Debts;
    app.zoom = true;
    shot(&mut app, trs!("Debts → Enter (expanded)", "Долги → Enter (развёрнуто)"))?;
    app.zoom = false;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digit that opens a screen — tests follow `Screen::ALL`, not a
    /// remembered number.
    fn k(screen: Screen) -> char {
        screen.key().chars().next().unwrap()
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        App::new(potok_core::db::open_memory().unwrap()).unwrap()
    }

    /// Your case in miniature: the Northwind debt, with a 720 refund tied to it
    /// twice and a utilities payment of 23.93.
    fn northwind() -> App {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',100000);
             insert into counterparty (name) values ('Northwind'), ('Epay');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'debt_movement','2026-09-03T00:00:00Z',72000,'EUR',-72000,'NORTHWIND return',1),
               (2,1,'debt_movement','2026-09-03T00:00:00Z',72000,'EUR',-72000,'NORTHWIND return',1),
               (3,1,'debt_movement','2026-06-16T00:00:00Z',2393,'EUR',-2393,'Epay To: Epay Utilities',2);
             insert into debt (counterparty_id,direction,currency,opened_on) values (1,'I_OWE','EUR','2026-01-30');
             insert into debt_movement (id,debt_id,operation_id,kind,amount_minor,occurred_on) values
               (1,1,null,'ISSUE',220000,'2026-01-30'),
               (2,1,3,'REPAYMENT',2393,'2026-06-16'),
               (3,1,1,'REPAYMENT',72000,'2026-09-03'),
               (4,1,2,'REPAYMENT',72000,'2026-09-03');",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.refresh_detail().unwrap();
        app
    }

    fn keys(app: &mut App, codes: &[KeyCode]) {
        for code in codes {
            handle_key(app, press(*code)).unwrap();
            app.refresh_detail().unwrap();
        }
    }

    /// Topping up Revolut from iCard: a transfer between one's own, two legs.
    fn apple_pay() -> App {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('iCard','EUR','CARD'), ('Revolut','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'transfer','2026-10-16T00:00:00Z',1000,'EUR',-1000,'Revolut** Dublin'),
               (2,2,'transfer','2026-10-16T00:00:00Z',1000,'EUR', 1000,'Apple Pay deposit');
             update operation set pair_id = 2 where id = 1;
             update operation set pair_id = 1 where id = 2;",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.refresh_detail().unwrap();
        app
    }

    /// A top-up recorded twice (as a transfer and as a spend), and two real
    /// fees on one day.
    fn copies() -> App {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',50000), ('Revolut','EUR','BANK',0);
             insert into counterparty (id,name) values (1,'Revolut'), (2,'Fee');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'transfer','2026-09-02T12:00:00Z',10000,'EUR',-10000,'Revolut** Dublin',1),
               (2,2,'transfer','2026-09-02T12:00:00Z',10000,'EUR', 10000,'Top-up',null),
               (3,1,'expense', '2026-09-02T12:00:00Z',10000,'EUR',-10000,'Revolut** Dublin, -100.00 EUR',1),
               (4,1,'expense', '2026-09-03T12:00:00Z',50,'EUR',-50,'Fee SEPA',2),
               (5,1,'expense', '2026-09-03T12:00:00Z',50,'EUR',-50,'Fee SEPA',2);
             update operation set pair_id = 2 where id = 1;
             update operation set pair_id = 1 where id = 2;",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.refresh_detail().unwrap();
        app
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            keys(app, &[KeyCode::Char(c)]);
        }
    }

    /// Reconciling from the keyboard: s, the remainder, Enter — and the gap
    /// shows by the account. Removed the copy on the duplicates screen — it fit.
    #[test]
    fn a_copy_is_found_reconciled_and_removed_from_the_keyboard() {
        let mut app = copies();
        while app.accounts[app.cursor()].name != "iCard" {
            keys(&mut app, &[KeyCode::Down]);
        }
        keys(&mut app, &[KeyCode::Char('s')]);
        assert_eq!(app.mode, Mode::Reconcile);
        typed(&mut app, "399 2026-09-03");
        keys(&mut app, &[KeyCode::Enter]);
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("off by -100.00"), "{status}");
        let app::Detail::Account(d) = &app.detail else { panic!("{:?}", app.detail) };
        assert!(!d.reconciliation.as_ref().unwrap().matches());

        keys(&mut app, &[KeyCode::Char(k(Screen::Duplicates))]);
        assert_eq!(app.duplicates.len(), 2);
        // the first from the top is the fees: those are real
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Not a duplicate");
        assert_eq!(app.duplicates.len(), 1);
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Remove #3");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert!(app.duplicates.is_empty(), "{:?}", app.status);

        keys(&mut app, &[KeyCode::Char(k(Screen::Accounts))]);
        let app::Detail::Account(d) = &app.detail else { panic!() };
        assert!(d.reconciliation.as_ref().unwrap().matches(), "{:?}", d.reconciliation);
    }

    /// «This is not Other but medicine»: from the Overview drill into a «where
    /// it went» row, fix one operation — and be offered to move the rest of the
    /// same shop's operations, remembered for the future.
    #[test]
    fn home_drills_into_where_it_went_and_one_fix_moves_the_whole_shop() {
        let conn = potok_core::db::open_memory().unwrap();
        let today = chrono::Local::now().date_naive();
        conn.execute_batch(&format!(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',100000);
             insert into category (name) values ('Other'), ('Healthcare');
             insert into counterparty (name) values ('Pharmacy');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,1,'expense','{today}T10:00:00Z',1000,'EUR',-1000,'PHARMACY SOFIA',1,1),
               (2,1,'expense','{today}T11:00:00Z',2000,'EUR',-2000,'PHARMACY SOFIA',1,1),
               (3,1,'expense','{today}T12:00:00Z',3000,'EUR',-3000,'PHARMACY SOFIA',1,1);"
        ))
        .unwrap();
        let mut app = App::new(conn).unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Home))]);
        assert_eq!(app.rows_on(Screen::Home), 1, "one «where it went» row");
        assert_eq!(app.home_flows()[0].label, "Other");

        // Enter drills into this row's operations.
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        assert_eq!(app.return_to, Some(Screen::Home), "Esc returns to the Overview");
        assert_eq!(app.visible_operations().len(), 3, "every operation of the row");

        // One is fixed by hand.
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "A category for this operation only");
        for c in "Healthcare".chars() {
            keys(&mut app, &[KeyCode::Char(c)]);
        }
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Confirm, "asks about the rest: {:?}", app.status);
        let asked = app.pending.as_ref().and_then(|p| p.confirm.clone()).unwrap_or_default();
        assert!(asked.contains("Pharmacy") && asked.contains("Healthcare"), "{asked}");

        // «No» leaves the edit on the one — and tells the truth.
        keys(&mut app, &[KeyCode::Char('n')]);
        assert_eq!(app.home_flows().len(), 2, "one went to Healthcare, the rest to Other");

        // And «yes» moves the rest and remembers the rule on the counterparty.
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "A category for this operation only");
        for c in "Healthcare".chars() {
            keys(&mut app, &[KeyCode::Char(c)]);
        }
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char('y')]);
        keys(&mut app, &[KeyCode::Char(k(Screen::Home))]);
        // Nobody touched the third: it follows the counterparty's rule.
        assert_eq!(app.home_flows().len(), 1, "all three in one row: {:?}", app.status);
        assert_eq!(app.home_flows()[0].label, "Healthcare", "{:?}", app.status);
    }

    /// The Overview's right pane is a list, not labels: Tab, the arrows, Enter
    /// does what the row says.
    #[test]
    fn the_right_side_of_home_is_walked_and_enter_does_what_it_says() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',100000);
             insert into counterparty (name) values ('Pharmacy');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'expense','2026-10-01T00:00:00Z',1000,'EUR',-1000,'PHARMACY',1);
             insert into debt (id,counterparty_id,direction,currency,opened_on) values (1,1,'I_OWE','EUR','2026-01-30');
             insert into debt_movement (debt_id,kind,amount_minor,occurred_on) values (1,'ISSUE',50000,'2026-01-30');",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Home))]);

        // A counterparty with no category and an open debt — both rows on the right.
        let look = app.home_look();
        assert!(look.iter().any(|r| r.go == app::HomeGo::Categorise), "{:?}", look.iter().map(|r| &r.text).collect::<Vec<_>>());
        assert_eq!(app.home_right_len(), look.len() + 1, "plus the debt row");

        // Tab takes the cursor right, Enter on the debt opens «Debts».
        keys(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.focus, app::Focus::Detail, "the cursor is on the right");
        for _ in 0..look.len() {
            keys(&mut app, &[KeyCode::Down]);
        }
        assert_eq!(app.home_right_go(app.detail_cursor), Some(app::HomeGo::Debt(1)));
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Debts, "{:?}", app.status);

        // And Enter on «counterparties with no category» opens marking in a row.
        keys(&mut app, &[KeyCode::Char(k(Screen::Home)), KeyCode::Tab]);
        let at = look.iter().position(|r| r.go == app::HomeGo::Categorise).unwrap();
        for _ in 0..at {
            keys(&mut app, &[KeyCode::Down]);
        }
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Counterparties);
        assert!(app.queue, "marking in a row has begun: {:?}", app.status);
    }

    /// The month columns are the Overview's third list: Tab reaches them, the
    /// arrows walk the months, Enter opens that month's operations.
    #[test]
    fn the_month_columns_of_home_are_walked_and_opened() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('iCard','EUR','CARD',100000);
             insert into counterparty (name) values ('Shop');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'expense','2026-08-04T00:00:00Z',1000,'EUR',-1000,'SHOP',1),
               (2,1,'expense','2026-09-05T00:00:00Z',2000,'EUR',-2000,'SHOP',1),
               (3,1,'expense','2026-10-06T00:00:00Z',3000,'EUR',-3000,'SHOP',1);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.today = chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        app.reload().unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Home))]);
        let months = app.home_months();
        assert!(months.len() >= 3, "the months since August: {}", months.len());

        // The first Tab — «where it went» → the months.
        keys(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.home_pane, app::HomePane::Months);
        assert_eq!(app.focus, app::Focus::List);

        // The cursor starts on this month, not on the first that came along.
        assert_eq!(app.home_month_at(), months.len() - 1, "the last column is this month");

        // The arrows walk the columns, not the list under them.
        let flows_cursor = app.cursor();
        keys(&mut app, &[KeyCode::Up, KeyCode::Up]);
        assert_eq!(app.home_month_at(), months.len() - 3, "the cursor moved two columns left");
        assert_eq!(app.cursor(), flows_cursor, "«where it went» does not stir");

        // «Where it went» shows the chosen month rather than this one, and
        // drilling in goes by its bounds.
        let past = app.home_past.as_ref().expect("a past month is chosen");
        assert_eq!(past.at, months.len() - 3);
        let (from, to) = app.month_bounds(months[months.len() - 3].start);
        assert_eq!((past.from.clone(), past.to.clone()), (from.clone(), to.clone()));
        assert_eq!(app.home_bounds(), (from.clone(), to.clone()), "the «where it went» row opens for that month");
        assert!(app.home_flows().iter().any(|f| !f.out.is_empty()), "something happened in that month: {:?}", app.home_flows().len());

        // Back to this column — and «where it went» is about this month again.
        keys(&mut app, &[KeyCode::Down, KeyCode::Down]);
        assert!(app.home_past.is_none(), "this month is not marked as past");

        keys(&mut app, &[KeyCode::Up, KeyCode::Up]);

        // Enter — this month's operations, Esc goes back to the Overview.
        let month = app.home_months()[months.len() - 3].clone();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        assert_eq!(app.return_to, Some(Screen::Home));
        let (from, to) = app.month_bounds(month.start);
        assert_eq!(app.ops_view, app::OpsView::Range { from, to });
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Home);

        // And one more Tab reaches the right stack.
        keys(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.focus, app::Focus::Detail, "{:?}", app.home_pane);
    }

    /// A counterparty's card is editable throughout: category, name, note and
    /// spellings — the last right from the cursor on them.
    #[test]
    fn the_counterparty_card_takes_a_spelling_and_gives_it_back() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('iCard','EUR','CARD');
             insert into counterparty (id,name) values (1,'APTEKA MIR');
             insert into counterparty_alias (alias,counterparty_id) values ('apteka mir',1);
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
               (1,1,'expense','2026-10-01T00:00:00Z',480,'EUR',-480,'APTEKA MIR 2 SOFIA BGR',1);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.go_to(Screen::Counterparties);
        app.refresh_detail().unwrap();

        // Adding a spelling is a card item, the key derived from the bank's text.
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Add a spelling");
        // Normalisation strips the shop number and the city itself, so «BENU
        // OPERA KASA 1» would give the same key; tie another spelling instead.
        for c in "APTEKA NORD SOFIA BGR".chars() {
            keys(&mut app, &[KeyCode::Char(c)]);
        }
        keys(&mut app, &[KeyCode::Enter]);
        let aliases = |app: &App| potok_core::detail::counterparty(&app.conn, "APTEKA MIR").unwrap().unwrap().aliases;
        assert_eq!(aliases(&app), ["apteka mir", "apteka nord"], "{:?}", app.status);

        // In the card the cursor lands on a spelling, and it can be removed.
        keys(&mut app, &[KeyCode::Right, KeyCode::Down]);
        assert_eq!(app.detail_items().first(), Some(&app::DetailItem::Alias("apteka mir".into())));
        assert_eq!(app.detail_cursor, 1, "the second spelling");
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Remove the spelling");
        assert_eq!(app.mode, Mode::Confirm, "a spelling is removed with a question");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(aliases(&app), ["apteka mir"], "{:?}", app.status);

        // And it is an edit like any other: undoable.
        let entry: i64 = app.conn.query_row("select max(id) from change_log where undo like 'alias%'", [], |r| r.get(0)).unwrap();
        potok_core::journal::undo(&mut app.conn, entry).unwrap();
        assert_eq!(aliases(&app), ["apteka mir", "apteka nord"], "undo brought the spelling back");
    }

    /// `c` on a counterparty takes with it those of its operations that sit in
    /// the old category: after the move from saldo it is written on every row,
    /// and without this the counterparty's rule would touch none of them.
    #[test]
    fn categorising_a_counterparty_takes_its_operations_out_of_the_old_category() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');
             insert into category (id,name) values (1,'Other'), (2,'Utilities');
             insert into counterparty (id,name,category_id) values (1,'ePay.bg',1);
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,1,'expense','2026-10-13T00:00:00Z',2771,'EUR',-2771,'ePay.bg To: Epay Utilities',1,1),
               (2,1,'expense','2026-09-24T00:00:00Z', 632,'EUR', -632,'ePay.bg To: Epay Utilities',1,null);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.go_to(Screen::Counterparties);
        app.refresh_detail().unwrap();

        keys(&mut app, &[KeyCode::Char('c')]);
        for c in "Utilities".chars() {
            keys(&mut app, &[KeyCode::Char(c)]);
        }
        keys(&mut app, &[KeyCode::Enter]);

        let category = |app: &App, id: i64| -> Option<String> {
            potok_core::query::operations(&app.conn, None, false).unwrap().into_iter().find(|o| o.id == id).and_then(|o| o.category)
        };
        assert_eq!(category(&app, 1).as_deref(), Some("Utilities"), "the own «Other» is stripped: {:?}", app.status);
        assert_eq!(category(&app, 2).as_deref(), Some("Utilities"), "and the one that already followed the counterparty");
    }

    /// A note on an operation: what the bank did not write. Editable always —
    /// the amount and date of a posted one are not touched, and this is not money.
    #[test]
    fn an_operation_takes_a_note_of_its_own() {
        let mut app = northwind();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('x')]);
        choose(&mut app, "A note of your own");
        typed(&mut app, "return to Kolya");
        keys(&mut app, &[KeyCode::Enter]);
        let id = app.visible_operations()[0].id;
        let note = |app: &App| potok_core::detail::operation(&app.conn, id).unwrap().unwrap().note;
        assert_eq!(note(&app).as_deref(), Some("return to Kolya"), "{:?}", app.status);

        // And it is undoable, like any edit.
        let entry: i64 = app.conn.query_row("select max(id) from change_log where undo like 'operation-note%'", [], |r| r.get(0)).unwrap();
        potok_core::journal::undo(&mut app.conn, entry).unwrap();
        assert_eq!(note(&app), None, "undo cleared the note");
    }

    /// «Category for every operation of «X»» from an operation's menu: it marks
    /// the one paid in that row, not the one under the cursor on another screen.
    #[test]
    fn a_category_for_the_whole_counterparty_is_set_from_the_operation_menu() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');
             insert into category (id,name) values (1,'Other'), (2,'Transport');
             insert into counterparty (id,name,category_id) values (1,'Urban Mobility Center',1), (2,'Billa',null);
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,1,'expense','2026-10-09T00:00:00Z', 80,'EUR', -80,'Urban Mobility Center',1,1),
               (2,1,'expense','2026-10-08T00:00:00Z',110,'EUR',-110,'Urban Mobility Center',1,null),
               (3,1,'expense','2026-10-07T00:00:00Z',900,'EUR',-900,'Billa',2,null);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations))]);
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "A category for every operation of");
        for c in "Transport".chars() {
            keys(&mut app, &[KeyCode::Char(c)]);
        }
        keys(&mut app, &[KeyCode::Enter]);

        let category = |app: &App, id: i64| -> Option<String> {
            potok_core::query::operations(&app.conn, None, false).unwrap().into_iter().find(|o| o.id == id).and_then(|o| o.category)
        };
        assert_eq!(category(&app, 1).as_deref(), Some("Transport"), "the own «Other» is stripped: {:?}", app.status);
        assert_eq!(category(&app, 2).as_deref(), Some("Transport"), "and the one that followed the counterparty");
        assert_eq!(category(&app, 3), None, "another counterparty was left alone");
    }

    /// The marked rows get every role, not three of five: transit was missing
    /// for them, a person took the row item — and the wrong one became transit.
    #[test]
    fn marked_operations_can_be_made_transit_together() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'expense','2026-10-09T00:00:00Z',10500,'EUR',-10500,'to myself'),
               (2,1,'income', '2026-10-09T00:00:00Z',10500,'EUR', 10500,'top-up'),
               (3,1,'expense','2026-10-08T00:00:00Z', 1200,'EUR', -1200,'coffee');",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations))]);
        keys(&mut app, &[KeyCode::Char(' '), KeyCode::Down, KeyCode::Char(' ')]);
        assert_eq!(app.marked.len(), 2);
        let marked: Vec<i64> = app.marked.iter().copied().collect();

        keys(&mut app, &[KeyCode::Char('x')]);
        let labels: Vec<&str> = app.menu.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.starts_with("The 2 marked: transit")), "{labels:?}");
        assert!(labels.iter().any(|l| l.starts_with("This row only:")), "the row item says it is the row: {labels:?}");
        choose(&mut app, "The 2 marked: transit");

        let role = |app: &App, id: i64| -> String {
            potok_core::query::operations(&app.conn, None, false).unwrap().into_iter().find(|o| o.id == id).unwrap().role
        };
        for id in &marked {
            assert_eq!(role(&app, *id), "passthrough", "the marked #{id}: {:?}", app.status);
        }
        let untouched = [1, 2, 3].into_iter().find(|id| !marked.contains(id)).unwrap();
        assert_ne!(role(&app, untouched), "passthrough", "the unmarked #{untouched} was left alone");
    }

    /// Nonsense instead of an amount — an answer in the status line, not a panic.
    #[test]
    fn a_bad_amount_is_answered_not_crashed() {
        let mut app = copies();
        keys(&mut app, &[KeyCode::Char('s')]);
        typed(&mut app, "one hundred euro");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.status.clone().unwrap_or_default().starts_with("not reconciled"), "{:?}", app.status);
        assert_eq!(app.mode, Mode::Normal);
    }

    /// A month of a Revolut statement: a top-up from iCard (whose second leg is
    /// already in the ledger) and a transfer to a friend.
    const REVOLUT_OCTOBER: &str = "\
EUR Statement
Revolut Bank UAB
 Account (Current Account) €0.00 €5.00 €10.00 €5.00
Date Description Money out Money in Balance
Oct 16, 2026 Apple Pay deposit by *9012 €10.00 €10.00
Oct 17, 2026 Transfer to Mila Stoeva €5.00 €5.00
";

    /// A whole import from the interface: x → the path → parsing → dropping a
    /// row → «no» returns to parsing → writing → rolling back.
    #[test]
    fn a_statement_is_reviewed_written_and_rolled_back_from_the_keyboard() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('iCard','EUR','CARD'), ('Revolut','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description)
               values (1,1,'expense','2026-10-16T00:00:00Z',1000,'EUR',-1000,'Revolut** Dublin');",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        let path = std::env::temp_dir().join(format!("potok-tui-import-{}.txt", std::process::id()));
        std::fs::write(&path, REVOLUT_OCTOBER).unwrap();

        keys(&mut app, &[KeyCode::Char(k(Screen::Counterparties)), KeyCode::Char('x')]);
        choose(&mut app, "Import a statement");
        assert_eq!(app.mode, Mode::Path);
        app.input = path.display().to_string();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Review, "{:?}", app.status);
        let review = app.review.as_ref().unwrap();
        assert_eq!(review.plan.account, "Revolut");
        assert!(review.plan.rows[0].hint.is_some(), "the top-up found its leg on iCard");

        keys(&mut app, &[KeyCode::Down, KeyCode::Char(' ')]);
        assert!(!app.review.as_ref().unwrap().plan.rows[1].write, "a transfer to a friend is not written");
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.mode, Mode::Confirm, "Esc asks before the decisions are lost");
        keys(&mut app, &[KeyCode::Char('n')]);
        assert_eq!(app.mode, Mode::Review, "«no» — back to parsing");
        assert!(!app.review.as_ref().unwrap().plan.rows[1].write, "with what was decided");
        keys(&mut app, &[KeyCode::Char('w'), KeyCode::Char('n')]);
        assert_eq!(app.mode, Mode::Review, "«no» — back to parsing");
        keys(&mut app, &[KeyCode::Char('w'), KeyCode::Char('y')]);
        assert!(app.review.is_none(), "{:?}", app.status);
        assert_eq!(roles(&app), ["transfer"], "one row written, and tied — in the journal a transfer is one row");

        // The rollback lives on «Statements» — next to what is rolled back, and
        // a key of its own leads there: the digits ran out.
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('I')]);
        assert_eq!(app.screen, Screen::Statements);
        assert_eq!(app.rows_on(Screen::Statements), 1, "one statement loaded");

        // Enter on it — what it brought; Esc returns to the list of statements.
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        assert_eq!(app.visible_operations().len(), 1, "a transfer as one row: {:?}", app.status);
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Statements, "Esc — to the statements");

        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Roll back the import");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(roles(&app), ["expense"], "{:?}", app.status);
        let _ = std::fs::remove_file(&path);
    }

    /// iCard prints no balance: writing its statement goes straight to the
    /// bank balance prompt for that account, and the answer reconciles it.
    #[test]
    fn an_icard_import_asks_for_the_bank_balance() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind) values ('Revolut','EUR','BANK'), ('iCard','EUR','CARD');").unwrap();
        let mut app = App::new(conn).unwrap();
        let path = std::env::temp_dir().join(format!("potok-tui-icard-{}.txt", std::process::id()));
        std::fs::write(
            &path,
            "© iCard AD, www.icard.com
История на трансакции
Дата и час Вальор Тип Описание Сума Валута Статус
16:20 06.10.2026 Incoming transfer FROM SOMEBODY 100.00 EUR 100.00 EUR Approved
",
        )
        .unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Counterparties)), KeyCode::Char('x')]);
        choose(&mut app, "Import a statement");
        app.input = path.display().to_string();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Review, "{:?}", app.status);
        keys(&mut app, &[KeyCode::Char('w'), KeyCode::Char('y')]);
        assert_eq!(app.mode, Mode::Reconcile, "{:?}", app.status);
        assert_eq!((app.screen, app.accounts[app.cursor()].name.as_str()), (Screen::Accounts, "iCard"));
        typed(&mut app, "100 2026-10-06");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.status.clone().unwrap_or_default().contains("matches"), "{:?}", app.status);
        let _ = std::fs::remove_file(&path);
    }

    /// Not a statement — an answer in the status line, not a panic.
    #[test]
    fn a_file_that_is_not_a_statement_is_answered_not_crashed() {
        let mut app = copies();
        let path = std::env::temp_dir().join(format!("potok-tui-junk-{}.txt", std::process::id()));
        std::fs::write(&path, "this is not a statement").unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Counterparties)), KeyCode::Char('x')]);
        choose(&mut app, "Import a statement");
        app.input = path.display().to_string();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status.clone().unwrap_or_default().starts_with("not done"), "{:?}", app.status);
        let _ = std::fs::remove_file(&path);
    }

    fn cash_only() -> App {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind,opening_minor) values ('Cash','EUR','CASH',5000);").unwrap();
        App::new(conn).unwrap()
    }

    /// Cash from the keyboard: n, the amount, to whom, Enter. An error stays in
    /// the form, what was typed is not lost.
    #[test]
    fn a_cash_purchase_is_typed_in_and_then_fixed() {
        let mut app = cash_only();
        keys(&mut app, &[KeyCode::Char('n')]);
        assert_eq!(app.mode, Mode::Form);
        typed(&mut app, "12.5");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Form, "with no «who» it is not written");
        assert!(app.form.as_ref().unwrap().error.is_some());

        keys(&mut app, &[KeyCode::Tab, KeyCode::Tab]);
        typed(&mut app, "A bakery");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal, "{:?}", app.form);
        assert_eq!(app.accounts[0].balance.minor(), 5000 - 1250);

        // correct it: 12.50 was a mistake, it is really 1.25
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('x')]);
        choose(&mut app, "Fix");
        assert_eq!(app.form.as_ref().unwrap().value("amount"), "12.50");
        for _ in 0..5 {
            keys(&mut app, &[KeyCode::Backspace]);
        }
        typed(&mut app, "1.25");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.accounts[0].balance.minor(), 5000 - 125, "{:?}", app.status);
        assert_eq!(app.visible_operations().iter().filter(|o| !o.removed).count(), 1);
    }

    /// Income — by an arrow on the «what» field; the account — by an arrow on «account».
    #[test]
    fn income_and_the_account_are_chosen_with_arrows() {
        let mut app = copies();
        keys(&mut app, &[KeyCode::Char('n')]);
        keys(&mut app, &[KeyCode::Up, KeyCode::Right, KeyCode::Up, KeyCode::Right]);
        let form = app.form.as_ref().unwrap();
        assert_eq!(form.chosen("kind"), 1, "income");
        let account = form.choice("account").to_string();
        keys(&mut app, &[KeyCode::Down, KeyCode::Down]);
        typed(&mut app, "100");
        keys(&mut app, &[KeyCode::Tab, KeyCode::Tab]);
        typed(&mut app, "Salary");
        keys(&mut app, &[KeyCode::Enter]);
        let line = &app.operations[0];
        assert_eq!((line.account.as_str(), line.amount.minor()), (account.as_str(), 10000));
    }

    #[test]
    fn the_opening_balance_is_set_from_the_accounts_screen() {
        let mut app = cash_only();
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Opening balance");
        assert_eq!(app.input, "50.00");
        app.input = "80".into();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.accounts[0].balance.minor(), 8000, "{:?}", app.status);
    }

    /// A friend sent 300: on the operation — «tie to a debt» → «a new debt with
    /// this operation» → the form is already filled → Enter. Returned 100 — tied
    /// to the same debt. The remainder written off from the debt's menu.
    #[test]
    fn a_loan_from_a_friend_is_opened_paid_and_written_off_from_the_keyboard() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'income', '2026-10-01T00:00:00Z',30000,'EUR', 30000,'Mila Stoeva'),
               (2,1,'expense','2026-10-10T00:00:00Z',10000,'EUR',-10000,'Mila Stoeva');",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        // the journal: the new on top — 2 (returned), then 1 (she sent)
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Down, KeyCode::Char('x')]);
        choose(&mut app, "Link to a debt");
        choose(&mut app, "New debt from this operation");
        let form = app.form.as_ref().unwrap();
        assert_eq!((form.value("who"), form.choice("direction")), ("Mila Stoeva", "I owe"));
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal, "{:?}", app.form);
        assert_eq!(app.debts[0].remaining.minor(), 30000);

        keys(&mut app, &[KeyCode::Up, KeyCode::Char('x')]);
        choose(&mut app, "Link to a debt");
        choose(&mut app, "Mila Stoeva");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(app.debts[0].remaining.minor(), 20000, "{:?}", app.status);

        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Char('x')]);
        choose(&mut app, "Write off what is left");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(app.debts[0].remaining.minor(), 0, "{:?}", app.status);
        assert!(potok_core::verify::run(&app.conn).unwrap().ok());
    }

    /// An old debt with no operation: n on the debts, who, how much, Enter.
    #[test]
    fn an_old_debt_is_typed_in_on_the_debts_screen() {
        let mut app = cash_only();
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Char('n')]);
        typed(&mut app, "Ivan");
        keys(&mut app, &[KeyCode::Tab, KeyCode::Right, KeyCode::Tab]);
        typed(&mut app, "50");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal, "{:?}", app.form);
        assert_eq!((app.debts[0].direction.as_str(), app.debts[0].remaining.minor()), ("OWED_TO_ME", 5000));
    }

    /// Removed by mistake — x, y on the edits screen — and the operation is back.
    #[test]
    fn a_mistake_is_undone_from_the_journal_screen() {
        let mut app = apple_pay();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('x')]);
        choose(&mut app, "Remove");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert!(app.visible_operations().iter().all(|o| o.removed));

        keys(&mut app, &[KeyCode::Char('u')]);
        assert!(app.changes[0].what.starts_with("removed"), "{:?}", app.changes);
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Undo");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert!(app.changes[1].undone_at.is_some(), "{:?}", app.changes);
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations))]);
        assert!(app.visible_operations().iter().all(|o| !o.removed), "{:?}", app.status);
    }

    /// A flat's deposit is not a spend: «do not count», and it left the flows;
    /// then changed one's mind — «count it again».
    #[test]
    fn an_operation_is_taken_out_of_flows_and_back_from_the_keyboard() {
        let mut app = cash_only();
        keys(&mut app, &[KeyCode::Char('n')]);
        typed(&mut app, "800");
        keys(&mut app, &[KeyCode::Tab, KeyCode::Tab]);
        typed(&mut app, "Deposit");
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char(k(Screen::Operations)), KeyCode::Char('x')]);
        choose(&mut app, "Do not count");
        assert_eq!(roles(&app), ["excluded"], "{:?}", app.status);
        assert_eq!(app.accounts[0].balance.minor(), 5000 - 80000, "the balance is the same");
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Count again");
        assert_eq!(roles(&app), ["expense"]);
    }

    fn counterparty_on_top(app: &App) -> String {
        app.visible_operations()[0].counterparty.clone().unwrap_or_default()
    }

    /// From an operation: rename its counterparty, then say it is another one
    /// altogether; from the import screen — fold two into one.
    #[test]
    fn counterparties_are_renamed_respelled_and_merged_from_the_keyboard() {
        let mut app = copies();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations))]);
        assert_eq!(counterparty_on_top(&app), "Fee");

        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Rename «Fee»");
        assert_eq!((app.mode.clone(), app.input.as_str()), (Mode::Input, "Fee"));
        app.input.clear();
        typed(&mut app, "A fee");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(counterparty_on_top(&app), "A fee", "{:?}", app.status);

        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "This is another counterparty");
        typed(&mut app, "Bank fee");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(counterparty_on_top(&app), "Bank fee", "{:?}", app.status);
        assert_eq!(app.visible_operations()[1].counterparty.as_deref(), Some("Bank fee"), "the same record — too");

        keys(&mut app, &[KeyCode::Char(k(Screen::Counterparties))]);
        while app.selected_counterparty().map(|c| c.name.clone()) != Some("Revolut".into()) {
            keys(&mut app, &[KeyCode::Down]);
        }
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Merge into another");
        typed(&mut app, "bank");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(matches!(app.mode, Mode::Menu { .. }), "{:?}", app.status);
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char('y')]);
        assert!(app.counterparties.iter().all(|c| c.name != "Revolut"), "{:?}", app.status);
    }

    /// The cursor on a settings row and Enter.
    fn on_setting(app: &mut App, key: app::SettingKey) {
        keys(app, &[KeyCode::Char(',')]);
        app.cursors_mut()[Screen::Settings as usize] = app::SettingKey::ALL.iter().position(|k| *k == key).unwrap();
        keys(app, &[KeyCode::Enter]);
    }

    /// Settings from the keyboard: the language switches at once and is stored
    /// in the ledger, the default account is picked up by `n`, the statements
    /// folder and the flows period are remembered.
    #[test]
    fn settings_are_changed_from_the_keyboard_and_kept_in_the_ledger() {
        use potok_core::i18n::{lang, Lang};
        let mut app = copies();
        keys(&mut app, &[KeyCode::Char(','), KeyCode::Enter]);
        assert_eq!(lang(), Lang::Ru);
        assert_eq!(Screen::Settings.title(), "Настройки");
        assert_eq!(potok_core::settings::language(&app.conn).unwrap(), Lang::Ru);
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(lang(), Lang::En);

        on_setting(&mut app, app::SettingKey::StatementsDir);
        assert_eq!((app.mode.clone(), app.input.as_str()), (Mode::Input, "~/Downloads"));
        app.input = "/tmp/statements".into();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.statements_folder(), "/tmp/statements");

        on_setting(&mut app, app::SettingKey::DefaultAccount);
        let chosen = app.default_account.expect("an account after Auto");
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('n')]);
        let form = app.form.as_ref().unwrap();
        assert_eq!(app.accounts[form.chosen("account")].id, chosen);
        keys(&mut app, &[KeyCode::Esc, KeyCode::Char(',')]);
        on_setting(&mut app, app::SettingKey::FlowsPeriod);
        assert_eq!(app.period, app::Period::PreviousMonth);
        on_setting(&mut app, app::SettingKey::Theme);
        assert_eq!(theme::current(), theme::Theme::Midnight, "Enter walks the themes in order");

        // open the same ledger again — everything in place
        let again = App::new(std::mem::replace(&mut app.conn, potok_core::db::open_memory().unwrap())).unwrap();
        assert_eq!((again.period, again.statements_folder().as_str()), (app::Period::PreviousMonth, "/tmp/statements"));
        assert_eq!(again.default_account, Some(chosen));
        assert_eq!(theme::current(), theme::Theme::Midnight, "the theme comes back with the ledger");
        potok_core::i18n::set_lang(Lang::En);
        theme::set(theme::Theme::Terminal);
    }

    /// An account from the interface: create by form, rename, archive and back.
    #[test]
    fn an_account_is_created_renamed_archived_and_brought_back_from_the_keyboard() {
        let mut app = cash_only();
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "New account");
        typed(&mut app, "Wise");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.accounts.iter().any(|a| a.name == "Wise"), "{:?} {:?}", app.status, app.form);
        while app.accounts[app.cursor()].name != "Wise" {
            keys(&mut app, &[KeyCode::Down]);
        }
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Rename");
        app.input = "Wise EUR".into();
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.accounts.iter().any(|a| a.name == "Wise EUR"), "{:?}", app.status);
        while app.accounts[app.cursor()].name != "Wise EUR" {
            keys(&mut app, &[KeyCode::Down]);
        }
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Archive");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert!(app.accounts.iter().all(|a| a.name != "Wise EUR"), "{:?}", app.status);
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Archived accounts");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.accounts.iter().any(|a| a.name == "Wise EUR"), "{:?}", app.status);
    }

    /// A transfer by hand, a debt edited, a pending statement row in an
    /// account's details — from the keyboard.
    #[test]
    fn a_transfer_is_typed_a_debt_edited_and_pending_rows_shown() {
        let mut app = copies();
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "New transfer");
        typed(&mut app, "25");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal, "{:?}", app.form);
        let legs = potok_core::query::operations(&app.conn, None, false).unwrap();
        assert_eq!(legs.iter().filter(|o| o.amount.minor().abs() == 2500 && o.role == "transfer").count(), 2);
        assert_eq!(app.operations.iter().filter(|o| o.amount.minor().abs() == 2500).count(), 1, "one line in the journal");

        let mut app = northwind();
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Char('x')]);
        choose(&mut app, "Edit");
        typed(&mut app, "31.12.2026");
        keys(&mut app, &[KeyCode::Enter]);
        let id = app.debts[0].id;
        assert_eq!(potok_core::debt::fields(&app.conn, id).unwrap().due_on.as_deref(), Some("2026-12-31"), "{:?}", app.form);

        // a statement with one pending row
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind) values ('Revolut','EUR','BANK');").unwrap();
        let mut app = App::new(conn).unwrap();
        let path = std::env::temp_dir().join(format!("potok-tui-pending-{}.txt", std::process::id()));
        std::fs::write(&path, format!("{REVOLUT_OCTOBER} Start date Description Money out Money in\nOct 18, 2026 Hosting €6.60\n")).unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Counterparties)), KeyCode::Char('x')]);
        choose(&mut app, "Import a statement");
        app.input = path.display().to_string();
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char('w'), KeyCode::Char('y'), KeyCode::Char(k(Screen::Accounts))]);
        let app::Detail::Account(d) = &app.detail else { panic!("{:?}", app.detail) };
        assert!(d.pending.iter().any(|raw| raw.contains("Hosting")), "{:?} {:?}", d.pending, app.status);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_operation_is_split_from_the_keyboard() {
        let mut app = cash_only();
        keys(&mut app, &[KeyCode::Char('n')]);
        typed(&mut app, "12.5");
        keys(&mut app, &[KeyCode::Tab, KeyCode::Tab]);
        typed(&mut app, "Kaufland");
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char(k(Screen::Operations)), KeyCode::Char('x')]);
        choose(&mut app, "Split by category");
        typed(&mut app, "Food 10; Household");
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char(k(Screen::Operations))]);
        let app::Detail::Operation(d) = &app.detail else { panic!("{:?}", app.detail) };
        assert_eq!(d.split.iter().map(|(c, a)| (c.as_str(), a.minor())).collect::<Vec<_>>(), [("Food", 1000), ("Household", 250)], "{:?}", app.status);
    }

    /// Categories from the keyboard: rename, a budget, fold, delete.
    #[test]
    fn categories_are_managed_from_their_screen() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'Cash','EUR','CASH');
             insert into category (id,name) values (1,'Food'), (2,'Groceries');
             insert into counterparty (id,name,category_id) values (1,'Lidl',1), (2,'Kaufland',2);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id) values
               (1,'expense',date('now'),3000,'EUR',-3000,1), (1,'expense',date('now'),1000,'EUR',-1000,2);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        let goto = |app: &mut App, name: &str| {
            let at = app.category_lines.iter().position(|c| c.name == name).unwrap();
            app.screen = Screen::Categories;
            app.cursors_mut()[Screen::Categories as usize] = at;
            app.refresh_detail().unwrap();
        };
        goto(&mut app, "Groceries");
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Rename");
        app.input = "Supermarket".into();
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.category_lines.iter().any(|c| c.name == "Supermarket"), "{:?}", app.status);

        goto(&mut app, "Food");
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Monthly budget");
        typed(&mut app, "200");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(potok_core::budget::get(&app.conn, "Food").unwrap().map(|m| m.minor()), Some(20000), "{:?}", app.status);

        goto(&mut app, "Supermarket");
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Merge into another");
        typed(&mut app, "foo");
        keys(&mut app, &[KeyCode::Enter, KeyCode::Enter, KeyCode::Char('y')]);
        assert_eq!(app.category_lines.iter().map(|c| (c.name.as_str(), c.operations)).collect::<Vec<_>>(), [("Food", 2)], "{:?}", app.status);

        goto(&mut app, "Food");
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Delete the category");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert!(app.category_lines.is_empty(), "{:?}", app.status);
    }

    /// The journal: mark two operations with space and remove both; fold by
    /// counterparty and drill into one; another order; a month by `d`.
    #[test]
    fn the_journal_is_marked_folded_ordered_and_cut_to_dates() {
        let mut app = copies();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('f')]);
        assert!(app.fold);
        let groups = app.groups();
        assert!(groups.iter().any(|g| g.name == "Revolut" && g.count == 2), "{groups:?}");
        let at = groups.iter().position(|g| g.name == "Revolut").unwrap();
        app.cursors_mut()[Screen::Operations as usize] = at;
        keys(&mut app, &[KeyCode::Enter]);
        assert!(matches!(&app.ops_view, app::OpsView::Counterparty { name } if name == "Revolut"));
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Operations, "back to the journal it came from");

        keys(&mut app, &[KeyCode::Char('o')]);
        assert_eq!(app.visible_operations()[0].amount.minor().abs(), 10000, "largest first");
        keys(&mut app, &[KeyCode::Char('o'), KeyCode::Char('o')]);

        keys(&mut app, &[KeyCode::Char('g'), KeyCode::Char(' '), KeyCode::Char(' ')]);
        assert_eq!(app.marked.len(), 2);
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "The 2 marked: do not count");
        assert!(app.marked.is_empty(), "{:?}", app.status);

        keys(&mut app, &[KeyCode::Char('d')]);
        app.input = "2026-09-03 2026-09-03".into();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.visible_operations().len(), 2, "{:?}", app.status);
    }

    /// The lock: a passphrase in the settings, L locks, another's passphrase
    /// does not open it, one's own does; what is typed shows nowhere.
    #[test]
    fn the_screen_locks_and_opens_only_on_its_passphrase() {
        let mut app = cash_only();
        on_setting(&mut app, app::SettingKey::ScreenLock);
        assert_eq!(app.asking, Some(app::Ask::LockSet));
        typed(&mut app, "secret words");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.lock_hash().is_some(), "{:?}", app.status);
        assert!(!app.lock_hash().unwrap().contains("secret"));

        keys(&mut app, &[KeyCode::Char('L')]);
        assert_eq!(app.mode, Mode::Locked);
        typed(&mut app, "wrong");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Locked);
        assert!(app.lock_wait() > 0, "a wrong passphrase makes the next try wait");
        typed(&mut app, "secret words");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Locked, "typing during the wait is ignored");
        assert!(app.input.is_empty());
        app.lock_wait_until = None; // the wait has passed
        typed(&mut app, "secret words");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.lock_failures, 0);
        assert!(app.input.is_empty(), "the passphrase is not kept in the input");
    }

    /// A month from the 25th and from the last working day: the flows of «this
    /// month» start on payday in the previous calendar month.
    #[test]
    fn a_salary_day_moves_where_a_month_starts() {
        use potok_core::period::{Kind, MonthRule};
        let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let today = day("2026-10-19");
        let on25 = MonthRule { kind: Kind::Day(25), overrides: Default::default() };
        assert_eq!(app::Period::ThisMonth.bounds_on(today, &on25), ("2026-09-25".into(), "2026-10-19".into()));
        assert_eq!(app::Period::PreviousMonth.bounds_on(today, &on25), ("2026-08-25".into(), "2026-09-24".into()));
        assert_eq!(app::Period::ThisMonth.previous_bounds_on(today, &on25), Some(("2026-08-25".into(), "2026-09-18".into())));
        let payday = MonthRule { kind: Kind::LastWorkday, overrides: Default::default() };
        assert_eq!(app::Period::ThisMonth.bounds_on(today, &payday), ("2026-09-30".into(), "2026-10-19".into()));
        assert_eq!(app::Period::ThisMonth.bounds_on(today, &MonthRule::calendar()), app::Period::ThisMonth.bounds(today));
    }

    /// The last working day — from the settings; a month's correction — from the
    /// analytics; an empty correction gives the rule back.
    #[test]
    fn the_month_rule_is_chosen_and_a_month_corrected_from_the_keyboard() {
        let mut app = cash_only();
        for _ in 0..6 {
            on_setting(&mut app, app::SettingKey::MonthDay);
        }
        assert_eq!(app.month_rule.kind, potok_core::period::Kind::LastWorkday, "1 → 5 … 25 → last working day");
        let key = app.month_rule.cycle(app.today, 0).key;
        let rule_start = app.month_rule.by_rule(key);

        keys(&mut app, &[KeyCode::Char(k(Screen::Analytics)), KeyCode::Char('x')]);
        choose(&mut app, "Correct when this month starts");
        assert_eq!(app.input, rule_start.to_string());
        app.input = (rule_start - chrono::Duration::days(1)).to_string();
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.month_rule.overrides.get(&key), Some(&(rule_start - chrono::Duration::days(1))), "{:?}", app.status);
        assert!(app.months.last().map(|m| m.start) <= Some(rule_start));

        on_setting(&mut app, app::SettingKey::MonthStart);
        app.input.clear();
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.month_rule.overrides.is_empty(), "{:?}", app.status);
    }

    fn roles(app: &App) -> Vec<String> {
        app.visible_operations().iter().map(|op| op.role.clone()).collect()
    }

    /// A transfer is split and tied back together — from the keyboard, no SQL.
    #[test]
    fn a_transfer_is_unpaired_and_paired_back_from_the_keyboard() {
        let mut app = apple_pay();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('x')]);
        choose(&mut app, "Not a transfer");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(roles(&app), ["income", "expense"]);

        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "A transfer between own accounts");
        assert!(matches!(app.mode, app::Mode::Menu { .. }), "{:?}", app.status);
        assert_eq!(app.menu.len(), 1, "one candidate — the leg on iCard");
        keys(&mut app, &[KeyCode::Enter, KeyCode::Char('y')]);
        assert_eq!(roles(&app), ["transfer"], "paired again: one line for both legs");
        assert!(potok_core::verify::run(&app.conn).unwrap().ok());
    }

    fn remaining(app: &App) -> i64 {
        app.debts[0].remaining.minor()
    }

    fn choose(app: &mut App, label_start: &str) {
        let at = app.menu.iter().position(|i| i.label.starts_with(label_start))
            .unwrap_or_else(|| panic!("no «{label_start}» in the menu: {:?}", app.menu));
        app.mode = Mode::Menu { selected: at };
        handle_key(app, press(KeyCode::Enter)).unwrap();
    }

    /// «This is not a debt payment»: debts → into the details → onto the
    /// utilities → x → untie → y. Keys only, as a person would do it.
    #[test]
    fn a_wrong_repayment_is_unlinked_from_the_keyboard() {
        let mut app = northwind();
        let before = remaining(&app);
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Right, KeyCode::Down]);
        assert_eq!(app.focus, app::Focus::Detail);
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Not a debt payment");
        assert_eq!(app.mode, Mode::Confirm, "what changes a debt asks first");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(remaining(&app), before + 2393);
    }

    /// A duplicate is removed and brought back; the debt follows it.
    #[test]
    fn a_duplicate_is_removed_and_restored_from_the_keyboard() {
        let mut app = northwind();
        let before = remaining(&app);
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations))]);
        let at = app.visible_operations().iter().position(|o| o.id == 2).unwrap();
        app.cursors_mut()[Screen::Operations as usize] = at;
        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Remove");
        keys(&mut app, &[KeyCode::Char('y')]);
        assert_eq!(remaining(&app), before + 72000, "the duplicate no longer pays the debt down");
        assert!(app.visible_operations().iter().any(|o| o.id == 2 && o.removed), "but it is visible");

        keys(&mut app, &[KeyCode::Char('x')]);
        choose(&mut app, "Restore");
        assert_eq!(remaining(&app), before, "back — and paying it down again");
    }

    /// Any key but «y» means no. What moves money must not happen because a
    /// finger missed.
    #[test]
    fn anything_but_yes_cancels() {
        let mut app = northwind();
        let before = remaining(&app);
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Right, KeyCode::Down, KeyCode::Char('x')]);
        choose(&mut app, "Not a debt payment");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(remaining(&app), before, "nothing changed");
    }

    /// On a Russian layout «y» is «н».
    #[test]
    fn yes_works_on_a_russian_layout_too() {
        let mut app = northwind();
        let before = remaining(&app);
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Right, KeyCode::Down, KeyCode::Char('x')]);
        choose(&mut app, "Not a debt payment");
        keys(&mut app, &[KeyCode::Char('н')]);
        assert_eq!(remaining(&app), before + 2393);
    }

    /// Enter on a movement in the details opens its operation; Esc closes it.
    #[test]
    fn enter_on_a_movement_opens_its_operation() {
        let mut app = northwind();
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Right, KeyCode::Down, KeyCode::Enter]);
        assert!(app.pinned && app.zoom);
        let app::Detail::Operation(op) = &app.detail else { panic!("{:?}", app.detail) };
        assert_eq!(op.line.description, "Epay To: Epay Utilities");
        keys(&mut app, &[KeyCode::Esc]);
        assert!(!app.pinned);
        assert!(matches!(app.detail, app::Detail::Debt(_)), "back to the debt");
    }

    #[test]
    fn a_key_becomes_the_name_the_bindings_use() {
        assert_eq!(key_name(press(KeyCode::Char('q'))).as_deref(), Some("q"));
        assert_eq!(key_name(press(KeyCode::Up)).as_deref(), Some("up"));
        assert_eq!(key_name(press(KeyCode::BackTab)).as_deref(), Some("shift+tab"));
        assert_eq!(
            key_name(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)).as_deref(),
            Some("shift+tab")
        );
        assert_eq!(key_name(press(KeyCode::F(5))), None);
    }

    #[test]
    fn digits_and_brackets_walk_the_screens() {
        let mut app = app();
        handle_key(&mut app, press(KeyCode::Char(k(Screen::Flows)))).unwrap();
        assert_eq!(app.screen, Screen::Flows);
        handle_key(&mut app, press(KeyCode::Char(']'))).unwrap();
        assert_eq!(app.screen, Screen::Analytics);
        handle_key(&mut app, press(KeyCode::Char('['))).unwrap();
        assert_eq!(app.screen, Screen::Flows);
    }

    /// The screens go round rather than stopping at the edge.
    #[test]
    fn the_screens_wrap_around() {
        let mut app = app();
        app.go_to(Screen::ALL[0]);
        handle_key(&mut app, press(KeyCode::Char('['))).unwrap();
        assert_eq!(app.screen, Screen::Settings, "back from the first — to the last");
        handle_key(&mut app, press(KeyCode::Char(']'))).unwrap();
        assert_eq!(app.screen, Screen::ALL[0]);
    }

    /// Tab — between a screen's panes, there and back.
    #[test]
    fn tab_switches_between_the_list_and_the_details() {
        let mut app = northwind();
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts))]);
        keys(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.focus, app::Focus::Detail);
        keys(&mut app, &[KeyCode::BackTab]);
        assert_eq!(app.focus, app::Focus::List);
    }

    /// An opened window is modal: the arrows walk inside it, the list under it
    /// stands still, Tab does not leave it, Esc closes it.
    #[test]
    fn an_open_window_keeps_the_keys_to_itself() {
        let mut app = northwind();
        keys(&mut app, &[KeyCode::Char(k(Screen::Debts)), KeyCode::Enter]);
        assert!(app.zoom);
        let list_at = app.cursor();
        keys(&mut app, &[KeyCode::Down, KeyCode::Tab, KeyCode::Left]);
        assert_eq!(app.cursor(), list_at, "the list under the window does not move");
        assert_eq!(app.detail_cursor, 1, "the arrow went into the window");
        assert!(app.zoom);
        keys(&mut app, &[KeyCode::Esc]);
        assert!(!app.zoom);
    }

    /// Why typing takes the whole keyboard at all: a «q» in a category's name
    /// or in the search line must not close the program.
    #[test]
    fn typing_does_not_quit() {
        for mode in [Mode::PickCategory { selected: 0 }, Mode::Search] {
            let mut app = app();
            app.screen = Screen::Counterparties;
            app.mode = mode.clone();
            for c in "Groceries and quit".chars() {
                handle_key(&mut app, press(KeyCode::Char(c))).unwrap();
            }
            assert!(!app.quit, "a «q» inside typing closed the program ({mode:?})");
            assert_eq!(app.input, "Groceries and quit");
            handle_key(&mut app, press(KeyCode::Esc)).unwrap();
            assert_eq!(app.mode, Mode::Normal);
            assert!(app.input.is_empty(), "cancelling must clear what was typed");
        }
    }

    /// A slash from any screen opens the search, and the letters go into it.
    #[test]
    fn slash_opens_a_search_that_takes_the_letters() {
        let mut app = app();
        app.screen = Screen::Debts;
        handle_key(&mut app, press(KeyCode::Char('/'))).unwrap();
        assert_eq!(app.mode, Mode::Search);
        assert_eq!(app.screen, Screen::Operations);
        handle_key(&mut app, press(KeyCode::Char('g'))).unwrap();
        assert_eq!(app.input, "g", "«g» is a search letter, not «to the top»");
    }

    /// `q` once asks, a second `q` quits — on every screen; any other key in
    /// between keeps the program open.
    #[test]
    fn q_twice_quits_everywhere_and_once_only_asks() {
        for screen in Screen::ALL {
            let mut app = app();
            app.screen = screen;
            handle_key(&mut app, press(KeyCode::Char('q'))).unwrap();
            assert!(!app.quit, "one «q» closed the program on {screen:?}");
            handle_key(&mut app, press(KeyCode::Char('q'))).unwrap();
            assert!(app.quit, "«q q» did not close the program on {screen:?}");
        }
        let mut app = app();
        keys(&mut app, &[KeyCode::Char('q'), KeyCode::Down, KeyCode::Char('q')]);
        assert!(!app.quit, "a key between the two «q» cancels");
    }

    /// The help closes on any key and does not carry out its action as well.
    #[test]
    fn help_closes_without_doing_what_the_key_would_do() {
        let mut app = app();
        handle_key(&mut app, press(KeyCode::Char('?'))).unwrap();
        assert_eq!(app.mode, Mode::Help);
        handle_key(&mut app, press(KeyCode::Char('q'))).unwrap();
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.quit, "the key that closed the help must not also quit");
    }

    /// A key this screen does not have must do nothing.
    #[test]
    fn a_key_that_belongs_to_another_screen_does_nothing_here() {
        let mut app = app();
        app.screen = Screen::Accounts;
        handle_key(&mut app, press(KeyCode::Char('c'))).unwrap();
        assert_eq!(app.mode, Mode::Normal, "marking works on the import screen only");
    }

    /// `C` walks the uncategorised: Enter takes the hint and moves on, Tab
    /// leaves one for later, and the queue ends by itself.
    #[test]
    fn the_queue_categorises_the_uncategorised_one_after_another() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('iCard','EUR','CARD');
             insert into category (id,name) values (1,'Food'), (2,'Transport');
             insert into counterparty (id,name,category_id) values (1,'Lidl',null), (2,'Taxi',null), (3,'Billa',1);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,'expense','2026-10-01T00:00:00Z',5000,'EUR',-5000,'Lidl',1,1),
               (1,'expense','2026-10-02T00:00:00Z',900,'EUR',-900,'Taxi',2,null),
               (1,'expense','2026-10-03T00:00:00Z',100,'EUR',-100,'Billa',3,null);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.screen = Screen::Counterparties;
        handle_key(&mut app, press(KeyCode::Char('C'))).unwrap();
        assert!(app.queue);
        assert_eq!(app.visible_counterparties().len(), 2, "Billa has a category already");
        assert_eq!(app.selected_counterparty().unwrap().name, "Lidl", "the most money first");
        assert!(matches!(app.mode, Mode::PickCategory { .. }));

        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.selected_counterparty().unwrap().name, "Taxi", "Lidl took its hint and left");
        assert!(matches!(app.mode, Mode::PickCategory { .. }), "the next one asks at once");

        keys(&mut app, &[KeyCode::Tab]);
        assert!(!app.queue, "skipping the last one ends the queue");
        assert_eq!(app.mode, Mode::Normal);
        let food: Option<String> = app.counterparties.iter().find(|l| l.name == "Lidl").unwrap().category.clone();
        assert_eq!(food.as_deref(), Some("Food"));
        assert!(app.counterparties.iter().find(|l| l.name == "Taxi").unwrap().category.is_none());
    }

    /// An empty list: the cursor goes nowhere and drops nothing.
    #[test]
    fn moving_the_cursor_in_an_empty_list_is_harmless() {
        let mut app = app();
        for code in [KeyCode::Down, KeyCode::Up, KeyCode::PageDown, KeyCode::Char('G')] {
            handle_key(&mut app, press(code)).unwrap();
        }
        assert_eq!(app.cursor(), 0);
    }

    /// Another device writes: nothing here offers a change, and a key that
    /// would change something says who writes instead. Before, Enter on the
    /// theme closed the program with SQLite's «attempt to write a readonly
    /// database», and the menu offered every edit only to refuse it.
    #[test]
    fn read_only_offers_no_edits_and_does_not_crash() {
        let path = std::env::temp_dir().join(format!("potok-readonly-ui-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        {
            let conn = potok_core::db::open(&path).unwrap();
            conn.execute_batch(
                "insert into account (name,currency,kind,opening_minor) values ('Cash','EUR','CASH',1000);
                 insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description)
                     values (1,'expense','2026-10-10T00:00:00Z',500,'EUR',-500,'Bread');",
            )
            .unwrap();
        }
        let mut app = App::new(potok_core::db::open_read_only(&path).unwrap()).unwrap();
        app.read_only = Some(crate::cloud::Mark { device: "Other Mac".into(), since: "2026-10-20T08:00:00Z".into() });
        app.go_to(Screen::Operations);
        app.refresh_detail().unwrap();

        keys(&mut app, &[KeyCode::Char('x')]);
        assert!(app.menu.iter().all(|m| !m.action.writes()), "{:?}", app.menu);
        assert!(app.menu.iter().any(|m| m.action == app::MenuAction::OpenOperation(1)), "reading stays: {:?}", app.menu);
        keys(&mut app, &[KeyCode::Esc]);

        on_setting(&mut app, app::SettingKey::Theme);
        assert!(app.status.as_deref().unwrap_or("").contains("Other Mac"), "{:?}", app.status);

        app.status = None;
        keys(&mut app, &[KeyCode::Char(k(Screen::Accounts)), KeyCode::Char('n')]);
        assert_ne!(app.mode, app::Mode::Form, "no form to fill that cannot be saved");
        assert!(app.status.as_deref().unwrap_or("").contains("W"), "{:?}", app.status);
        drop(app);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn analytics_tabs_walk_months_detail_and_regular_payments() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'iCard','EUR','CARD');
             insert into category (id,name) values (1,'Housing'), (2,'Food');
             insert into counterparty (id,name,category_id) values (1,'Landlord',1), (2,'Lidl',2);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id) values
               (1,'expense','2026-06-03T00:00:00Z',160000,'EUR',-160000,1),
               (1,'expense','2026-07-03T00:00:00Z',160000,'EUR',-160000,1),
               (1,'expense','2026-08-03T00:00:00Z',160000,'EUR',-160000,1),
               (1,'expense','2026-09-03T00:00:00Z',160000,'EUR',-160000,1),
               (1,'expense','2026-10-03T00:00:00Z',160000,'EUR',-160000,1),
               (1,'expense','2026-10-10T00:00:00Z',4500,'EUR',-4500,2);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.today = chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
        app.reload().unwrap();
        keys(&mut app, &[KeyCode::Char(k(Screen::Analytics))]);
        assert_eq!(app.screen, Screen::Analytics);
        assert_eq!((app.focus, app.analytics_pane), (app::Focus::List, app::AnalyticsPane::Months));

        // Tab #1: Month details (categories in the selected month)
        keys(&mut app, &[KeyCode::Tab]);
        assert_eq!((app.focus, app.analytics_pane), (app::Focus::Detail, app::AnalyticsPane::Months));
        // Move down to second category (Food) and press Enter to drill into October Food operations
        keys(&mut app, &[KeyCode::Down, KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        assert_eq!(app.visible_operations().len(), 1);
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Analytics);

        // Tab -> Month Detail -> Tab -> Regular payments list
        keys(&mut app, &[KeyCode::Tab, KeyCode::Tab]);
        assert_eq!((app.focus, app.analytics_pane), (app::Focus::List, app::AnalyticsPane::Regular));
        assert!(matches!(app.detail, app::Detail::Counterparty(_)));

        // Enter on Regular payment opens Landlord's operations
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        assert_eq!(app.visible_operations().len(), 5);
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Analytics);

        // Shift+Tab (BackTab) walks back to Month Detail
        keys(&mut app, &[KeyCode::BackTab]);
        assert_eq!((app.focus, app.analytics_pane), (app::Focus::Detail, app::AnalyticsPane::Months));
    }

    /// Esc returns where one came from — both from the «all operations» menu
    /// and from the counterparties, even if the way in was the Overview. A
    /// folded journal offers actions on the counterparty, not an export alone.
    #[test]
    fn esc_returns_where_it_came_from_and_the_folded_journal_acts_on_the_counterparty() {
        let mut app = copies();
        // An old return_to from the Overview must not drag back there.
        app.return_to = Some(Screen::Home);
        keys(&mut app, &[KeyCode::Char(k(Screen::Counterparties)), KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Counterparties, "back to counterparties, not to a stale overview");

        keys(&mut app, &[KeyCode::Char(k(Screen::Operations)), KeyCode::Char('f')]);
        assert!(app.fold);
        let at = app.groups().iter().position(|g| g.name == "Revolut").unwrap();
        app.cursors_mut()[Screen::Operations as usize] = at;
        keys(&mut app, &[KeyCode::Char('x')]);
        for wanted in ["All its operations", "Categorise", "Rename", "A note of your own", "Add a spelling", "Export these"] {
            assert!(app.menu.iter().any(|m| m.label.starts_with(wanted)), "no «{wanted}» in {:?}", app.menu);
        }
        choose(&mut app, "All its operations");
        assert!(matches!(&app.ops_view, app::OpsView::Counterparty { name } if name == "Revolut"));
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Operations, "the menu remembers where it was opened");
    }

    #[test]
    fn cycle_account_preserves_transfer_filter_dedup_and_order() {
        let mut app = apple_pay();
        keys(&mut app, &[KeyCode::Char(k(Screen::Operations))]);
        assert_eq!(app.visible_operations().len(), 1, "both legs of the transfer are one row in All");
        // Cycle through Revolut -> iCard -> All
        keys(&mut app, &[KeyCode::Char('a')]);
        assert!(app.account_filter.is_some());
        keys(&mut app, &[KeyCode::Char('a')]);
        assert!(app.account_filter.is_some());
        keys(&mut app, &[KeyCode::Char('a')]);
        assert!(app.account_filter.is_none());
        assert_eq!(app.visible_operations().len(), 1, "returning to All still deduplicates paired transfers");

        // Turn on hide_transfers (`t`) and cycle accounts
        keys(&mut app, &[KeyCode::Char('t')]);
        assert!(app.hide_transfers);
        assert_eq!(app.visible_operations().len(), 0);
        keys(&mut app, &[KeyCode::Char('a')]);
        assert_eq!(app.visible_operations().len(), 0, "hide_transfers stays active after cycling account");
    }

    #[test]
    fn lock_clears_open_modals_and_lock_off_applies_backoff_on_wrong_passphrase() {
        let mut app = cash_only();
        on_setting(&mut app, app::SettingKey::ScreenLock);
        typed(&mut app, "secret words");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.lock_hash().is_some());

        // Open a form, then lock with L — the form must be cleared
        keys(&mut app, &[KeyCode::Char(k(Screen::Accounts)), KeyCode::Char('n')]);
        assert_eq!(app.mode, Mode::Form);
        assert!(app.form.is_some());
        app.lock();
        assert_eq!(app.mode, Mode::Locked);
        assert!(app.form.is_none(), "locking clears any open form");

        // Unlock
        typed(&mut app, "secret words");
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.mode, Mode::Normal);

        // Try turning off the lock with the wrong passphrase -> backoff applies
        on_setting(&mut app, app::SettingKey::ScreenLock);
        assert_eq!(app.asking, Some(app::Ask::LockOff));
        typed(&mut app, "wrong");
        keys(&mut app, &[KeyCode::Enter]);
        assert!(app.lock_hash().is_some(), "lock stays enabled on wrong passphrase");
        assert!(app.lock_wait() > 0, "wrong passphrase in LockOff also triggers backoff");
    }

    #[test]
    fn home_screen_focus_and_read_only_guard_work_cleanly() {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind,opening_minor) values (1,'Revolut','EUR','BANK',10000);
             insert into category (id,name) values (1,'Food');
             insert into counterparty (id,name,category_id) values (1,'UncatShop',null), (2,'Lidl',1);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,'expense',date('now'),2000,'EUR',-2000,'UncatShop',1,null),
               (1,'expense',date('now'),3000,'EUR',-3000,'Lidl',2,1);",
        )
        .unwrap();
        let mut app = App::new(conn).unwrap();
        app.go_to(Screen::Home);
        assert_eq!(app.screen, Screen::Home);
        assert_eq!((app.home_pane, app.focus), (app::HomePane::Flows, app::Focus::List));

        // Pressing Enter on Flows drills into the selected category's operations
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!(app.screen, Screen::Operations);
        assert!(matches!(&app.ops_view, app::OpsView::Drill { label, .. } if label == "Food"));
        keys(&mut app, &[KeyCode::Esc]);
        assert_eq!(app.screen, Screen::Home);

        // Tab moves from Flows (List) -> Right column (Detail)
        keys(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.focus, app::Focus::Detail);

        // Enable read_only: write actions on the right column ("not reconciled: Revolut") must be refused
        app.read_only = Some(crate::cloud::Mark { device: "Other Mac".into(), since: "2026-10-20T08:00:00Z".into() });
        keys(&mut app, &[KeyCode::Enter]);
        assert_eq!((app.screen, app.mode.clone()), (Screen::Accounts, Mode::Normal), "read-only refuses Home write actions");
        assert!(app.status.as_deref().unwrap_or("").contains("Other Mac"), "{:?}", app.status);
    }
}

