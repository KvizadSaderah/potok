//! Every action is declared here once.
//!
//! The keyboard, the hint line and the help screen read this list rather than
//! copies of their own. In the old client (`tui/src/action.rs` in `saldo`) this
//! replaced three independent enumerations that fell out of step with every key
//! added, and four tests hold the invariants across all the screens. The
//! decision was taken whole, the list is our own.
use super::Screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Help,
    Reload,
    NextScreen,
    PrevScreen,
    GoTo(Screen),
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Walk the account the operations are filtered by.
    CycleAccount,
    /// Walk the period the flows are shown for.
    CyclePeriod,
    /// Mark the counterparty under the cursor.
    Categorize,
    /// Every uncategorised counterparty, one after another.
    CategorizeAll,
    /// Drill in: a flow row's operations.
    Open,
    /// Search: over the operations, and on the import screen over counterparties.
    Search,
    /// A step back: out of a drill-in, out of the search, clear the filter.
    Back,
    /// A menu of actions on what is under the cursor — like `x` in lazygit.
    Menu,
    /// The cursor into the details pane.
    FocusDetail,
    /// The cursor back into the list.
    FocusList,
    /// The cursor into the other pane: list ↔ details.
    SwitchPane,
    /// Reconcile the account under the cursor with the remainder at the bank.
    Reconcile,
    /// Load a statement: pick the file, parse it, look, write.
    ImportStatement,
    /// Enter an operation by hand.
    NewOperation,
    /// The journal over a span of dates.
    Dates,
    /// Fold the journal by counterparty.
    Fold,
    /// Another order for the journal.
    Order,
    /// Mark an operation.
    Mark,
    /// Hide or show transfers between own accounts.
    Transfers,
    /// Take writing over from another device.
    TakeWrite,
    /// Lock the screen.
    Lock,
}

pub struct Binding {
    pub keys: &'static [&'static str],
    pub action: Action,
    /// Which screens the action works on. Empty means all of them.
    pub screens: &'static [Screen],
    /// English and Russian; `label()` picks the current one.
    pub label: [&'static str; 2],
}

impl Binding {
    pub fn label(&self) -> &'static str {
        match potok_core::i18n::lang() {
            potok_core::i18n::Lang::En => self.label[0],
            potok_core::i18n::Lang::Ru => self.label[1],
        }
    }
}

pub const BINDINGS: &[Binding] = &[
    Binding { keys: &["q"], action: Action::Quit, screens: &[], label: ["quit", "выйти"] },
    Binding { keys: &["?"], action: Action::Help, screens: &[], label: ["help", "справка"] },
    Binding { keys: &["tab", "shift+tab"], action: Action::SwitchPane, screens: &[], label: ["other pane", "другая панель"] },
    Binding { keys: &["enter"], action: Action::Open, screens: &[], label: ["deeper / expand", "глубже / развернуть"] },
    Binding { keys: &["x"], action: Action::Menu, screens: &[], label: ["what can be done", "что можно сделать"] },
    Binding { keys: &["right", "l"], action: Action::FocusDetail, screens: &[], label: ["into details", "в подробности"] },
    Binding { keys: &["left", "h"], action: Action::FocusList, screens: &[], label: ["back to the list", "обратно к списку"] },
    Binding { keys: &["/"], action: Action::Search, screens: &[], label: ["search", "поиск"] },
    Binding { keys: &["esc"], action: Action::Back, screens: &[], label: ["back", "назад"] },
    Binding { keys: &["r"], action: Action::Reload, screens: &[], label: ["reload", "перечитать"] },
    Binding { keys: &["]"], action: Action::NextScreen, screens: &[], label: ["next screen", "следующий экран"] },
    Binding { keys: &["["], action: Action::PrevScreen, screens: &[], label: ["previous screen", "предыдущий экран"] },
    // Digits are not listed one by one: `resolve` maps 1–9 and 0 to screens
    // by their place in `Screen::ALL`, so a new screen gets its digit by being
    // added there. This row is what help shows for all of them.
    Binding { keys: &["1…0"], action: Action::GoTo(Screen::Accounts), screens: &[], label: ["screens by number", "экраны по номеру"] },
    Binding { keys: &["L"], action: Action::Lock, screens: &[], label: ["lock the screen", "заблокировать экран"] },
    Binding { keys: &["W"], action: Action::TakeWrite, screens: &[], label: ["take writing over from another device", "забрать запись у другого устройства"] },
    Binding { keys: &[","], action: Action::GoTo(Screen::Settings), screens: &[], label: ["settings", "настройки"] },
    Binding { keys: &["u"], action: Action::GoTo(Screen::Journal), screens: &[], label: ["changes and undo", "правки и отмена"] },
    // The digits ran out on the tenth screen, and «Statements» gets a letter —
    // the same one that loads a statement, only capital.
    Binding { keys: &["I"], action: Action::GoTo(Screen::Statements), screens: &[], label: ["statements", "выписки"] },
    Binding { keys: &["k", "up"], action: Action::Up, screens: &[], label: ["up", "выше"] },
    Binding { keys: &["j", "down"], action: Action::Down, screens: &[], label: ["down", "ниже"] },
    Binding { keys: &["pageup"], action: Action::PageUp, screens: &[], label: ["page up", "страница выше"] },
    Binding { keys: &["pagedown"], action: Action::PageDown, screens: &[], label: ["page down", "страница ниже"] },
    Binding { keys: &["g"], action: Action::First, screens: &[], label: ["to the top", "в начало"] },
    Binding { keys: &["G"], action: Action::Last, screens: &[], label: ["to the bottom", "в конец"] },
    Binding {
        keys: &["a"],
        action: Action::CycleAccount,
        screens: &[Screen::Operations],
        label: ["another account", "другой счёт"],
    },
    Binding {
        keys: &["p"],
        action: Action::CyclePeriod,
        screens: &[Screen::Flows],
        label: ["another period", "другой период"],
    },
    Binding {
        keys: &["s"],
        action: Action::Reconcile,
        screens: &[Screen::Accounts],
        label: ["reconcile with the bank", "сверить с банком"],
    },
    Binding {
        keys: &["n"],
        action: Action::NewOperation,
        screens: &[Screen::Accounts, Screen::Operations],
        label: ["new operation", "новая операция"],
    },
    Binding { keys: &["n"], action: Action::NewOperation, screens: &[Screen::Debts], label: ["new debt", "новый долг"] },
    Binding { keys: &[" "], action: Action::Mark, screens: &[Screen::Operations], label: ["mark", "отметить"] },
    Binding { keys: &["d"], action: Action::Dates, screens: &[Screen::Operations], label: ["dates", "даты"] },
    Binding { keys: &["f"], action: Action::Fold, screens: &[Screen::Operations], label: ["fold by who", "свернуть по кому"] },
    Binding { keys: &["o"], action: Action::Order, screens: &[Screen::Operations], label: ["order", "порядок"] },
    Binding { keys: &["t"], action: Action::Transfers, screens: &[Screen::Operations], label: ["hide transfers", "скрыть переводы"] },
    Binding {
        keys: &["i"],
        action: Action::ImportStatement,
        screens: &[Screen::Counterparties, Screen::Statements],
        label: ["import a statement", "загрузить выписку"],
    },
    Binding {
        keys: &["c"],
        action: Action::Categorize,
        screens: &[Screen::Counterparties],
        label: ["categorise the counterparty", "разметить контрагента"],
    },
    Binding {
        keys: &["C"],
        action: Action::CategorizeAll,
        screens: &[Screen::Counterparties],
        label: ["categorise everyone without a category, in turn", "разметить всех без категории подряд"],
    },
];

/// What a key does on this screen. The one path from a press to an action.
pub fn resolve(key: &str, screen: Screen) -> Option<Action> {
    if let Some(target) = Screen::ALL.into_iter().find(|s| s.key() == key) {
        return Some(Action::GoTo(target));
    }
    BINDINGS
        .iter()
        .find(|binding| {
            binding.keys.contains(&key)
                && (binding.screens.is_empty() || binding.screens.contains(&screen))
        })
        .map(|binding| binding.action)
}

/// What to show in the hint line on this screen: only what works here, and in
/// the same order as in the help. The order of the list above is chosen for it:
/// the line is clipped on the right, and what the interface cannot be used
/// without — Enter, `x`, `→` — has to come first.
pub fn hints(screen: Screen) -> Vec<(&'static str, &'static str)> {
    BINDINGS
        .iter()
        .filter(|binding| !binding.screens.is_empty() && binding.screens.contains(&screen))
        .chain(BINDINGS.iter().filter(|binding| binding.screens.is_empty()))
        .map(|binding| (binding.keys[0], binding.label()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two actions cannot take one key on one screen — otherwise which of them
    /// fires is decided by the order in the list, and that is not something one
    /// wants to find out by tracing.
    #[test]
    fn no_two_actions_claim_the_same_key_on_one_screen() {
        for screen in Screen::ALL {
            let mut seen: Vec<&str> = Vec::new();
            for binding in BINDINGS {
                if !binding.screens.is_empty() && !binding.screens.contains(&screen) {
                    continue;
                }
                for key in binding.keys {
                    assert!(
                        !seen.contains(key),
                        "клавиша {key:?} занята дважды на экране {screen:?}"
                    );
                    seen.push(key);
                }
            }
        }
    }

    /// Every action has at least one key and a label: an action with no label
    /// will not reach the help and will stay a secret.
    #[test]
    fn every_binding_has_a_key_and_a_label() {
        for binding in BINDINGS {
            assert!(!binding.keys.is_empty(), "{:?} без клавиши", binding.action);
            assert!(binding.label.iter().all(|l| !l.is_empty()), "{:?} has no label", binding.action);
        }
    }

    /// Quit and help must work on any screen: those are the only two actions
    /// without which one can get stuck.
    #[test]
    fn quit_and_help_work_everywhere() {
        for screen in Screen::ALL {
            assert_eq!(resolve("q", screen), Some(Action::Quit));
            assert_eq!(resolve("?", screen), Some(Action::Help));
        }
    }

    /// The hint shows exactly what works on this screen.
    #[test]
    fn the_hint_line_shows_what_the_screen_can_do() {
        let import = hints(Screen::Counterparties);
        assert!(import.iter().any(|(key, _)| *key == "c"), "{import:?}");
        let accounts = hints(Screen::Accounts);
        assert!(!accounts.iter().any(|(key, _)| *key == "c"), "{accounts:?}");
        assert!(accounts.iter().any(|(key, _)| *key == "q"));
    }

    #[test]
    fn search_back_and_enter_work_everywhere() {
        for screen in Screen::ALL {
            assert_eq!(resolve("/", screen), Some(Action::Search));
            assert_eq!(resolve("esc", screen), Some(Action::Back));
        }
        for screen in Screen::ALL {
            assert_eq!(resolve("enter", screen), Some(Action::Open), "Enter глубже на {screen:?}");
        }
    }

    /// Every screen is reachable by a digit, and no digit leads nowhere.
    #[test]
    fn every_screen_has_its_own_digit() {
        for screen in Screen::ALL.iter().take(10) {
            let key = screen.key();
            assert_eq!(
                resolve(&key, Screen::Accounts),
                Some(Action::GoTo(*screen)),
                "цифра {key} должна вести на {screen:?}"
            );
        }
    }
}
