//! Colour. One place, so a screen cannot invent its own meaning for red.
//!
//! Four palettes, chosen in Settings:
//!
//! * **terminal** — the terminal's own sixteen colours, so the theme the
//!   person already chose shows through untouched. Works on a light
//!   background too, because the text takes the terminal's own foreground.
//! * **midnight** — deep blue ink: structure in cornflower, mint and rose for
//!   money, a lit row under the cursor. Truecolour.
//! * **amber** — a warm screen: amber text, orange structure. The money
//!   colours stay green and red, or the rule would break.
//! * **mono** — greyscale and one white accent, money in the quietest green
//!   and red that still read. For when the numbers should be the only thing
//!   with a colour.
//!
//! None of them paints the background. Green always means money came in and
//! red that it left; nothing else is coloured that way.
use std::cell::Cell;

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Terminal,
    Midnight,
    Amber,
    Mono,
    Neon,
    Green,
}

impl Theme {
    pub const ALL: [Theme; 6] = [Theme::Terminal, Theme::Midnight, Theme::Amber, Theme::Mono, Theme::Neon, Theme::Green];

    pub fn code(self) -> &'static str {
        match self {
            Theme::Terminal => "terminal",
            Theme::Midnight => "midnight",
            Theme::Amber => "amber",
            Theme::Mono => "mono",
            Theme::Neon => "neon",
            Theme::Green => "green",
        }
    }

    /// The «saldo» and «light» themes are gone. A ledger with one of them
    /// written in it opens on «terminal» rather than panicking or quietly
    /// losing the setting: the settings show what is set.
    pub fn from_code(code: &str) -> Option<Theme> {
        match code {
            "saldo" | "light" => Some(Theme::Terminal),
            _ => Theme::ALL.into_iter().find(|t| t.code() == code),
        }
    }

    fn palette(self) -> Palette {
        // The colours are given as RGB, except «terminal»: there it is the
        // terminal's own sixteen colours, as a person set them up.
        let rgb = Color::Rgb;
        match self {
            Theme::Terminal => Palette {
                fg: Color::Reset,
                dim: Color::Gray,
                accent: Color::Magenta,
                border: Color::DarkGray,
                inn: Color::Green,
                out: Color::Red,
                warn: Color::Yellow,
                selected_bg: Color::DarkGray,
            },
            Theme::Midnight => Palette {
                fg: rgb(0xc8, 0xd3, 0xf5),
                dim: rgb(0x72, 0x7d, 0xa6),
                accent: rgb(0x82, 0xaa, 0xff),
                border: rgb(0x33, 0x3b, 0x55),
                inn: rgb(0xc3, 0xe8, 0x8d),
                out: rgb(0xff, 0x75, 0x7f),
                warn: rgb(0xff, 0xc7, 0x77),
                selected_bg: rgb(0x2d, 0x3f, 0x76),
            },
            Theme::Amber => Palette {
                fg: rgb(0xf5, 0xc9, 0x7a),
                dim: rgb(0xa3, 0x7c, 0x45),
                accent: rgb(0xff, 0x9e, 0x64),
                border: rgb(0x4a, 0x38, 0x20),
                inn: rgb(0x9e, 0xce, 0x6a),
                out: rgb(0xf7, 0x76, 0x8e),
                warn: rgb(0xff, 0xd5, 0x80),
                selected_bg: rgb(0x47, 0x32, 0x15),
            },
            // Dark ultraviolet: a pink structure, turquoise income.
            Theme::Neon => Palette {
                fg: rgb(0xe8, 0xe3, 0xff),
                dim: rgb(0x8b, 0x7f, 0xc4),
                accent: rgb(0xff, 0x3d, 0xd8),
                border: rgb(0x47, 0x2b, 0x6b),
                inn: rgb(0x2b, 0xff, 0xc6),
                out: rgb(0xff, 0x44, 0x7c),
                warn: rgb(0xff, 0xe4, 0x4f),
                selected_bg: rgb(0x3d, 0x16, 0x63),
            },
            // Phosphor: a green screen, but money is still red and green —
            // otherwise colour stops meaning one thing.
            Theme::Green => Palette {
                fg: rgb(0xa9, 0xf0, 0xa0),
                dim: rgb(0x5f, 0xa0, 0x62),
                accent: rgb(0x5c, 0xff, 0x46),
                border: rgb(0x23, 0x4f, 0x27),
                inn: rgb(0x8c, 0xff, 0x70),
                out: rgb(0xff, 0x7a, 0x7a),
                warn: rgb(0xe3, 0xff, 0x7a),
                selected_bg: rgb(0x15, 0x3d, 0x1b),
            },
            Theme::Mono => Palette {
                fg: rgb(0xe4, 0xe4, 0xe4),
                dim: rgb(0x8a, 0x8a, 0x8a),
                accent: rgb(0xff, 0xff, 0xff),
                border: rgb(0x3a, 0x3a, 0x3a),
                inn: rgb(0x8f, 0xbc, 0x8f),
                out: rgb(0xd9, 0x82, 0x82),
                warn: rgb(0xd7, 0xaf, 0x5f),
                selected_bg: rgb(0x33, 0x33, 0x33),
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Palette {
    fg: Color,
    dim: Color,
    accent: Color,
    border: Color,
    inn: Color,
    out: Color,
    warn: Color,
    selected_bg: Color,
}

thread_local! {
    static THEME: Cell<Theme> = const { Cell::new(Theme::Terminal) };
}

pub fn set(theme: Theme) {
    THEME.with(|cell| cell.set(theme));
}

pub fn current() -> Theme {
    THEME.with(Cell::get)
}

fn p() -> Palette {
    current().palette()
}

pub fn accent() -> Color {
    p().accent
}

/// Money out, and anything that should stop the eye: an error, «off by».
pub fn bad() -> Color {
    p().out
}

/// Money in, and «matches».
pub fn good() -> Color {
    p().inn
}

pub fn warn() -> Color {
    p().warn
}

pub fn label() -> Style {
    Style::default().fg(p().dim)
}

pub fn value() -> Style {
    Style::default().fg(p().fg)
}

pub fn strong() -> Style {
    Style::default().fg(p().fg).add_modifier(Modifier::BOLD)
}

pub fn heading() -> Style {
    Style::default().fg(p().accent).add_modifier(Modifier::BOLD)
}

pub fn border() -> Style {
    Style::default().fg(p().border)
}

pub fn border_focus() -> Style {
    Style::default().fg(p().accent)
}

/// The frame of a pane — part of each theme's look, like its colours: the
/// terminal one's square and doubled where the keys are, the others rounded
/// and thick on focus.
pub fn frame(focused: bool) -> ratatui::widgets::BorderType {
    use ratatui::widgets::BorderType;
    match (current(), focused) {
        (Theme::Terminal, true) => BorderType::Double,
        (Theme::Terminal, false) => BorderType::Plain,
        (_, true) => BorderType::Thick,
        (_, false) => BorderType::Rounded,
    }
}

pub fn selected() -> Style {
    // A background rather than an inversion. Inverting swapped the row's colour
    // and the background: «Other» under the cursor became black on white, and
    // that was the one place on the screen where colour meant something else.
    // A background leaves the row its colours — a sum stays red, grey stays
    // grey but readable, because `dim` is lighter than `selected_bg` everywhere.
    Style::default().fg(p().fg).bg(p().selected_bg).add_modifier(Modifier::BOLD)
}

// ── Roles ────────────────────────────────────────────────────────────────
//
// What a screen asks for is a role, never a colour: the same thing looks the
// same on every screen. Blue is structure — headings, keys, focus, the
// current period in a chart; white is content; grey is what explains it;
// green and red are money in and out and nothing else besides «matches» and
// «failed»; yellow is «needs you». `ui.rs` builds no style of its own — a
// test holds it to that.

/// A heading inside a pane: a section, a day, a month. Blue, bold.
pub fn section() -> Style {
    heading()
}

/// What a detail pane is about — a name, an account. White, bold.
pub fn subject() -> Style {
    strong()
}

/// A key to press, and the prompt of a line being typed.
pub fn key() -> Style {
    heading()
}

/// A date beside something: secondary, like any label.
pub fn date() -> Style {
    label()
}

/// Something waiting on the person: not reconciled, not sorted, overdue, a
/// debt still open, read only.
pub fn attention() -> Style {
    Style::default().fg(p().warn)
}

/// It did not work, or it is off: failed, off by, over the limit.
pub fn error() -> Style {
    Style::default().fg(p().out)
}

/// It worked: matches, sent, on.
pub fn ok() -> Style {
    Style::default().fg(p().inn)
}

/// A bar of money spent.
pub fn spending() -> Style {
    Style::default().fg(p().out)
}

/// A bar of money in.
pub fn earning() -> Style {
    Style::default().fg(p().inn)
}

/// A chart of something neither in nor out — a balance — and the current
/// period among past ones.
pub fn chart() -> Style {
    Style::default().fg(p().accent)
}

/// A row under the cursor that keeps its own colours — an amount stays red
/// or green — on the cursor's background.
pub fn on_cursor(base: Style) -> Style {
    base.patch(selected()).fg(base.fg.unwrap_or(p().fg))
}

/// A colour picked by a rule elsewhere (a budget's state), as a style.
pub fn tone(color: Color) -> Style {
    Style::default().fg(color)
}

/// A label set in reverse — the «read only» badge.
pub fn badge(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::REVERSED)
}

/// A signed amount takes its colour from its sign, always.
pub fn money(minor: i64) -> Style {
    match minor {
        m if m < 0 => Style::default().fg(p().out),
        m if m > 0 => Style::default().fg(p().inn),
        _ => label(),
    }
}
