//! Language of everything a person reads: screens, command output, errors,
//! journal lines.
//!
//! Both texts sit side by side at the place they are used — `tr!("Removed
//! {what}", "Убрана {what}")` — rather than in a dictionary keyed by one of
//! them. A dictionary drifts: a key renamed in code silently falls back, and
//! a placeholder added to one language and not the other shows up only at
//! runtime. Here both are format strings checked by the compiler.
//!
//! The language is per thread, not global. The program runs on one thread,
//! and tests run on many: a global would let one test's language leak into
//! another's assertions.
use std::cell::Cell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Ru];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        match code {
            "en" => Some(Lang::En),
            "ru" => Some(Lang::Ru),
            _ => None,
        }
    }

    /// The language's own name for itself.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
        }
    }
}

use std::sync::atomic::{AtomicU8, Ordering};

static GLOBAL_LANG: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static THREAD_LANG: Cell<Option<Lang>> = const { Cell::new(None) };
}

pub fn lang() -> Lang {
    if let Some(l) = THREAD_LANG.with(Cell::get) {
        return l;
    }
    if cfg!(test) {
        return Lang::En;
    }
    match GLOBAL_LANG.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

pub fn set_lang(lang: Lang) {
    THREAD_LANG.with(|cell| cell.set(Some(lang)));
    GLOBAL_LANG.store(match lang { Lang::En => 0, Lang::Ru => 1 }, Ordering::Relaxed);
}

/// A text in the current language, formatted: `tr!("{n} rows", "{n} строк")`.
#[macro_export]
macro_rules! tr {
    ($en:literal, $ru:literal $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => format!($en),
            $crate::i18n::Lang::Ru => format!($ru),
        }
    };
    ($en:literal, $ru:literal, $($arg:tt)+) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => format!($en, $($arg)+),
            $crate::i18n::Lang::Ru => format!($ru, $($arg)+),
        }
    };
}

/// A fixed text in the current language, without formatting — for labels
/// that must be `&'static str`.
#[macro_export]
macro_rules! trs {
    ($en:literal, $ru:literal $(,)?) => {
        match $crate::i18n::lang() {
            $crate::i18n::Lang::En => $en,
            $crate::i18n::Lang::Ru => $ru,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_languages_format_with_captured_names() {
        let n = 3;
        set_lang(Lang::En);
        assert_eq!(tr!("{n} rows", "{n} строк"), "3 rows");
        set_lang(Lang::Ru);
        assert_eq!(tr!("{n} rows", "{n} строк"), "3 строк");
        assert_eq!(tr!("{} of {}", "{} из {}", 1, 2), "1 из 2");
        assert_eq!(trs!("yes", "да"), "да");
        set_lang(Lang::En);
    }

    #[test]
    fn each_thread_starts_in_english() {
        set_lang(Lang::Ru);
        let other = std::thread::spawn(lang).join().unwrap();
        assert_eq!(other, Lang::En);
        set_lang(Lang::En);
    }
}
