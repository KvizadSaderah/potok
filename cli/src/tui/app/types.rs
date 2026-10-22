//! What the screen's state is made of: kinds of list, modes, menus, forms.
//! The behaviour is in the neighbouring files, here are the declarations only.
use super::*;

pub use potok_core::period::Period;

/// What the operations screen is showing now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpsView {
    /// The whole journal, filtered by account.
    All,
    /// The operations of one flow row — drilling in. Esc returns to the flows.
    Drill { label: String, from: String, to: String },
    /// A search over the description, the counterparty and the amount.
    Search { text: String },
    /// One account's operations — Enter on the account. Esc returns to them.
    Account { id: i64, name: String },
    /// One counterparty's operations — Enter on the import. Esc returns there.
    Counterparty { name: String },
    /// One loaded statement's operations — Enter on it.
    Import { id: i64, name: String },
    /// The operations over a date span — Enter on a month in the analytics, or `d`.
    Range { from: String, to: String },
}

/// What the details pane shows: whatever is under the cursor.
#[derive(Debug, Clone)]
pub enum Detail {
    Nothing,
    Account(detail::AccountDetail),
    Operation(Box<detail::OperationDetail>),
    Flow(detail::FlowDetail),
    Debt(detail::DebtDetail),
    Counterparty(detail::CounterpartyDetail),
    Duplicates(potok_core::reconcile::DuplicateGroup),
    Change(potok_core::journal::Entry),
    Setting(SettingKey),
    Category(potok_core::category::CategoryLine),
    /// A month: its figures, comparisons, and what it was spent on / earned from.
    Month {
        month: potok_core::analytics::Month,
        prev_out: Option<potok_core::Money>,
        avg_out: Option<potok_core::Money>,
        flows: Vec<potok_core::query::Flow>,
    },
    /// A budget and its category over the last six months.
    Budget { line: potok_core::budget::BudgetLine, trend: Vec<(chrono::NaiveDate, potok_core::MoneyBag)> },
}

/// Rows of the settings screen, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKey {
    Language,
    Theme,
    StatementsDir,
    DefaultAccount,
    FlowsPeriod,
    BaseCurrency,
    Rates,
    TransferDays,
    BackupDir,
    BackupNow,
    Device,
    Cloud,
    ScreenLock,
    LockAfter,
    MonthDay,
    MonthStart,
}

impl SettingKey {
    /// In the order the screen shows them, a section at a time.
    pub const ALL: [SettingKey; 16] = [
        SettingKey::Language,
        SettingKey::Theme,
        SettingKey::DefaultAccount,
        SettingKey::FlowsPeriod,
        SettingKey::BaseCurrency,
        SettingKey::Rates,
        SettingKey::TransferDays,
        SettingKey::MonthDay,
        SettingKey::MonthStart,
        SettingKey::StatementsDir,
        SettingKey::BackupDir,
        SettingKey::BackupNow,
        SettingKey::Device,
        SettingKey::Cloud,
        SettingKey::ScreenLock,
        SettingKey::LockAfter,
    ];

    /// The heading it sits under.
    pub fn section(self) -> &'static str {
        match self {
            SettingKey::Language | SettingKey::Theme => trs!("Look", "Вид"),
            SettingKey::DefaultAccount | SettingKey::FlowsPeriod | SettingKey::BaseCurrency | SettingKey::Rates | SettingKey::TransferDays => {
                trs!("Ledger", "Учёт")
            }
            SettingKey::MonthDay | SettingKey::MonthStart => trs!("Month", "Месяц"),
            SettingKey::StatementsDir | SettingKey::BackupDir | SettingKey::BackupNow => trs!("Files", "Файлы"),
            SettingKey::Device | SettingKey::Cloud => trs!("Google Drive and devices", "Google Drive и устройства"),
            SettingKey::ScreenLock | SettingKey::LockAfter => trs!("Lock", "Блокировка"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SettingKey::Language => trs!("Language", "Язык"),
            SettingKey::Theme => trs!("Theme", "Тема"),
            SettingKey::StatementsDir => trs!("Statements folder", "Папка выписок"),
            SettingKey::DefaultAccount => trs!("Account for a new operation", "Счёт для новой операции"),
            SettingKey::FlowsPeriod => trs!("Flows open on", "Потоки открываются на"),
            SettingKey::BaseCurrency => trs!("Base currency", "Основная валюта"),
            SettingKey::Rates => trs!("Exchange rates", "Курсы валют"),
            SettingKey::TransferDays => trs!("Transfer legs apart, days", "Ноги перевода, дней врозь"),
            SettingKey::BackupDir => trs!("Backups folder", "Папка бэкапов"),
            SettingKey::BackupNow => trs!("Last backup", "Последний бэкап"),
            SettingKey::Cloud => trs!("Copy in Google Drive", "Копия в Google Drive"),
            SettingKey::Device => trs!("This device", "Это устройство"),
            SettingKey::ScreenLock => trs!("Screen lock", "Блокировка экрана"),
            SettingKey::LockAfter => trs!("Lock when idle", "Блокировать при простое"),
            SettingKey::MonthDay => trs!("Month starts", "Месяц начинается"),
            SettingKey::MonthStart => trs!("This month started", "Этот месяц начался"),
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            SettingKey::Language => trs!(
                "The language of every screen, of command output and of new journal lines. Lines already in the journal stay as written.",
                "Язык экранов, вывода команд и новых строк журнала. Строки, уже записанные в журнал, остаются как были."
            ),
            SettingKey::Theme => trs!(
                "saldo — the old client's 256 colours for a dark terminal; terminal — your terminal's own sixteen; light — for a light background.",
                "saldo — 256 цветов старого клиента для тёмного терминала; terminal — шестнадцать цветов твоего терминала; light — для светлого фона."
            ),
            SettingKey::StatementsDir => trs!(
                "Where i on the statements screen looks for PDF statements, newest first.",
                "Где i на экране выписок ищет PDF-выписки, новые сверху."
            ),
            SettingKey::DefaultAccount => trs!(
                "The account n starts a new operation on. Auto — the account under the cursor or in the journal filter, else cash.",
                "Счёт, с которого n начинает новую операцию. Авто — счёт под курсором или в фильтре журнала, иначе наличные."
            ),
            SettingKey::FlowsPeriod => trs!(
                "The period the flows screen shows when potok starts; p still switches it.",
                "Период, который показывают потоки при запуске potok; p по-прежнему его меняет."
            ),
            SettingKey::BackupDir => trs!(
                "Where a copy of the ledger goes once a day, 30 kept. Point it at a synced folder (iCloud Drive, Dropbox) to have the copy off this machine. Restore: potok restore.",
                "Куда раз в сутки кладётся копия леджера, хранятся 30. Укажи синхронизируемую папку (iCloud Drive, Dropbox), чтобы копия была не только на этом ноутбуке. Восстановить: potok restore."
            ),
            SettingKey::BackupNow => trs!("Enter makes a backup now.", "Enter делает бэкап сейчас."),
            SettingKey::Device => trs!(
                "The name other devices see in «who writes now». One device writes at a time; another opens read only and can take writing over with W. Enter — rename; empty — the Mac's own name.",
                "Имя, которое другие устройства видят в «кто сейчас пишет». Пишет одно устройство за раз; другое открывается только на чтение и может забрать запись клавишей W. Enter — переименовать; пусто — имя мака."
            ),
            SettingKey::Cloud => trs!(
                "The newest backup, encrypted with your passphrase, sent to Google Drive once a day. Set up and send from a terminal: potok cloud login, potok cloud push. Get one back: potok cloud get, then potok restore.",
                "Свежий бэкап, зашифрованный твоим паролем, раз в сутки уходит в Google Drive. Настроить и отправить — в терминале: potok cloud login, potok cloud push. Вернуть: potok cloud get, затем potok restore."
            ),
            SettingKey::ScreenLock => trs!(
                "A passphrase asked at start; L locks at once. A screen lock, not protection for the file: the ledger is plain SQLite. Not the password of anything else.",
                "Пароль при запуске; L блокирует сразу. Это блокировка экрана, а не защита файла: леджер — обычный SQLite. Не пароль от чего-то ещё."
            ),
            SettingKey::LockAfter => trs!(
                "Lock on its own after this many minutes without a key pressed. Needs a passphrase set above.",
                "Блокировать самому после стольких минут без нажатий. Нужен пароль строкой выше."
            ),
            SettingKey::MonthDay => trs!(
                "When an accounting month starts — a fixed day, or the last working day of the month, when a salary usually comes. Flows, analytics and budgets count months by it. The rule knows weekends, not holidays: correct a month below or with x on it in Analytics.",
                "Когда начинается учётный месяц — с числа или с последнего рабочего дня месяца, когда обычно приходит зарплата. По нему считают месяцы потоки, аналитика и бюджеты. Правило знает выходные, но не праздники: месяц можно поправить ниже или через x на нём в аналитике."
            ),
            SettingKey::MonthStart => trs!(
                "When the current month really started. Enter — correct it if the salary came on another day; empty — back to the rule. Kept in the journal and undone like any edit.",
                "Когда на самом деле начался текущий месяц. Enter — поправить, если зарплата пришла в другой день; пусто — вернуть правило. Попадает в журнал и отменяется, как любая правка."
            ),
            SettingKey::BaseCurrency => trs!(
                "The currency of the total on Accounts and of budgets. Other currencies are converted by the rates below.",
                "Валюта итога на экране счетов и бюджетов. Остальные валюты пересчитываются по курсам ниже."
            ),
            SettingKey::Rates => trs!(
                "Every account currency against the base. Enter: fetch now from the ECB (EUR against the world) and the Bank of Russia (RUB), fetch daily on start, add a rate by hand, or delete a wrong one. A pair with no direct rate goes through EUR.",
                "Каждая валюта счетов к основной. Enter: загрузить сейчас с ЕЦБ (евро ко всем) и ЦБ РФ (рубль), загружать каждый день при запуске, добавить курс руками или удалить неверный. Пара без прямого курса считается через евро."
            ),
            SettingKey::TransferDays => trs!(
                "How many days apart the two legs of a transfer may be when potok looks for one — on import and in «find the other leg».",
                "На сколько дней могут разойтись две ноги перевода, когда potok ищет пару, — при импорте и в «найти вторую ногу»."
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Help,
    /// Typing the search line; the list updates on every letter.
    Search,
    /// Choosing a category: for the counterparty under the cursor or for one operation.
    PickCategory { selected: usize },
    /// A menu of actions on what is under the cursor (`x`, as in lazygit).
    Menu { selected: usize },
    /// A «yes or no» question before what moves money.
    Confirm,
    /// Typing the remainder at the bank, to reconcile the account under the cursor.
    Reconcile,
    /// Typing the path to a statement.
    Path,
    /// The statement-parsing window: what will be written, the hints, the marks.
    Review,
    /// The screen is locked: it waits for the passphrase.
    Locked,
    /// An operation's form: new or a correction.
    Form,
    /// Typing the opening balance of the account under the cursor.
    Opening,
    /// Typing the text for whatever is in `App::asking`.
    Input,
}

/// What text is being typed for in `Mode::Input`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A new name for this counterparty.
    Rename(String),
    /// A note on a counterparty: what this place actually is.
    Note(String),
    /// One more spelling for this counterparty.
    Alias(String),
    /// An own note on an operation.
    OperationNote(i64),
    /// Whom to fold this counterparty into — part of a name.
    MergeInto(String),
    /// Whose counterparty this operation's really is.
    Respell(i64),
    /// The folder to look for statements in.
    StatementsDir,
    /// The backups folder.
    BackupDir,
    /// This device's name in the writer mark.
    DeviceName,
    /// A new name for the account.
    RenameAccount(i64),
    /// Splitting an operation across categories.
    Split(i64),
    RenameCategory(String),
    MergeCategory(String),
    /// A monthly limit on a category.
    Budget(String),
    /// The rate against the main currency.
    Rate,
    /// A date span for the journal.
    Range,
    /// A new lock passphrase.
    LockSet,
    /// Unlocking — the current passphrase is needed.
    LockOff,
    /// Correcting the start of the accounting month (the rule's calendar month).
    MonthStart((i32, u32)),
}

/// A form's field: text, or a choice from a list (←→ walk it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// What the form reads the field by — the same in every language.
    pub key: &'static str,
    pub label: &'static str,
    pub text: String,
    pub choices: Vec<String>,
    pub chosen: usize,
    /// What to show when the field is empty and optional.
    pub empty: &'static str,
}

impl Field {
    pub(super) fn text(key: &'static str, label: &'static str, text: impl Into<String>) -> Self {
        Self { key, label, text: text.into(), choices: Vec::new(), chosen: 0, empty: "" }
    }

    pub(super) fn optional(key: &'static str, label: &'static str, text: impl Into<String>, empty: &'static str) -> Self {
        Self { empty, ..Self::text(key, label, text) }
    }

    pub(super) fn choice(key: &'static str, label: &'static str, choices: Vec<String>, chosen: usize) -> Self {
        Self { choices, chosen, ..Self::text(key, label, "") }
    }

    pub fn shown(&self) -> String {
        match self.choices.get(self.chosen) {
            Some(choice) => format!("‹ {choice} ›"),
            None => self.text.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormKind {
    /// An operation by hand; `replacing` — a correction of that one.
    Operation { replacing: Option<i64> },
    /// A debt; `with_operation` — the first lending on it.
    Debt { with_operation: Option<i64> },
    /// A new account.
    Account,
    /// Editing a debt.
    DebtEdit(i64),
    /// A transfer between one's own by hand.
    Transfer,
}

/// A form over the screen: an operation or a debt. The fields are in the order
/// Tab walks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub kind: FormKind,
    pub title: String,
    pub fields: Vec<Field>,
    pub field: usize,
    pub error: Option<String>,
}

impl Form {
    pub fn value(&self, key: &str) -> &str {
        self.fields.iter().find(|f| f.key == key).map(|f| f.text.as_str()).unwrap_or("")
    }

    pub fn chosen(&self, key: &str) -> usize {
        self.fields.iter().find(|f| f.key == key).map(|f| f.chosen).unwrap_or(0)
    }

    pub fn choice(&self, key: &str) -> &str {
        self.fields
            .iter()
            .find(|f| f.key == key)
            .and_then(|f| f.choices.get(f.chosen))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// The field the letters go into now; a choice from a list has none.
    pub(super) fn text_mut(&mut self) -> Option<&mut String> {
        self.fields.get_mut(self.field).filter(|f| f.choices.is_empty()).map(|f| &mut f.text)
    }
}

/// A statement parsed and not yet written.
#[derive(Debug, Clone)]
pub struct Review {
    pub path: std::path::PathBuf,
    pub statement: potok_core::import::Statement,
    pub plan: potok_core::import::review::Plan,
    /// The cursor walks the new rows: duplicates and pending ones are not
    /// written, and there is nothing to decide about them.
    pub cursor: usize,
}

impl Review {
    /// The numbers of the plan's new rows — what the window shows.
    pub fn new_rows(&self) -> Vec<usize> {
        use potok_core::import::review::RowStatus;
        (0..self.plan.rows.len()).filter(|&i| self.plan.rows[i].status == RowStatus::New).collect()
    }
}

/// Where the cursor is: in the list on the left or in the details on the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Detail,
}

/// Which of the two left panes is chosen on the analytics screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsPane {
    Months,
    Regular,
}

/// A row inside the details one can stand on and act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailItem {
    Operation(i64),
    /// A spelling in a counterparty's card: one stands on it and removes it.
    Alias(String),
    Movement { id: i64, operation: Option<i64> },
    MonthFlow { label: String, from: String, to: String },
}

/// What a menu item does. Each is one call into `potok_core::edit`, or a move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    OpenOperation(i64),
    RemoveOperation(i64),
    RestoreOperation(i64),
    UnlinkMovement(i64),
    OwnCategory(i64),
    /// Remove a spelling from the counterparty's card.
    DropAlias(String),
    ClearOwnCategory(i64),
    /// «This operation is not Other but Healthcare»: move the same
    /// counterparty's other operations there too, and remember it for the future.
    PromoteCategory { operation: i64, who: String, category: String, from: Option<String> },
    CategorizeCounterparty(String),
    CounterpartyOperations(String),
    UnpairTransfer(i64),
    FindPair(i64),
    PairWith(i64, i64),
    DismissDuplicates(String),
    DismissAll(Vec<String>),
    NewAccount,
    NewTransfer,
    GoCategories,
    MarkedCategory,
    MarkedCount(potok_core::edit::Counting),
    MarkedRemove,
    MarkedClear,
    RatesMenu,
    RatesUpdate,
    RatesAuto(bool),
    RateDelete(String, String, String),
    ExportCsv,
    DeleteCategory(String),
    MergeCategory(String, String),
    EditDebt(i64),
    DeleteDebt(i64),
    Archive(i64, bool),
    ShowArchived,
    ImportFile(std::path::PathBuf),
    ImportInto(std::path::PathBuf, String),
    ImportPath,
    WriteImport,
    /// Esc in the review, once confirmed: nothing is written.
    DropReview,
    /// Every uncategorised counterparty whose hint is near certain.
    TakeHints,
    /// Every refund still counted as income, as a refund.
    TakeRefunds,
    Unimport(i64),
    Fix(i64),
    Opening(i64),
    PickDebt(i64),
    LinkDebt(i64, i64),
    NewDebtWith(i64),
    WriteOff(i64),
    Undo(i64),
    Count(i64, potok_core::edit::Counting),
    Ask(Ask),
    Merge(String, String),
    AccountOperations(i64),
    Drill,
    Zoom,
}

/// How sure a hint must be to be taken for all at once: this share of the
/// counterparty's operations already in the category.
pub const HINT_PERCENT: usize = 80;

impl MenuAction {
    /// Changes the ledger. While another device writes, these are not offered:
    /// the file is open to read only, and SQLite would refuse them anyway.
    pub fn writes(&self) -> bool {
        !matches!(
            self,
            MenuAction::OpenOperation(_)
                | MenuAction::CounterpartyOperations(_)
                | MenuAction::GoCategories
                | MenuAction::MarkedClear
                | MenuAction::ExportCsv
                | MenuAction::ShowArchived
                | MenuAction::AccountOperations(_)
                | MenuAction::Drill
                | MenuAction::Zoom
                | MenuAction::Ask(Ask::Range)
                | MenuAction::DropReview
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub action: MenuAction,
    /// It moves money — ask «yes or no» before doing it.
    pub confirm: Option<String>,
}

/// The Overview's left side is two lists: the month columns and «where it went».
/// The right stack is `Focus::Detail`, like the details on other screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomePane {
    Flows,
    Months,
}

/// The chosen column when it is not this month: «where it went» shows that one,
/// and it has to be said plainly. A screen where one month's figures stand
/// under another month's name is what this struct exists for.
#[derive(Debug, Clone)]
pub struct PastMonth {
    /// Which column is shown, as an index into `home_months()`.
    pub at: usize,
    pub from: String,
    pub to: String,
    pub flows: Vec<potok_core::query::Flow>,
}

/// A row of the Overview's right pane and where Enter on it leads. The rows are
/// what has to be done; they used to be labels with a hint of the keys, and a
/// person pressed those themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeGo {
    /// A regular payment: this counterparty's operations.
    Counterparty(String),
    Debt(i64),
    /// Reconcile this account with the bank.
    Reconcile(String),
    Duplicates,
    /// Marking counterparties in a row, the dearest first.
    Categorise,
    /// Count refunds as refunds — with the same question as in the import menu.
    Refunds,
    Import,
    Accounts,
    Settings,
}

/// A «Look at» row: what is wrong, which keys fix it by hand, and what Enter
/// does.
#[derive(Debug, Clone)]
pub struct LookRow {
    pub text: String,
    pub keys: &'static str,
    pub go: HomeGo,
}

/// Whom a category is being chosen for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickTarget {
    /// The counterparty under the cursor on the «Counterparties» screen.
    Counterparty,
    /// A named counterparty: marking from an operation's menu, where the cursor
    /// stands on a journal row while the one it paid is marked.
    Named(String),
    Operation(i64),
    /// Every operation marked with Space.
    Marked,
}

/// A row in the category-choosing window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// A hint: the commonest category among its operations.
    Hint { category: String, count: usize, of: usize },
    Existing(String),
    /// The typed name is not among the categories — create it.
    Create(String),
}

impl Pick {
    pub fn category(&self) -> &str {
        match self {
            Pick::Hint { category, .. } | Pick::Existing(category) | Pick::Create(category) => {
                category
            }
        }
    }
}

