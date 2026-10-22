//! `potok` — one binary.
//!
//! With no arguments it should open the TUI (stage 2). While there is no TUI, a
//! run with no arguments prints what it can do, and the work goes by subcommands.
//!
//! The arguments are parsed by hand. Five subcommands and three flags are not
//! worth an outside dependency; `clap` will arrive together with shell
//! completion and a `--help` in sections, when there is something to describe.
use std::path::PathBuf;

use anyhow::{Context, Result};
mod cloud;
mod secrets;
mod mcp;
mod tui;

use potok_core::{db, import, query, reconcile, settings, tr, trs, verify};

/// `println!` in the ledger's language: `say!("{n} rows", "{n} строк")`.
macro_rules! say {
    ($($t:tt)*) => { println!("{}", potok_core::tr!($($t)*)) };
}

const USAGE_EN: &str = "\
potok — a local ledger

  potok                        open the interface
  potok account add <name>     create an account
  potok account rename <name> --into <new> | archive <name> | unarchive <name> | opening <name> <amount>
  potok import <file>          review a statement and write it
  potok imports                what was imported
  potok unimport <number>      roll an import back whole
  potok accounts               accounts and their balances
  potok add <account> <amount> <who> [--on date] [--category what]
                               enter an operation by hand: 12.50 — expense, +12.50 — income
  potok counterparties         who shows up and for how much
  potok categorize <who> --category <what> [--from <old>]   categorise a counterparty
  potok counterparty merge <who> --into <who>   merge two into one
  potok counterparty rename <who> --into <name>   rename by hand
  potok counterparty note <who> [<text>]   a note of your own; without text it shows it, --clear removes it
  potok counterparty refresh-names   rebuild counterparty names by the current rules
  potok alias add <text> --into <who> | remove <text>   a spelling that leads to a counterparty
  potok debts                  debts and what is left
  potok debt open <who> <i-owe|owed-to-me> [amount] [--with operation] [--on date] [--currency EUR]
                               open a debt: by an amount with no operation, or from an operation (--with)
  potok debt link <debt> <operation>   link an operation to a debt
  potok debt write-off <debt>  write off what is left
  potok debt due <debt> <date|-> | note <debt> <text|-> | delete <debt>
  potok flows [--from A] [--to B]  where the money went; no dates — this accounting month
  potok verify                 check the invariants
  potok role <operation> <count|no-count|transit|refund>   how an operation enters the flows
  potok refunds [apply]        money back from purchases still counted as income; apply — count them as refunds
  potok hints [apply]          uncategorised counterparties whose operations mostly agree on one category
  potok op <operation> remove | restore | unpair | pair <operation> | category <name|-> | split \"Food 30; Home 12.50\"
  potok category rename <name> --into <new> | merge <name> --into <other> | delete <name> | budget <name> <amount|->
  potok month <YYYY-MM> <start|->   correct when an accounting month starts
  potok export [--from A] [--to B] [--account X]   operations as CSV on stdout
  potok log                    the journal of changes, newest first
  potok undo <number>          undo a change from the journal
  potok duplicates             look-alike operations — duplicate candidates
  potok duplicates keep <operation>   its group is not duplicates
  potok reconcile <account> [amount] [--on YYYY-MM-DD]   reconcile with the bank balance
  potok set <key> <value>      change a setting: language en|ru, statements_dir <path>, backup_dir <path>
  potok rate <currency> <rate> [--on date]   1 of it in the base currency (potok set base_currency EUR)
  potok rates [update|all]     rates for this ledger's currencies; update — fetch the ECB and the Bank of Russia
  potok backup                 make a backup now (one is made on its own once a day)
  potok backups                list the backups, newest first
  potok restore [file|latest]  put a backup in place of the ledger; the current one is backed up first
  potok cloud login [--email <address>] [--project <id>]  set up the encrypted copy in Google Drive
  potok cloud                  is it set up, when the last copy went, the copies there
  potok cloud push             send the newest backup now (otherwise once a day on its own)
  potok cloud get [name|latest] download and decrypt a copy into the backups; then potok restore <file>
  potok cloud take             write from this device: take the mark, take a newer copy (as starting potok does)
  potok cloud release          send what changed and let go of the mark (as leaving potok does)
  potok cloud logout           forget the sign-in and the passphrase on this Mac
  potok screens [WxH]          draw every screen as text (for the eyes)
  potok mcp                    MCP server over stdio for an agent, read-only
  potok migrate <dump> --user <id>   migrate a ledger from a saldo dump and reconcile it (§9)
  potok version                which potok this is, and the ledger it opens
  potok update [--check] [--pull]    build and install this potok again from its source

Flags:
  --db <path>        another database (default ~/.local/share/potok/ledger.db)
  --account <name>   which account a statement goes to (default — the account named after the bank)
  --currency <code>  currency of the account being created
  --kind <kind>      CASH | CARD | BANK | OTHER, default BANK
  --opening <amount> opening balance of the account being created
  --user <prefix>    whose ledger to migrate — the start of the user_id in saldo
  --on <date>        the day the bank balance is for (default today)
  --dry-run          only review and show, write nothing
  --version, -V      version and the commit it was built from
  --check            update: say what is installed and what the source has, build nothing
  --pull             update: fast-forward the source first (refuses on changes of your own)
";

fn usage() -> &'static str {
    match potok_core::i18n::lang() {
        potok_core::i18n::Lang::En => USAGE_EN,
        potok_core::i18n::Lang::Ru => USAGE_RU,
    }
}

const USAGE_RU: &str = "\
potok — локальный леджер

  potok                        открыть интерфейс
  potok account add <имя>      создать счёт
  potok account rename <имя> --into <новое> | archive <имя> | unarchive <имя> | opening <имя> <сумма>
  potok import <файл>          разобрать выписку и записать
  potok imports                что загружалось
  potok unimport <номер>       откатить загрузку целиком
  potok accounts               счета и их балансы
  potok add <счёт> <сумма> <кто> [--on дата] [--category что]
                               ввести операцию руками: 12.50 — расход, +12.50 — доход
  potok counterparties         кто встречается и на сколько
  potok categorize <кто> --category <что> [--from <старая>]   разметить контрагента
  potok counterparty merge <кто> --into <кто>   свести двух в одного
  potok counterparty rename <кто> --into <имя>   переименовать руками
  potok counterparty note <кто> [<текст>]   своя заметка; без текста — показать, --clear — снять
  potok counterparty refresh-names   пересобрать имена контрагентов по нынешним правилам
  potok alias add <текст> --into <кто> | remove <текст>   написание, ведущее к контрагенту
  potok debts                  долги и остатки
  potok debt open <кто> <i-owe|owed-to-me> [сумма] [--with операция] [--on дата] [--currency EUR]
                               открыть долг: суммой без операции, или с операции (--with)
  potok debt link <долг> <операция>   привязать операцию к долгу
  potok debt write-off <долг>  списать остаток
  potok debt due <долг> <дата|-> | note <долг> <текст|-> | delete <долг>
  potok flows [--from A] [--to B]  куда ушли деньги; без дат — этот учётный месяц
  potok verify                 проверить инварианты
  potok role <операция> <count|no-count|transit|refund>   как операция входит в потоки
  potok refunds [apply]        возвраты за покупки, которые ещё считаются доходом; apply — считать возвратами
  potok hints [apply]          контрагенты без категории, чьи операции почти все в одной; apply — разметить
  potok op <операция> remove | restore | unpair | pair <операция> | category <имя|-> | split \"Еда 30; Дом 12.50\"
  potok category rename <имя> --into <новое> | merge <имя> --into <другая> | delete <имя> | budget <имя> <сумма|->
  potok month <ГГГГ-ММ> <начало|->   поправить начало учётного месяца
  potok export [--from A] [--to B] [--account X]   операции в CSV на stdout
  potok log                    журнал правок, новые сверху
  potok undo <номер>           отменить правку из журнала
  potok duplicates             похожие операции — кандидаты в дубли
  potok duplicates keep <операция>   её группа — не дубли
  potok reconcile <счёт> [сумма] [--on ГГГГ-ММ-ДД]   сверить с остатком в банке
  potok set <ключ> <значение>  поменять настройку: language en|ru, statements_dir <путь>, backup_dir <путь>
  potok rate <валюта> <курс> [--on дата]   1 её в основной валюте (potok set base_currency EUR)
  potok rates [update|all]     курсы валют этого леджера; update — загрузить ЕЦБ и ЦБ РФ
  potok backup                 сделать бэкап сейчас (раз в сутки он делается сам)
  potok backups                бэкапы, новые сверху
  potok restore [файл|latest]  поставить бэкап на место леджера; нынешний сначала уходит в бэкап
  potok cloud login [--email <адрес>] [--project <id>]  настроить зашифрованную копию в Google Drive
  potok cloud                  настроено ли, когда ушла последняя копия, какие копии там
  potok cloud push             отправить свежий бэкап сейчас (иначе раз в сутки само)
  potok cloud get [имя|latest] скачать и расшифровать копию в бэкапы; дальше potok restore <файл>
  potok cloud take             писать с этого устройства: взять метку и свежую копию (как при запуске)
  potok cloud release          отправить изменения и снять метку (как при выходе)
  potok cloud logout           забыть вход и пароль на этом маке
  potok screens [ШxВ]          нарисовать все экраны в текст (для глаз)
  potok mcp                    MCP-сервер по stdio для агента, только чтение
  potok migrate <дамп> --user <id>   перенести леджер из дампа saldo и сверить (§9)
  potok version                какой это potok и какой леджер он открывает
  potok update [--check] [--pull]    собрать и поставить этот potok заново из исходников

Флаги:
  --db <путь>        другая база (по умолчанию ~/.local/share/potok/ledger.db)
  --account <имя>    на какой счёт загружать выписку (по умолчанию — счёт с именем банка)
  --currency <код>   валюта создаваемого счёта
  --kind <вид>       CASH | CARD | BANK | OTHER, по умолчанию BANK
  --opening <сумма>  начальный остаток создаваемого счёта
  --user <префикс>   чей леджер переносить — начало user_id в saldo
  --on <дата>        на какой день остаток в банке (по умолчанию сегодня)
  --dry-run          только разобрать и показать, ничего не записывать
  --version, -V      версия и коммит, из которого собрано
  --check            update: что стоит и что есть в исходниках, без сборки
  --pull             update: сначала подтянуть исходники (откажет, если есть свои правки)
";

struct Args {
    command: Option<String>,
    positional: Vec<String>,
    db: Option<PathBuf>,
    dry_run: bool,
    from: Option<String>,
    to: Option<String>,
    account: Option<String>,
    currency: Option<String>,
    kind: Option<String>,
    opening: Option<String>,
    category: Option<String>,
    into: Option<String>,
    user: Option<String>,
    on: Option<String>,
    with: Option<String>,
    /// `cloud login`: the Google account to sign in with.
    email: Option<String>,
    /// `cloud login`: with gcloud's sign-in, the project to count calls against.
    project: Option<String>,
    /// `update`: say what is installed and what the source has, build nothing.
    check: bool,
    /// `update`: fast-forward the source before building.
    pull: bool,
    /// `counterparty note`: clear the note.
    clear: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        command: None,
        positional: Vec::new(),
        db: None,
        dry_run: false,
        from: None,
        to: None,
        account: None,
        currency: None,
        kind: None,
        opening: None,
        category: None,
        into: None,
        user: None,
        on: None,
        with: None,
        email: None,
        project: None,
        check: false,
        pull: false,
        clear: false,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--db" => args.db = Some(PathBuf::from(raw.next().with_context(|| tr!("--db expects a path", "--db ждёт путь"))?)),
            "--from" => args.from = Some(raw.next().with_context(|| tr!("--from expects a date", "--from ждёт дату"))?),
            "--to" => args.to = Some(raw.next().with_context(|| tr!("--to expects a date", "--to ждёт дату"))?),
            "--account" => args.account = Some(raw.next().with_context(|| tr!("--account expects a name", "--account ждёт имя"))?),
            "--category" => args.category = Some(raw.next().with_context(|| tr!("--category expects a name", "--category ждёт название"))?),
            "--into" => args.into = Some(raw.next().with_context(|| tr!("--into expects a counterparty name", "--into ждёт имя контрагента"))?),
            "--user" => args.user = Some(raw.next().with_context(|| tr!("--user expects the start of a user_id", "--user ждёт начало user_id"))?),
            "--currency" => args.currency = Some(raw.next().with_context(|| tr!("--currency expects a code", "--currency ждёт код"))?),
            "--kind" => args.kind = Some(raw.next().with_context(|| tr!("--kind expects an account kind", "--kind ждёт вид счёта"))?),
            "--opening" => args.opening = Some(raw.next().with_context(|| tr!("--opening expects an amount", "--opening ждёт сумму"))?),
            "--with" => args.with = Some(raw.next().with_context(|| tr!("--with expects an operation number", "--with ждёт номер операции"))?),
            "--on" => args.on = Some(raw.next().with_context(|| tr!("--on expects a date YYYY-MM-DD", "--on ждёт дату ГГГГ-ММ-ДД"))?),
            "--project" => args.project = Some(raw.next().with_context(|| tr!("--project expects a Google Cloud project id", "--project ждёт id проекта Google Cloud"))?),
            "--email" => args.email = Some(raw.next().with_context(|| tr!("--email expects an address", "--email ждёт адрес"))?),
            "--dry-run" => args.dry_run = true,
            "--check" => args.check = true,
            "--pull" => args.pull = true,
            "--clear" => args.clear = true,
            "-V" | "--version" => {
                println!("{}", version_line());
                std::process::exit(0);
            }
            "-h" | "--help" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            // A bare «-» is a value (no category, no due date), and so is «-12.50».
            other if other.starts_with('-') && other.len() > 1 && !other.as_bytes()[1].is_ascii_digit() => anyhow::bail!(tr!("unknown flag {other}", "неизвестный флаг {other}")),
            other if args.command.is_none() => args.command = Some(other.to_string()),
            other => args.positional.push(other.to_string()),
        }
    }
    Ok(args)
}

fn main() {
    // `potok flows | head`: the reader went away, which is not a failure.
    // `println!` panics on a closed pipe; quietly stop instead.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = info.payload().downcast_ref::<String>().map(String::as_str).unwrap_or("");
        if text.contains("failed printing to stdout") && text.contains("Broken pipe") {
            std::process::exit(0);
        }
        default_hook(info);
    }));
    if let Err(error) = run() {
        eprintln!("potok: {error:#}");
        std::process::exit(1);
    }
}

/// `potok 0.2.0 (ab12cd3)` — the version from `Cargo.toml` and the commit it
/// was built from, `+` if potok had uncommitted changes then. Without git at
/// build time there is no hash and none is invented.
fn version_line() -> String {
    match option_env!("POTOK_COMMIT") {
        Some(commit) => format!("potok {} ({commit})", env!("CARGO_PKG_VERSION")),
        None => format!("potok {}", env!("CARGO_PKG_VERSION")),
    }
}

/// Where this binary was built from — the folder with `install.sh`.
fn source_dir() -> PathBuf {
    PathBuf::from(env!("POTOK_SRC"))
}

/// git in the source folder; `None` when it is not there or git says no.
fn git_says(args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .arg("-C")
        .arg(source_dir())
        .args(args)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// What this is, and what it opens. Nothing is created: a ledger that is not
/// there is said to be not there.
fn cmd_version(args: &Args) -> Result<()> {
    println!("{}", version_line());
    let path = db_path(args)?;
    let schema = if path.exists() {
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .and_then(|conn| conn.query_row("pragma user_version", [], |row| row.get::<_, i64>(0)))
            .map(|version| version.to_string())
            .unwrap_or_else(|error| format!("{error}"))
    } else {
        tr!("not there yet", "ещё нет")
    };
    say!("ledger schema {} · this potok writes {}", "схема леджера {} · этот potok пишет {}", schema, db::SCHEMA_VERSION);
    say!("ledger: {}", "леджер: {}", path.display());
    if let Ok(exe) = std::env::current_exe() {
        say!("binary: {}", "бинарник: {}", exe.display());
    }
    say!("built from: {}", "собран из: {}", source_dir().display());
    let changes = source_dir().join("CHANGELOG.md");
    if changes.exists() {
        say!("what is new: {}", "что нового: {}", changes.display());
    }
    Ok(())
}

/// Build and install this potok again from the source it was built from.
///
/// There is no release to download — potok is built on the Mac it runs on, so
/// «update» is `install.sh` in the folder the binary remembers. `--check`
/// only says whether the source has moved on; `--pull` fast-forwards it
/// first, and refuses if there are changes of your own there.
fn cmd_update(args: &Args) -> Result<()> {
    let src = source_dir();
    let script = src.join("install.sh");
    if !script.exists() {
        anyhow::bail!(tr!(
            "no source in {} — this binary was built somewhere else; build it from the repository",
            "в {} исходников нет — бинарник собран не здесь; собери из репозитория",
            src.display()
        ));
    }
    let head = git_says(&["rev-parse", "--short", "HEAD"]);
    let dirty = git_says(&["status", "--porcelain", "--", "."]).is_some_and(|out| !out.is_empty());
    let mine = option_env!("POTOK_COMMIT").map(|c| c.trim_end_matches('+').to_string());
    let same = matches!((&mine, &head), (Some(mine), Some(head)) if mine == head);

    if args.check {
        say!("installed: {}", "стоит: {}", version_line());
        match &head {
            Some(head) => say!("source:    {} {}", "исходники: {} {}", head, if dirty { tr!("(with changes of your own)", "(со своими правками)") } else { String::new() }),
            None => say!("source:    {} (no git)", "исходники: {} (без git)", src.display()),
        }
        if same && !dirty {
            say!("up to date", "свежий");
        } else {
            say!("potok update builds and installs it", "potok update соберёт и поставит");
        }
        return Ok(());
    }

    if args.pull {
        if dirty {
            anyhow::bail!(tr!(
                "there are changes of your own in {} — commit or put them aside first",
                "в {} есть свои правки — сначала закоммить или отложи их",
                src.display()
            ));
        }
        let pulled = std::process::Command::new("git").arg("-C").arg(&src).args(["pull", "--ff-only"]).status();
        match pulled {
            Ok(status) if status.success() => {}
            Ok(_) => anyhow::bail!(tr!("git pull did not go through", "git pull не прошёл")),
            Err(error) => anyhow::bail!(tr!("git did not start: {error}", "git не запустился: {error}")),
        }
    }

    say!("building {}{}", "собираю {}{}",
        git_says(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| src.display().to_string()),
        if dirty && !args.pull { tr!(" with the changes of your own in the source", " с твоими правками в исходниках") } else { String::new() });
    // install.sh prints what it installed, and cargo its own progress.
    let built = std::process::Command::new("sh").arg(&script).status();
    match built {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => anyhow::bail!(tr!("install.sh stopped: {status}", "install.sh остановился: {status}")),
        Err(error) => anyhow::bail!(tr!("install.sh did not start: {error}", "install.sh не запустился: {error}")),
    }
}

/// `potok cloud release` in a child of its own: send what changed and let go
/// of the mark, after this process is gone. Its own process group, so closing
/// the terminal window does not take it with it; what it did is in Settings
/// and on Home (`cloud_sent`, and `cloud_error` if it failed).
fn release_detached(args: &Args) {
    use std::process::{Command, Stdio};
    #[cfg(unix)]
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    let mut child = Command::new(exe);
    child.arg("cloud").arg("release");
    if let Some(db) = &args.db {
        child.arg("--db").arg(db);
    }
    child.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    child.process_group(0);
    let _ = child.spawn();
}

fn run() -> Result<()> {
    early_language();
    let args = parse_args()?;
    let Some(command) = args.command.as_deref() else {
        // With no arguments — the interface. That is the main path; the
        // subcommands are for a one-off action and for an agent.
        let path = db_path(&args)?;
        let conn = open(&args)?;
        // Google Drive is nowhere near the way in and out: the screen opens
        // on the ledger that is on this disk, the check lands in the status
        // line when Drive answers, and the copy goes out after the screen is
        // gone. Waiting on the network to open a local file is what this
        // replaced — two or three seconds every start, the timeout with no
        // network, and the same again on the way out.
        let cloud = cloud::start_in_background(&path);
        let writes = tui::run(conn, cloud)?;
        // The writer sends what changed and lets go of the mark — in a child
        // of its own, so leaving the screen is leaving.
        if writes && cloud::applies_to(&path) {
            release_detached(&args);
        }
        return Ok(());
    };

    match command {
        "import" => cmd_import(&args),
        "imports" => cmd_imports(&args),
        "unimport" => cmd_unimport(&args),
        "account" => cmd_account(&args),
        "accounts" => cmd_accounts(&args),
        "add" => cmd_add(&args),
        "counterparties" => cmd_counterparties(&args),
        "categorize" => cmd_categorize(&args),
        "counterparty" => cmd_counterparty(&args),
        "alias" => cmd_alias(&args),
        "debts" => cmd_debts(&args),
        "debt" => cmd_debt(&args),
        "flows" => cmd_flows(&args),
        "verify" => cmd_verify(&args),
        "log" => cmd_log(&args),
        "export" => cmd_export(&args),
        "rate" => cmd_rate(&args),
        "rates" => cmd_rates(&args),
        "backup" => cmd_backup(&args),
        "backups" => cmd_backups(&args),
        "restore" => cmd_restore(&args),
        "cloud" => cmd_cloud(&args),
        "set" => cmd_set(&args),
        "role" => cmd_role(&args),
        "refunds" => cmd_refunds(&args),
        "hints" => cmd_hints(&args),
        "op" => cmd_op(&args),
        "category" => cmd_category(&args),
        "month" => cmd_month(&args),
        "undo" => cmd_undo(&args),
        "duplicates" => cmd_duplicates(&args),
        "reconcile" => cmd_reconcile(&args),
        "screens" => cmd_screens(&args),
        "migrate" => cmd_migrate(&args),
        "version" => cmd_version(&args),
        "update" => cmd_update(&args),
        "mcp" => {
            // Read-only, and what holds that is the database
            // (`db::open_read_only`), not which tools are registered.
            let path = match &args.db {
                Some(path) => path.clone(),
                None => db::default_path()?,
            };
            mcp::run(db::open_read_only(&path)?)
        }
        other => anyhow::bail!(tr!("no subcommand {other:?}; `potok --help` shows what there is", "нет подкоманды {other:?}; `potok --help` покажет, что есть")),
    }
}

fn db_path(args: &Args) -> Result<std::path::PathBuf> {
    match &args.db {
        Some(path) => Ok(path.clone()),
        None => db::default_path(),
    }
}

/// Commands that may start a ledger where there is none: the interface (the
/// first run), creating an account, importing, migrating, and the cloud and
/// restore, which bring one onto a new machine. Any other on a missing file
/// is a mistyped `--db`, and used to create an empty ledger there, folders
/// and a backup included.
fn may_create(args: &Args) -> bool {
    match args.command.as_deref() {
        None | Some("import" | "migrate" | "cloud" | "restore") => true,
        Some("account") => args.positional.first().map(String::as_str) == Some("add"),
        _ => false,
    }
}

fn open(args: &Args) -> Result<rusqlite::Connection> {
    let path = db_path(args)?;
    if !path.exists() && !may_create(args) {
        anyhow::bail!(tr!(
            "there is no ledger at {} — check --db; `potok account add` or `potok import` starts one",
            "леджера нет: {} — проверь --db; `potok account add` или `potok import` начинают новый",
            path.display()
        ));
    }
    let conn = db::open(&path)?;
    settings::apply_language(&conn)?;
    // A copy a day, on whatever command comes first. A failed backup is told,
    // not fatal: refusing to open the ledger over it would be worse.
    if let Err(error) = potok_core::backup::dir(&conn, &path).and_then(|dir| potok_core::backup::make_if_due(&conn, &dir)) {
        eprintln!("{}", tr!("potok: no backup made: {error:#}", "potok: бэкап не сделан: {error:#}"));
    }
    Ok(conn)
}

fn cmd_cloud(args: &Args) -> Result<()> {
    let path = db_path(args)?;
    let login = args.positional.first().map(String::as_str) == Some("login");
    if !login && cloud::configured() && !cloud::applies_to(&path) {
        anyhow::bail!(tr!(
            "only the default ledger goes to Google Drive; {} is another one",
            "в Google Drive ходит только леджер по умолчанию; {} — другой",
            path.display()
        ));
    }
    // `take` may put a copy from Drive in place of the ledger: it runs
    // before this command holds the ledger open itself.
    if args.positional.first().map(String::as_str) == Some("take") {
        let start = cloud::start(&path, true);
        if let cloud::Start::Unreachable(why) = &start {
            anyhow::bail!("{why}");
        }
        say!("{}", "{}", cloud::start_note(&start).unwrap_or_else(|| tr!("this device writes", "пишет это устройство")));
        return Ok(());
    }
    let conn = db::open(&path)?;
    settings::apply_language(&conn)?;
    match args.positional.first().map(String::as_str) {
        Some("login") => cloud::login(&conn, &path, args.email.as_deref(), args.project.as_deref()),
        Some("logout") => {
            cloud::logout();
            say!("signed out; the copies stay in Google Drive", "вход забыт; копии остались в Google Drive");
            Ok(())
        }
        // The same as leaving the interface, for use without it.
        Some("release") => {
            match cloud::finish(&path)? {
                Some(sent) => say!("sent {sent}; the mark is released", "отправлено {sent}; метка снята"),
                None => say!("nothing new to send; the mark is released", "нового нет; метка снята"),
            }
            Ok(())
        }
        Some("push") => {
            let sent = cloud::push(&conn, &path)?;
            say!("sent: {sent}", "отправлено: {sent}");
            Ok(())
        }
        Some("get") => {
            let file = cloud::get(&conn, &path, args.positional.get(1).map(String::as_str))?;
            say!("decrypted into {} — potok restore {} puts it in place", "расшифровано в {} — potok restore {} поставит её на место", file.display(), file.display());
            Ok(())
        }
        None | Some("list") | Some("status") => {
            if !cloud::configured() {
                say!("Google Drive is not set up — potok cloud login", "Google Drive не настроен — potok cloud login");
                return Ok(());
            }
            let sent = settings::get(&conn, settings::CLOUD_SENT)?.unwrap_or_else(|| "—".into());
            say!("this device: {}", "это устройство: {}", cloud::device_name(&conn));
            say!("secrets live in: {}", "секреты лежат в: {}", secrets::where_they_live());
            say!("last copy sent: {sent}", "последняя копия ушла: {sent}");
            match cloud::writer(&conn)? {
                Some(mark) => say!("writes now: {} since {}", "сейчас пишет: {} с {}", mark.device, mark.since_local()),
                None => say!("writes now: nobody", "сейчас пишет: никто"),
            }
            if let Some(error) = settings::get(&conn, settings::CLOUD_ERROR)? {
                say!("the last attempt failed: {error}", "последняя попытка не удалась: {error}");
            }
            for r in cloud::list(&conn)? {
                // Drive answers in UTC; the list is read here.
                let when = chrono::DateTime::parse_from_rfc3339(&r.created)
                    .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|_| r.created.clone());
                println!("{when}  {:>9}  {}", format!("{} KB", r.size / 1024), r.name);
            }
            Ok(())
        }
        Some(other) => anyhow::bail!(tr!("potok cloud {other}? login, take, release, push, get, logout", "potok cloud {other}? login, take, release, push, get, logout")),
    }
}

/// The ledger as it is, with no copy of the day made on the way: for the
/// commands that make or choose a copy themselves. Opening through `open()`
/// first made the day's copy — so `restore` the morning after a mistake put
/// back the copy of the mistake, and `backup` made two.
fn open_without_backup(args: &Args) -> Result<rusqlite::Connection> {
    let path = db_path(args)?;
    if !path.exists() && !may_create(args) {
        anyhow::bail!(tr!("there is no ledger at {} — check --db", "леджера нет: {} — проверь --db", path.display()));
    }
    let conn = db::open(&path)?;
    settings::apply_language(&conn)?;
    Ok(conn)
}

fn cmd_backup(args: &Args) -> Result<()> {
    let conn = open_without_backup(args)?;
    let dir = potok_core::backup::dir(&conn, &db_path(args)?)?;
    let made = potok_core::backup::make(&conn, &dir)?;
    say!("backup: {}", "бэкап: {}", made.display());
    Ok(())
}

fn cmd_backups(args: &Args) -> Result<()> {
    let conn = open_without_backup(args)?;
    let dir = potok_core::backup::dir(&conn, &db_path(args)?)?;
    let found = potok_core::backup::list(&dir);
    if found.is_empty() {
        say!("no backups in {}", "в {} бэкапов нет", dir.display());
    }
    for (_, path) in found {
        println!("{}", path.display());
    }
    Ok(())
}

/// Put a backup in place of the ledger. The ledger as it is now is backed up
/// first, so a restore is itself undoable.
fn cmd_restore(args: &Args) -> Result<()> {
    let path = db_path(args)?;
    let conn = open_without_backup(args)?;
    let dir = potok_core::backup::dir(&conn, &path)?;
    let chosen = match args.positional.first().map(String::as_str) {
        None | Some("latest") => {
            potok_core::backup::list(&dir).into_iter().next().map(|(_, p)| p).with_context(|| {
                tr!("no backups in {}", "в {} бэкапов нет", dir.display())
            })?
        }
        Some(file) => potok_core::backup::expand(file),
    };
    let check = rusqlite::Connection::open_with_flags(&chosen, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|c| c.query_row("select count(*) from operation", [], |r| r.get::<_, i64>(0)))
        .with_context(|| tr!("{} is not a potok ledger", "{} — не леджер potok", chosen.display()))?;
    let safety = potok_core::backup::make(&conn, &dir)?;
    potok_core::backup::put_in_place(
        conn,
        &path,
        &chosen,
        &tr!("restored from {}", "восстановлено из {}", chosen.display()),
    )?;
    say!(
        "restored from {} ({check} operations); the ledger as it was is in {}",
        "восстановлено из {} ({check} операций); леджер, каким он был, — в {}",
        chosen.display(),
        safety.display()
    );
    Ok(())
}

/// The ledger's language before arguments are even read, so that `--help`
/// and a mistyped flag answer in it. A ledger that cannot be read yet — none
/// there, or an older version still to upgrade — leaves English.
fn early_language() {
    let mut raw = std::env::args().skip_while(|a| a != "--db");
    let path = match raw.nth(1) {
        Some(path) => PathBuf::from(path),
        None => match db::default_path() {
            Ok(path) => path,
            Err(_) => return,
        },
    };
    if let Ok(conn) = db::open_read_only(&path) {
        let _ = settings::apply_language(&conn);
    }
}

fn cmd_set(args: &Args) -> Result<()> {
    let [key, value] = args.positional.as_slice() else {
        anyhow::bail!(tr!(
            "potok set <key> <value>: language en|ru, statements_dir <path>",
            "potok set <ключ> <значение>: language en|ru, statements_dir <путь>"
        ));
    };
    let mut normalized = value.clone();
    match key.as_str() {
        settings::LANGUAGE => {
            anyhow::ensure!(
                potok_core::i18n::Lang::from_code(value).is_some(),
                tr!("language is en or ru, not «{value}»", "язык — en или ru, а не «{value}»")
            );
        }
        settings::STATEMENTS_DIR | settings::BACKUP_DIR | settings::DEVICE_NAME => {}
        settings::RATES_AUTO => {
            anyhow::ensure!(["on", "off"].contains(&value.as_str()), tr!("rates_auto is on or off", "rates_auto — on или off"));
        }
        settings::BASE_CURRENCY => {
            normalized = potok_core::Currency::new(value)?.as_str().to_string();
        }
        settings::THEME => {
            // The list of themes is one, in theme.rs: otherwise it falls out of
            // step with what the settings walk (and it did: «saldo» and «light»).
            let all: Vec<&str> = tui::theme::Theme::ALL.iter().map(|t| t.code()).collect();
            anyhow::ensure!(
                all.contains(&value.as_str()),
                tr!("theme is one of: {}", "тема — одна из: {}", all.join(", "))
            );
        }
        other => anyhow::bail!(tr!("no setting «{other}»", "нет настройки «{other}»")),
    }
    let conn = open(args)?;
    if normalized.trim().is_empty() {
        settings::clear(&conn, key)?;
    } else {
        settings::set(&conn, key, &normalized)?;
    }
    settings::apply_language(&conn)?;
    say!("{key} = {normalized}", "{key} = {normalized}");
    Ok(())
}

fn cmd_import(args: &Args) -> Result<()> {
    let path = args.positional.first().with_context(|| tr!("which file to review?", "какой файл разбирать?"))?;
    let path = PathBuf::from(path);
    let text = import::pdf::text_of(&path)?;
    let statement = import::parse(&text)?;

    say!(
        "{}: {} operations, currency {}", "{}: {} операций, валюта {}",
        statement.bank.as_str(),
        statement.rows.len(),
        statement.currency
    );
    if statement.pending_count() > 0 {
        say!(
            "  pending among them: {} — they will not enter the balance (invariant 6)", "  из них ожидающих: {} — в баланс не пойдут (инвариант 6)",
            statement.pending_count()
        );
    }

    // A check on the row, not only on the total: Revolut prints the balance
    // after every operation, and the chain has to agree (§7).
    let posted: Vec<_> = statement.posted().collect();
    let with_balance = posted.iter().filter(|r| r.running_balance.is_some()).count();
    if with_balance == posted.len() && !posted.is_empty() {
        // The opening balance from the summary is the chain's first link, or the
        // first row stays unchecked, and it is like all the others.
        let mut previous = import::revolut::declared_totals(&text).map(|t| t.opening.minor());
        let mut chained = 0usize;
        for row in &posted {
            let balance = row.running_balance.unwrap().minor();
            if let Some(prev) = previous {
                if balance - prev == row.amount.minor() {
                    chained += 1;
                }
            }
            previous = Some(balance);
        }
        let links = if import::revolut::declared_totals(&text).is_some() {
            posted.len()
        } else {
            posted.len() - 1
        };
        say!("  balance chain: {chained} of {links}", "  цепочка остатка: {chained} из {links}");
    } else {
        say!("  the bank prints no running balance — nothing to check the parse against", "  банк не печатает остаток в строке — проверить разбор изнутри нечем");
    }

    if let Some(totals) = import::revolut::declared_totals(&text) {
        let mut out = 0i64;
        let mut inn = 0i64;
        for row in &posted {
            if row.amount.minor() < 0 {
                out -= row.amount.minor();
            } else {
                inn += row.amount.minor();
            }
        }
        let say = |label: &str, got: i64, want: i64| {
            say!(
                "  {label}: {:.2} | statement {:.2} | {}", "  {label}: {:.2} | в выписке {:.2} | {}",
                got as f64 / 100.0,
                want as f64 / 100.0,
                if got == want { trs!("matches", "сошлось") } else { trs!("DOES NOT MATCH", "НЕ СОШЛОСЬ") }
            );
        };
        say(trs!("out", "ушло"), out, totals.out.minor());
        say(trs!("in", "пришло"), inn, totals.inn.minor());
        say(trs!("total", "итог"), totals.opening.minor() - out + inn, totals.closing.minor());
    }

    let mut conn = open(args)?;
    let account = match args.account.clone() {
        Some(name) => name,
        None => import::review::account_for(&conn, &statement)?.with_context(|| {
            let bank = match statement.bank.as_str() {
                "icard" => "iCard",
                "revolut" => "Revolut",
                other => other,
            };
            tr!(
                "no {bank} account in {} — create it: potok account add {bank} --currency {}; or import into another: --account <name>",
                "счёта {bank} в {} нет — создай: potok account add {bank} --currency {}; или загрузи в другой: --account <имя>",
                statement.currency,
                statement.currency
            )
        })?,
    };
    let today = chrono::Local::now().date_naive();
    let plan = import::review::plan(&conn, &account, &path, &statement, today)?;
    if plan.already_imported {
        say!("\nthis file is already imported — nothing written", "\nэтот файл уже загружен — ничего не записано");
        return Ok(());
    }
    print_plan(&plan);

    if args.dry_run {
        say!("\n--dry-run: nothing written", "\n--dry-run: ничего не записано");
        return Ok(());
    }

        // From the command line every hint is accepted: there is nobody here to
        // ask. A hint can be refused in the interface, and what is done can be
        // rolled back with `potok unimport`.
    let outcome = import::store::apply(
        &mut conn,
        &path,
        &statement,
        &plan,
        // The archive sits beside the database being written to: an import into
        // a copy must not put files next to the real one.
        &db::archive_dir(&db_path(args)?),
    )?;

    say!("\nwritten to account {account:?}:", "\nзаписано на счёт {account:?}:");
    say!("  import journal rows:     {}", "  строк в журнале импорта: {}", outcome.rows_recorded);
    say!("  operations created:      {}", "  создано операций:        {}", outcome.operations_created);
    if outcome.transfers_paired > 0 {
        say!("  of them transfers:       {} (paired with their other leg)", "  из них переводов:        {} (связаны со второй ногой)", outcome.transfers_paired);
    }
    if outcome.debt_movements > 0 {
        say!("  of them debt payments:   {}", "  из них по долгам:        {}", outcome.debt_movements);
    }
    if outcome.duplicates_skipped > 0 {
        say!(
            "  skipped as duplicates:   {} (same signature from another file)", "  пропущено как дубли:     {} (та же подпись из другого файла)",
            outcome.duplicates_skipped
        );
    }
    if outcome.pending_skipped > 0 {
        say!(
            "  pending not written:     {} — they will come completed in the next statement", "  ожидающих не записано:   {} — придут завершёнными в следующей выписке",
            outcome.pending_skipped
        );
    }
    if outcome.declined > 0 {
        say!("  declined by choice:      {}", "  не записано по решению:  {}", outcome.declined);
    }
    say!("  source in the archive:   {}", "  исходник в архиве:       {}", outcome.archived_at.display());
    say!("  roll back: potok unimport {}", "  откатить: potok unimport {}", outcome.file_id);
    Ok(())
}

/// What the import is about to do: the hints and marks row by row, the rest as
/// a number.
fn print_plan(plan: &import::review::Plan) {
    use import::review::{Flag, Hint, RowStatus};
    say!(
        "\nto account {:?}: new {}, duplicates {}, pending {}", "\nна счёт {:?}: новых {}, дублей {}, ожидающих {}",
        plan.account,
        plan.count(RowStatus::New),
        plan.count(RowStatus::Duplicate),
        plan.count(RowStatus::Pending)
    );
    for row in plan.rows.iter().filter(|r| r.status == RowStatus::New) {
        let mut notes: Vec<String> = Vec::new();
        match &row.hint {
            Some(Hint::Transfer { with }) => notes.push(tr!("transfer ↔ {} #{}", "перевод ↔ {} #{}", with.account, with.id)),
            Some(Hint::Debt { counterparty, kind, .. }) => notes.push(tr!(
                "debt {counterparty}: {}", "долг {counterparty}: {}",
                if *kind == "REPAYMENT" { trs!("repayment", "возврат") } else { trs!("borrowed more", "взял ещё") }
            )),
            None => {}
        }
        for flag in &row.flags {
            notes.push(
                match flag {
                    Flag::Future => trs!("future date — not written", "дата из будущего — не пишется"),
                    Flag::Large => trs!("larger than usual", "крупнее обычного"),
                    Flag::Person => trs!("money between people — a debt?", "деньги между людьми — может, долг?"),
                    Flag::ByHand => trs!("already entered by hand — not written", "уже введена руками — не пишется"),
                }
                .to_string(),
            );
        }
        if !notes.is_empty() {
            println!("  {} {:>14}  {}  ← {}", row.occurred_on, row.amount.to_string(), row.description, notes.join("; "));
        }
    }
}

fn cmd_imports(args: &Args) -> Result<()> {
    let conn = open(args)?;
    for file in query::import_journal(&conn)? {
        say!(
            "#{:<4} {:<16} {:<10} rows {:<5} operations {:<5} {}", "#{:<4} {:<16} {:<10} строк {:<5} операций {:<5} {}",
            file.id, file.imported_at, file.bank, file.rows, file.operations, file.filename
        );
    }
    Ok(())
}

fn cmd_unimport(args: &Args) -> Result<()> {
    let id: i64 = args
        .positional
        .first()
        .with_context(|| tr!("which import to roll back? the number is in potok imports", "какую загрузку откатить? номер — в potok imports"))?
        .parse()
        .with_context(|| tr!("an import number is a number", "номер загрузки — число"))?;
    let mut conn = open(args)?;
    let done = import::store::rollback(&mut conn, id)?;
    say!(
        "rolled back «{}»: {} operations, {} transfers split, {} debt movements", "откачено «{}»: операций {}, переводов разорвано {}, движений долга {}",
        done.filename, done.operations, done.partners_unpaired, done.debt_movements
    );
    Ok(())
}

fn cmd_account(args: &Args) -> Result<()> {
    match args.positional.first().map(String::as_str) {
        Some("add") => {}
        Some(verb @ ("rename" | "archive" | "unarchive")) => {
            let name = args.positional.get(1).with_context(|| tr!("which account?", "какой счёт?"))?;
            let conn = open(args)?;
            let id: i64 = conn
                .query_row("select id from account where name = ?1", [name], |r| r.get(0))
                .with_context(|| tr!("no account «{name}»", "нет счёта «{name}»"))?;
            match verb {
                "rename" => {
                    let to = args.into.as_deref().with_context(|| tr!("--into expects the new name", "--into ждёт новое имя"))?;
                    potok_core::account::rename(&conn, id, to)?;
                }
                "archive" => potok_core::account::set_archived(&conn, id, true)?,
                _ => potok_core::account::set_archived(&conn, id, false)?,
            }
            say!("done", "готово");
            return Ok(());
        }
        Some("opening") => {
            let (Some(name), Some(amount)) = (args.positional.get(1), args.positional.get(2)) else {
                anyhow::bail!(tr!("potok account opening <account> <amount>", "potok account opening <счёт> <сумма>"));
            };
            let conn = open(args)?;
            let id = account_id(&conn, name)?;
            potok_core::edit::set_opening(&conn, id, reconcile::parse_input(amount)?)?;
            say!("opening balance of «{name}»: {amount}", "начальный остаток «{name}»: {amount}");
            return Ok(());
        }
        _ => anyhow::bail!(tr!(
            "potok account add | rename | archive | unarchive | opening — see potok --help",
            "potok account add | rename | archive | unarchive | opening — см. potok --help"
        )),
    }
    let name = args.positional.get(1).with_context(|| tr!("what to call the account?", "как назвать счёт?"))?;
    let currency = args.currency.as_deref().with_context(|| tr!("--currency expects a code like EUR", "--currency ждёт код вида EUR"))?;
    let currency = potok_core::Currency::new(currency)?;
    let kind = args.kind.as_deref().unwrap_or("BANK");
    let opening = match args.opening.as_deref() {
        Some(text) => potok_core::Money::parse(text, currency)?.minor(),
        None => 0,
    };
    let conn = open(args)?;
    let id = potok_core::account::create(&conn, name, currency.as_str(), kind, opening)?;
    say!(
        "account {name:?} created (#{id}), {}, opening balance {}", "счёт {name:?} создан (#{id}), {}, начальный остаток {}",
        currency,
        potok_core::Money::new(opening, currency)
    );
    Ok(())
}

fn cmd_accounts(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let rows = query::balances(&conn)?;
    if rows.is_empty() {
        say!("no accounts yet", "счетов пока нет");
    }
    // The name column as wide as the longest name: a long one pushed its
    // balance out of line with the rest.
    let w = rows.iter().map(|r| r.name.chars().count()).max().unwrap_or(0).max(24);
    for row in &rows {
        println!("{:<w$} {:>16}", row.name, row.balance.to_string());
    }
    if !rows.is_empty() {
        let total = potok_core::rate::total(&conn, &chrono::Local::now().date_naive().to_string())?;
        let missing: Vec<String> = total.missing.iter().map(|c| c.to_string()).collect();
        if missing.is_empty() {
            println!("{:<w$} {:>16}", tr!("total ≈", "итого ≈"), total.total.to_string());
        } else {
            say!("total ≈ {} without {} — no rate: potok rate <currency> <rate>", "итого ≈ {} без {} — нет курса: potok rate <валюта> <курс>", total.total, missing.join(", "));
        }
    }
    Ok(())
}

fn cmd_debts(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let rows = query::debts(&conn, false)?;
    if rows.is_empty() {
        say!("no open debts", "открытых долгов нет");
    }
    for row in rows {
        let who = if row.direction == "OWED_TO_ME" { trs!("owes me", "мне должен") } else { trs!("I owe", "я должен") };
        say!(
            "#{:<4} {:<20} {:<12} {:>16} of {}", "#{:<4} {:<20} {:<12} {:>16} из {}",
            row.id, row.counterparty, who, row.remaining.to_string(), row.issued
        );
    }
    Ok(())
}

fn account_id(conn: &rusqlite::Connection, name: &str) -> Result<i64> {
    query::balances(conn)?
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(name))
        .map(|a| a.id)
        .with_context(|| tr!("no account «{name}»", "нет счёта «{name}»"))
}

fn cmd_add(args: &Args) -> Result<()> {
    let [account, amount, who @ ..] = args.positional.as_slice() else {
        anyhow::bail!(tr!("potok add <account> <amount> <who>, for example: potok add Cash 12.50 Bakery", "potok add <счёт> <сумма> <кто>, например: potok add Cash 12.50 Пекарня"));
    };
    anyhow::ensure!(!who.is_empty(), tr!("who or what for? potok add {account} {amount} <who>", "кто или за что? potok add {account} {amount} <кто>"));
    // With no sign — a spend: what is entered by hand is most often cash spent.
    // Income takes an explicit plus.
    let (income, digits) = match amount.strip_prefix('+') {
        Some(rest) => (true, rest),
        None => (false, amount.as_str()),
    };
    let minor = reconcile::parse_input(digits)?.abs();
    let mut conn = open(args)?;
    let account_id = account_id(&conn, account)?;
    let today = chrono::Local::now().date_naive();
    let new = potok_core::edit::NewOperation {
        account_id,
        occurred_on: potok_core::edit::parse_date(args.on.as_deref().unwrap_or(""), today)?,
        effect_minor: if income { minor } else { -minor },
        who: who.join(" "),
        category: args.category.clone(),
    };
    let id = potok_core::edit::add_operation(&mut conn, &new)?;
    let line = query::operation_by_id(&conn, id)?.with_context(|| tr!("the operation just written cannot be read", "только что записанная операция не читается"))?;
    println!(
        "#{} {} {} {} {}",
        line.id,
        line.occurred_on,
        line.account,
        line.amount,
        line.counterparty.as_deref().unwrap_or(&line.description)
    );
    if let Some(category) = &line.category {
        say!("  category: {category}", "  категория: {category}");
    }
    Ok(())
}

fn cmd_role(args: &Args) -> Result<()> {
    use potok_core::edit::Counting;
    let [id, how] = args.positional.as_slice() else {
        anyhow::bail!(tr!("potok role <operation> <count|no-count|transit|refund>", "potok role <операция> <считать|не-считать|транзит|возврат>"));
    };
    let id: i64 = id.trim_start_matches('#').parse().with_context(|| tr!("an operation number is a number", "номер операции — число"))?;
    let counting = match how.as_str() {
        "count" | "считать" => Counting::Counted,
        "no-count" | "не-считать" => Counting::Excluded,
        "transit" | "транзит" => Counting::Passthrough,
        "refund" | "возврат" => Counting::Refund,
        other => anyhow::bail!(tr!("«{other}» — use count, no-count, transit or refund", "«{other}» — нужно считать, не-считать, транзит или возврат")),
    };
    let mut conn = open(args)?;
    potok_core::edit::set_counting(&mut conn, id, counting)?;
    let line = query::operation_by_id(&conn, id)?.with_context(|| tr!("the operation cannot be read", "операция не читается"))?;
    say!("#{} {} {} — now {}", "#{} {} {} — теперь {}", line.id, line.occurred_on, line.amount, line.role);
    Ok(())
}

fn operation_number(text: Option<&String>) -> Result<i64> {
    text.with_context(|| tr!("which operation? its number is in the interface and in potok export", "какая операция? номер — в интерфейсе и в potok export"))?
        .trim_start_matches('#')
        .parse()
        .with_context(|| tr!("an operation number is a number", "номер операции — число"))
}

/// What the operation menu does, as a command.
fn cmd_op(args: &Args) -> Result<()> {
    let id = operation_number(args.positional.first())?;
    let mut conn = open(args)?;
    use potok_core::edit;
    match args.positional.get(1).map(String::as_str) {
        Some("remove") => {
            let removed = edit::remove_operation(&mut conn, id)?;
            say!(
                "operations removed: {}, debt movements no longer counted: {} · back: potok op {id} restore",
                "убрано операций: {}, движений долга больше не считается: {} · вернуть: potok op {id} restore",
                removed.operations,
                removed.debt_movements
            );
        }
        Some("note") => {
            // With no text — show it, as for a counterparty; --clear removes it.
            if args.positional.len() < 3 && !args.clear {
                match potok_core::detail::operation(&conn, id)?.and_then(|op| op.note) {
                    Some(note) => println!("#{id}: {note}"),
                    None => say!("#{id}: no note · op {id} note <text> writes one", "#{id}: заметки нет · op {id} note <текст> напишет"),
                }
                return Ok(());
            }
            let note = args.positional[2..].join(" ");
            edit::set_note(&conn, id, &note)?;
            if note.trim().is_empty() {
                say!("note on #{id} removed", "заметка с #{id} снята");
            } else {
                say!("#{id}: {note}", "#{id}: {note}");
            }
        }
        Some("restore") => {
            let back = edit::restore_operation(&mut conn, id)?;
            say!("operations restored: {back}", "возвращено операций: {back}");
        }
        Some("unpair") => {
            edit::unpair_transfer(&mut conn, id)?;
            say!("#{id}: no longer a transfer", "#{id}: больше не перевод");
        }
        Some("pair") => {
            let other = operation_number(args.positional.get(2))?;
            edit::pair_transfer(&mut conn, id, other)?;
            say!("#{id} and #{other}: a transfer", "#{id} и #{other}: перевод");
        }
        Some("category") => {
            let name = args.positional.get(2).with_context(|| tr!("which category? «-» takes the counterparty's", "какая категория? «-» — как у контрагента"))?;
            edit::set_own_category(&conn, id, (name != "-").then_some(name.as_str()))?;
            say!("#{id}: category {name}", "#{id}: категория {name}");
        }
        Some("split") => {
            let text = args.positional.get(2).with_context(|| tr!("parts: \"Food 30; Home 12.50\" — one without an amount takes the rest; «-» — no split", "части: \"Еда 30; Дом 12.50\" — одна без суммы берёт остаток; «-» — без разбивки"))?;
            let line = query::operation_by_id(&conn, id)?.with_context(|| tr!("no operation #{id}", "нет операции #{id}"))?;
            let parts = if text == "-" { Vec::new() } else { edit::parse_split(text, line.amount.minor().abs())? };
            edit::split(&mut conn, id, &parts)?;
            say!("#{id}: split in {}", "#{id}: частей {}", parts.len());
        }
        _ => anyhow::bail!(tr!(
            "potok op <number> note [<text>] | remove | restore | unpair | pair <number> | category <name|-> | split \"<parts>\"|-",
            "potok op <номер> note [<текст>] | remove | restore | unpair | pair <номер> | category <имя|-> | split \"<части>\"|-"
        )),
    }
    Ok(())
}

/// The Categories screen, as a command.
fn cmd_category(args: &Args) -> Result<()> {
    let mut conn = open(args)?;
    let name = args.positional.get(1).with_context(|| tr!("which category?", "какая категория?"))?;
    match args.positional.first().map(String::as_str) {
        Some("rename") => potok_core::category::rename(&conn, name, args.into.as_deref().with_context(|| tr!("--into expects the new name", "--into ждёт новое имя"))?)?,
        Some("merge") => {
            let moved = potok_core::category::merge(&mut conn, name, args.into.as_deref().with_context(|| tr!("--into expects the category to merge into", "--into ждёт, в какую свести"))?)?;
            say!("moved: {moved}", "перенесено: {moved}");
        }
        Some("delete") => potok_core::category::delete(&mut conn, name)?,
        Some("budget") => {
            let limit = match args.positional.get(2).map(String::as_str) {
                None | Some("-") => None,
                Some(amount) => Some(reconcile::parse_input(amount)?.abs()),
            };
            potok_core::budget::set(&conn, name, limit)?;
        }
        _ => anyhow::bail!(tr!(
            "potok category rename <name> --into <new> | merge <name> --into <other> | delete <name> | budget <name> <amount|->",
            "potok category rename <имя> --into <новое> | merge <имя> --into <другая> | delete <имя> | budget <имя> <сумма|->"
        )),
    }
    say!("done", "готово");
    Ok(())
}

/// Correct when an accounting month starts, or go back to the rule.
fn cmd_month(args: &Args) -> Result<()> {
    let [month, start] = args.positional.as_slice() else {
        anyhow::bail!(tr!("potok month <YYYY-MM> <start date|->", "potok month <ГГГГ-ММ> <дата начала|->"));
    };
    let first = chrono::NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
        .with_context(|| tr!("a month is YYYY-MM", "месяц — ГГГГ-ММ"))?;
    let conn = open(args)?;
    let start = match start.as_str() {
        "-" => None,
        text => Some(potok_core::edit::parse_date(text, chrono::Local::now().date_naive())?),
    };
    use chrono::Datelike;
    potok_core::period::set_start(&conn, (first.year(), first.month()), start)?;
    say!("done", "готово");
    Ok(())
}

/// Money back from purchases that still counts as income: list them, or with
/// `apply` count them as refunds — each takes its shop's spending down.
fn cmd_refunds(args: &Args) -> Result<()> {
    let mut conn = open(args)?;
    if args.positional.first().map(String::as_str) == Some("apply") {
        let (counted, moved) = potok_core::counterparty::take_refunds(&mut conn)?;
        say!(
            "refunds now: {counted}; moved to their shop: {moved} · each can be undone: potok log, potok undo",
            "теперь возвратами: {counted}; перенесены к своему магазину: {moved} · каждую можно отменить: potok log, potok undo"
        );
        return Ok(());
    }
    let found = potok_core::counterparty::refunds_counted_as_income(&conn)?;
    if found.is_empty() {
        say!("no refund counted as income", "возвратов, посчитанных доходом, нет");
        return Ok(());
    }
    for (id, _, shop) in &found {
        let line = query::operation_by_id(&conn, *id)?.with_context(|| tr!("no operation #{id}", "нет операции #{id}"))?;
        let shop = match shop {
            Some(s) => conn.query_row("select name from counterparty where id = ?1", [s], |r| r.get::<_, String>(0))?,
            None => tr!("shop not known", "магазин не опознан"),
        };
        let what: String = line.description.chars().take(60).collect();
        println!("#{:<6} {} {:>14}  {what}  → {shop}", line.id, line.occurred_on, line.amount.to_string());
    }
    say!("\n{} · potok refunds apply — count them as refunds", "\n{} · potok refunds apply — считать возвратами", found.len());
    Ok(())
}

/// Counterparties with no category almost all of whose operations are in one:
/// the same marking as `x` on the import screen, but as a list and before writing.
fn cmd_hints(args: &Args) -> Result<()> {
    const SURE: usize = 80;
    let mut conn = open(args)?;
    if args.positional.first().map(String::as_str) == Some("apply") {
        let taken = potok_core::counterparty::take_confident_hints(&mut conn, SURE)?;
        say!(
            "marked: {taken} · each can be undone: potok log, potok undo",
            "размечено: {taken} · каждую можно отменить: potok log, potok undo"
        );
        return Ok(());
    }
    let found = potok_core::counterparty::confident_hints(&conn, SURE)?;
    if found.is_empty() {
        say!("nobody whose operations agree that well", "никого, у кого операции сходятся настолько");
        return Ok(());
    }
    for (who, category, in_it, total) in &found {
        let percent = in_it * 100 / total.max(&1);
        let who: String = who.chars().take(34).collect();
        println!("{who:<34} → {category:<22} {in_it}/{total} ({percent}%)");
    }
    say!(
        "\n{} · potok hints apply — put them in that category",
        "\n{} · potok hints apply — поставить им эту категорию",
        found.len()
    );
    Ok(())
}

fn cmd_rate(args: &Args) -> Result<()> {
    let [code, rate] = args.positional.as_slice() else {
        anyhow::bail!(tr!("potok rate <currency> <rate> [--on date] — 1 of it in the base currency", "potok rate <валюта> <курс> [--on дата] — 1 её в основной валюте"));
    };
    let conn = open(args)?;
    let base = settings::base_currency(&conn)?;
    let from = potok_core::Currency::new(&code.to_uppercase())?;
    // A code no account uses and no feed quotes is most likely a typo
    // (`potok rate XYZ 2` used to be taken).
    let known: bool = query::balances(&conn)?.iter().any(|a| a.balance.currency() == from)
        || potok_core::rate::rate(&conn, from, base, &chrono::Local::now().date_naive().to_string())?.is_some();
    if !known {
        anyhow::bail!(tr!("no account is in {from} and no rate mentions it — a typo?", "ни один счёт не в {from}, и курсов к ней нет — опечатка?"));
    }
    let rate = potok_core::rate::parse(rate)?;
    let on = potok_core::edit::parse_date(args.on.as_deref().unwrap_or(""), chrono::Local::now().date_naive())?;
    potok_core::rate::set(&conn, from, base, rate, &on.to_string())?;
    say!("1 {from} = {} {base} from {on}", "1 {from} = {} {base} с {on}", potok_core::rate::show(rate));
    Ok(())
}

fn cmd_rates(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let today = chrono::Local::now().date_naive().to_string();
    match args.positional.first().map(String::as_str) {
        Some("update") => {
            let done = potok_core::rate::update(&conn)?;
            say!("rates stored: {}", "курсов сохранено: {}", done.stored);
            for failed in &done.failed {
                say!("  not fetched: {failed}", "  не загружено: {failed}");
            }
        }
        Some("all") => {
            for (from, to, rate, since) in potok_core::rate::latest(&conn)? {
                println!("{since}  1 {from} = {} {to}", potok_core::rate::show(rate));
            }
        }
        _ => {
            for row in potok_core::rate::table(&conn, &today)? {
                match row.rate {
                    Some(rate) => println!("1 {} = {} {}", row.from, potok_core::rate::show(rate), row.to),
                    None => say!("1 {} = ? {} — no rate: potok rates update", "1 {} = ? {} — нет курса: potok rates update", row.from, row.to),
                }
            }
        }
    }
    Ok(())
}

/// Operations as CSV on stdout: `potok export --from 2026-09-01 > september.csv`.
fn cmd_export(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let found = query::search_operations(
        &conn,
        &query::OperationFilter {
            account: args.account.clone(),
            from: args.from.clone(),
            to: args.to.clone(),
            limit: usize::MAX,
            ..Default::default()
        },
    )?;
    print!("{}", potok_core::export::csv(&found.operations));
    Ok(())
}

fn cmd_log(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let entries = potok_core::journal::entries(&conn, 50)?;
    if entries.is_empty() {
        say!("no changes yet", "правок ещё не было");
    }
    for entry in entries {
        let mark = match (&entry.undone_at, entry.can_undo) {
            (Some(_), _) => trs!("undone", "отменена"),
            (None, true) => "",
            (None, false) => trs!("no undo", "без отмены"),
        };
        println!("#{:<5} {}  {}  {mark}", entry.id, entry.at, entry.what);
    }
    Ok(())
}

fn cmd_undo(args: &Args) -> Result<()> {
    let id: i64 = args
        .positional
        .first()
        .with_context(|| tr!("which change to undo? the number is in potok log", "какую правку отменить? номер — в potok log"))?
        .trim_start_matches('#')
        .parse()
        .with_context(|| tr!("a change number is a number", "номер правки — число"))?;
    let mut conn = open(args)?;
    println!("{}", potok_core::journal::undo(&mut conn, id)?);
    Ok(())
}

fn cmd_duplicates(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let only = args.account.as_deref().map(|name| account_id(&conn, name)).transpose()?;
    let groups = reconcile::duplicates(&conn, only)?;
    if args.positional.first().map(String::as_str) == Some("keep") {
        let id = operation_number(args.positional.get(1))?;
        let group = groups
            .iter()
            .find(|g| g.operations.iter().any(|o| o.id == id))
            .with_context(|| tr!("#{id} is in no look-alike group", "#{id} нет ни в одной группе похожих"))?;
        reconcile::dismiss_duplicates(&conn, &group.key)?;
        say!("not duplicates: {}", "не дубли: {}", group.key);
        return Ok(());
    }
    if groups.is_empty() {
        say!("no look-alike operations", "похожих операций нет");
        return Ok(());
    }
    for group in &groups {
        let first = &group.operations[0];
        println!(
            "{} {:<10} {:>14} ×{}  {}",
            first.occurred_on,
            first.account,
            first.amount.to_string(),
            group.operations.len(),
            first.counterparty.as_deref().unwrap_or(&first.description)
        );
        for op in &group.operations {
            println!("    #{:<6} {:<14} {}", op.id, op.role, op.description);
        }
    }
    say!("\ngroups: {}", "\nгрупп: {}", groups.len());
    say!(
        "remove one: potok op <number> remove · not duplicates: potok duplicates keep <number>",
        "убрать: potok op <номер> remove · не дубли: potok duplicates keep <номер>"
    );
    Ok(())
}

fn cmd_reconcile(args: &Args) -> Result<()> {
    let name = args.positional.first().with_context(|| tr!("which account to reconcile? potok reconcile <account> <amount>", "какой счёт сверять? potok reconcile <счёт> <сумма>"))?;
    let conn = open(args)?;
    let id = account_id(&conn, name)?;
    let found = match args.positional.get(1) {
        Some(amount) => {
            let on = args.on.clone().unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
            Some(reconcile::reconcile(&conn, id, &on, reconcile::parse_input(amount)?)?)
        }
        None => reconcile::last(&conn, id)?,
    };
    let Some(r) = found else {
        say!("«{name}» was never reconciled. potok reconcile {name} <bank balance>", "«{name}» ещё не сверялся. potok reconcile {name} <остаток в банке>");
        return Ok(());
    };
    say!("{} on {}: the bank says {}, here {}", "{} на {}: в банке {}, здесь {}", r.account, r.on_date, r.bank, r.ledger);
    if r.matches() {
        say!("matches", "сходится");
        return Ok(());
    }
    say!("off by {} ({})", "расходится на {} ({})", r.difference, if r.difference.minor() > 0 { trs!("more here", "здесь больше") } else { trs!("less here", "здесь меньше") });
    if r.duplicates_extra.minor() != 0 {
        say!(
            "without look-alike operations the balance would be {} — potok duplicates --account {name}", "без похожих операций баланс был бы {} — potok duplicates --account {name}",
            r.without_duplicates()
        );
    }
    for op in &r.same_amount {
        say!("  same amount: #{} {} {} {}", "  та же сумма: #{} {} {} {}", op.id, op.occurred_on, op.amount, op.description);
    }
    Ok(())
}

fn cmd_debt(args: &Args) -> Result<()> {
    let number = |at: usize, what: &str| -> Result<i64> {
        args.positional
            .get(at)
            .with_context(|| tr!("which {what}? the number is in potok debts or in the journal", "какой {what}? номер — в potok debts или в журнале"))?
            .trim_start_matches('#')
            .parse()
            .with_context(|| tr!("a {what} number is a number", "номер {what} — число"))
    };
    let mut conn = open(args)?;
    let today = chrono::Local::now().date_naive();
    match args.positional.first().map(String::as_str) {
        Some("open") => {
            let who = args.positional.get(1).with_context(|| tr!("who is the debt with?", "с кем долг?"))?;
            let direction = match args.positional.get(2).map(|d| d.to_lowercase()).as_deref() {
                Some("я-должен" | "i-owe") => "I_OWE",
                Some("мне-должны" | "owed-to-me") => "OWED_TO_ME",
                _ => anyhow::bail!(tr!("direction: i-owe or owed-to-me", "кто кому: я-должен или мне-должны")),
            };
            let opening = match args.positional.get(3) {
                Some(amount) => reconcile::parse_input(amount)?.abs(),
                None => 0,
            };
            let new = potok_core::debt::NewDebt {
                who: who.clone(),
                direction: direction.into(),
                currency: args.currency.clone().unwrap_or_else(|| "EUR".into()).to_uppercase(),
                opened_on: potok_core::edit::parse_date(args.on.as_deref().unwrap_or(""), today)?,
                due_on: None,
                opening_minor: opening,
                note: None,
            };
            let with = args
                .with
                .as_deref()
                .map(|n| n.trim_start_matches('#').parse::<i64>().with_context(|| tr!("--with is an operation number", "--with — номер операции")))
                .transpose()?;
            let id = potok_core::debt::open(&mut conn, &new, with)?;
            say!("debt #{id} opened: {who}", "открыт долг #{id}: {who}");
        }
        Some("link") => {
            let (debt, operation) = (number(1, trs!("debt", "долг"))?, number(2, trs!("operation", "операция"))?);
            let kind = potok_core::debt::link(&mut conn, debt, operation)?;
            say!("operation #{operation} linked to debt #{debt} as {}", "операция #{operation} привязана к долгу #{debt} как {}", if kind == "ISSUE" { trs!("issue", "выдача") } else { trs!("repayment", "возврат") });
        }
        Some("write-off") => {
            let debt = number(1, trs!("debt", "долг"))?;
            let minor = potok_core::debt::write_off(&conn, debt, today)?;
            say!("wrote off {} on debt #{debt} — settled", "списано {} по долгу #{debt} — закрыт", potok_core::money::format_minor(minor));
        }
        Some(what @ ("due" | "note")) => {
            let debt = number(1, trs!("debt", "долг"))?;
            let mut fields = potok_core::debt::fields(&conn, debt)?;
            let value = args.positional.get(2).filter(|v| v.as_str() != "-").cloned();
            if what == "due" {
                fields.due_on = value.map(|v| potok_core::edit::parse_date(&v, today).map(|d| d.to_string())).transpose()?;
            } else {
                fields.note = value;
            }
            potok_core::debt::update(&conn, debt, &fields)?;
            say!("debt #{debt} changed", "долг #{debt} изменён");
        }
        Some("delete") => {
            let debt = number(1, trs!("debt", "долг"))?;
            potok_core::debt::delete(&mut conn, debt)?;
            say!("debt #{debt} deleted; its operations stay", "долг #{debt} удалён; операции остались");
        }
        _ => anyhow::bail!(tr!(
            "potok debt open | link | write-off | due | note | delete — see potok --help",
            "potok debt open | link | write-off | due | note | delete — см. potok --help"
        )),
    }
    Ok(())
}

fn cmd_flows(args: &Args) -> Result<()> {
    let conn = open(args)?;
    // No dates: this accounting month, as the screens and the agent see it.
    let today = chrono::Local::now().date_naive();
    let (this_from, this_to) = tui::app::Period::ThisMonth.bounds_on(today, &potok_core::period::MonthRule::load(&conn)?);
    let date = |given: Option<&str>, default: String| -> Result<String> {
        Ok(match given {
            Some(text) => potok_core::edit::parse_date(text, today)?.to_string(),
            None => default,
        })
    };
    let from = date(args.from.as_deref(), this_from)?;
    let to = date(args.to.as_deref(), this_to)?;
    let (from, to) = (from.as_str(), to.as_str());
    say!("{from} — {to}", "{from} — {to}");
    let rows = query::flows(&conn, from, to)?;
    let (all_out, all_in) = query::flow_totals(&rows);
    if rows.is_empty() {
        say!("no posted operations for {from}..{to}", "за {from}..{to} проведённых операций нет");
    }
    for row in rows {
        let out: Vec<String> =
            row.out.in_display_order(None).iter().map(|m| m.to_string()).collect();
        let inn: Vec<String> =
            row.inn.in_display_order(None).iter().map(|m| m.to_string()).collect();
        // A bank transfer's description holds an IBAN and a whole phrase, and it
        // does not fit the column. Clipped by characters rather than bytes:
        // Cyrillic and the «ﬁ» from a PDF would otherwise tear in the middle.
        let label: String = match row.label.chars().count() > 40 {
            true => row.label.chars().take(39).chain("…".chars()).collect(),
            false => row.label.clone(),
        };
        say!(
            "{:<40} {:>18} {:>18}  ({} pcs)", "{:<40} {:>18} {:>18}  ({} шт)",
            label,
            out.join(" / "),
            inn.join(" / "),
            row.count
        );
    }
    let bag = |b: &potok_core::MoneyBag| b.in_display_order(None).iter().map(|m| m.to_string()).collect::<Vec<_>>().join(" / ");
    say!("{:<40} {:>18} {:>18}", "{:<40} {:>18} {:>18}", tr!("total", "итого"), bag(&all_out), bag(&all_in));
    Ok(())
}

fn cmd_verify(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let report = verify::run(&conn)?;
    for label in &report.checked {
        say!("checked: {label}", "проверено: {label}");
    }
    if report.ok() {
        say!("\nmatches", "\nсходится");
        return Ok(());
    }
    println!();
    for finding in &report.findings {
        say!("invariant {}: {}", "инвариант {}: {}", finding.invariant, finding.what);
    }
    std::process::exit(1);
}

fn cmd_counterparties(args: &Args) -> Result<()> {
    let conn = open(args)?;
    let rows = query::counterparties(&conn)?;
    if rows.is_empty() {
        say!("no counterparties yet — they appear on import", "контрагентов пока нет — они появляются при импорте");
        return Ok(());
    }
    let unmarked = rows.iter().filter(|r| r.category.is_none()).count();
    for row in &rows {
        let spent: Vec<String> =
            row.spent.in_display_order(None).iter().map(|m| m.to_string()).collect();
        let name: String = match row.name.chars().count() > 34 {
            true => row.name.chars().take(33).chain("…".chars()).collect(),
            false => row.name.clone(),
        };
        say!(
            "{:<34} {:>16}  {:>4} pcs  {}", "{:<34} {:>16}  {:>4} шт  {}",
            name,
            spent.join(" / "),
            row.operations,
            row.category.as_deref().unwrap_or("—")
        );
    }
    if unmarked > 0 {
        say!(
            "\nuncategorised: {unmarked} of {}. Categorise: potok categorize <who> --category <what>", "\nбез категории: {unmarked} из {}. Разметить: potok categorize <кто> --category <что>",
            rows.len()
        );
    }
    Ok(())
}

fn cmd_categorize(args: &Args) -> Result<()> {
    let who = args.positional.first().with_context(|| tr!("whom to categorise?", "кого разметить?"))?;
    let category = args.category.as_deref().with_context(|| tr!("--category expects a name", "--category ждёт название"))?;
    let conn = open(args)?;
    // `--from Other`: pull its operations out of the old category, or the
    // counterparty's rule will not reach them — an own category wins.
    let (affected, moved) = potok_core::counterparty::take_over(&conn, who, category, args.from.as_deref())?;
    println!("{who:?} → {category:?}");
    say!(
        "  operations now in the category: {affected} (and all future ones, nothing rewritten)", "  операций встало в категорию: {affected} (и все будущие, без переписывания)"
    );
    if moved > 0 {
        say!("  taken out of {}: {moved}", "  вынуто из {}: {moved}", args.from.as_deref().unwrap_or("—"));
    }
    Ok(())
}

fn cmd_counterparty(args: &Args) -> Result<()> {
    match args.positional.first().map(String::as_str) {
        Some("merge") => {}
        Some("note") => {
            let who = args.positional.get(1).with_context(|| tr!("whose note?", "чья заметка?"))?;
            let conn = open(args)?;
            // With no text — show it rather than erase it: `--clear` clears it.
            // Caught by itself: «potok counterparty note NORD» erased it.
            if args.positional.len() < 3 && !args.clear {
                let note = potok_core::detail::counterparty(&conn, who)?.and_then(|c| c.note);
                match note {
                    Some(note) => println!("{who:?}: {note}"),
                    None => say!("{who:?}: no note · note <who> <text> writes one", "{who:?}: заметки нет · note <кто> <текст> напишет"),
                }
                return Ok(());
            }
            let note = if args.clear { String::new() } else { args.positional[2..].join(" ") };
            potok_core::counterparty::set_note(&conn, who, &note)?;
            if note.trim().is_empty() {
                say!("note on {who:?} removed", "заметка с {who:?} снята");
            } else {
                say!("{who:?}: {note}", "{who:?}: {note}");
            }
            return Ok(());
        }
        Some("rename") => {
            let from = args.positional.get(1).with_context(|| tr!("whom to rename?", "кого переименовать?"))?;
            let to = args.into.as_deref().with_context(|| tr!("--into expects the new name", "--into ждёт новое имя"))?;
            let conn = open(args)?;
            potok_core::counterparty::rename(&conn, from, to)?;
            say!("{from:?} is now {to:?}; refresh-names will not touch this name", "{from:?} теперь {to:?}; refresh-names это имя не тронет");
            return Ok(());
        }
        Some("refresh-names") => {
            let mut conn = open(args)?;
            let refreshed = potok_core::counterparty::refresh_names(&mut conn)?;
            say!("names rebuilt: {}", "имён пересобрано: {}", refreshed.renamed);
            for (now, wanted) in &refreshed.taken {
                say!("  kept {now:?}: the name {wanted:?} belongs to another. Merge: potok counterparty merge {now:?} --into {wanted:?}", "  оставлено {now:?}: имя {wanted:?} уже у другого. Свести: potok counterparty merge {now:?} --into {wanted:?}");
            }
            return Ok(());
        }
        _ => anyhow::bail!(tr!(
            "there is `potok counterparty merge <who> --into <who>`, `rename <who> --into <name>`, `note <who> [<text>]` and `refresh-names`",
            "есть `potok counterparty merge <кто> --into <кто>`, `rename <кто> --into <имя>`, `note <кто> [<текст>]` и `refresh-names`"
        )),
    }
    let from = args.positional.get(1).with_context(|| tr!("whom to merge?", "кого сводить?"))?;
    let into = args.into.as_deref().with_context(|| tr!("--into expects a counterparty name", "--into ждёт имя контрагента"))?;
    let mut conn = open(args)?;
    let moved = potok_core::counterparty::merge(&mut conn, from, into)?;
    say!("{from:?} merged into {into:?}, operations moved: {moved}", "{from:?} сведён в {into:?}, перенесено операций: {moved}");
    Ok(())
}

fn cmd_alias(args: &Args) -> Result<()> {
    let verb = args.positional.first().map(String::as_str);
    let text = args.positional.get(1).with_context(|| tr!("which spelling?", "какое написание?"))?;
    let conn = open(args)?;
    match verb {
        Some("add") => {
            let to = args.into.as_deref().with_context(|| tr!("--into expects a counterparty name", "--into ждёт имя контрагента"))?;
            let alias = potok_core::counterparty::set_alias(&conn, text, to)?;
            say!("spelling {alias:?} → {to:?}", "написание {alias:?} → {to:?}");
            say!(
                "  the next statement that writes it that way lands on {to:?}",
                "  следующая выписка с таким написанием попадёт на {to:?}"
            );
        }
        Some("remove") => {
            potok_core::counterparty::drop_alias(&conn, text)?;
            say!("spelling {text:?} removed", "написание {text:?} убрано");
        }
        _ => anyhow::bail!(tr!(
            "potok alias add <text> --into <who> | remove <text>",
            "potok alias add <текст> --into <кто> | remove <текст>"
        )),
    }
    Ok(())
}
fn cmd_screens(args: &Args) -> Result<()> {
    let (width, height) = match args.positional.first() {
        Some(size) => {
            let (w, h) = size.split_once(['x', 'X', '*']).with_context(|| tr!("a size like 150x44", "размер вида 150x44"))?;
            (w.trim().parse()?, h.trim().parse()?)
        }
        None => (150, 44),
    };
    print!("{}", tui::screens(open(args)?, width, height)?);
    Ok(())
}

fn cmd_migrate(args: &Args) -> Result<()> {
    let path = PathBuf::from(args.positional.first().with_context(|| tr!("which dump to migrate?", "какой дамп переносить?"))?);
    let user = args
        .user
        .as_deref()
        .with_context(|| tr!("whose ledger? --user <start of user_id>. saldo has several, guessing is not allowed", "чей леджер? --user <начало user_id>. В saldo их несколько, угадывать нельзя"))?;
    let dump = std::fs::read_to_string(&path)
        .with_context(|| tr!("cannot read {}", "не читается {}", path.display()))?;
    let sha = import::store::sha256_of(&path)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(trs!("dump", "дамп")).to_string();

    let mut conn = if args.dry_run { db::open_memory()? } else { open(args)? };
    let report = potok_core::migrate::run(&mut conn, &dump, &name, &sha, user)?;
    let money = |minor: i64, currency: &str| {
        potok_core::Money::new(minor, potok_core::Currency::new(currency).expect(trs!("a currency from the database", "валюта из базы")))
            .to_string()
    };

    say!("migration from saldo, user {}…", "перенос из saldo, пользователь {}…", report.user_id.get(..8).unwrap_or(&report.user_id));
    say!("  operations migrated:   {}", "  операций перенесено:   {}", report.operations);
    say!(
        "  reversals not migrated: {} ({} are import rollbacks; saldo balances leave them out)", "  откатов не перенесено: {} ({} — откаты импортов; в балансы saldo они не входят)",
        report.reversed_skipped, report.reversed_from_rollbacks
    );
    say!("  transfer legs in pairs: {}", "  ног переводов в парах: {}", report.pairs);
    say!("  debts: {}, their movements: {}", "  долгов: {}, движений по ним: {}", report.debts.len(), report.movements);
    say!(
        "  categories: {}; counterparties: {}, fully categorised: {}; operations with their own category: {}", "  категорий: {}; контрагентов: {}, из них с категорией целиком: {}; операций со своей категорией: {}",
        report.categories,
        report.counterparties,
        report.counterparties_categorized,
        report.own_categories
    );

    let verdict = |potok: i64, stored: i64, recomputed: i64| match (potok == stored, stored == recomputed) {
        (true, _) => trs!("matches", "сходится").to_string(),
        (false, false) => trs!("off — but saldo itself is off against its own operations", "расходится — но и сам saldo расходится со своими операциями").to_string(),
        (false, true) => trs!("DOES NOT MATCH", "РАСХОДИТСЯ").to_string(),
    };
    say!("\n§9 reconciliation — accounts:", "\nсверка §9 — счета:");
    for a in &report.accounts {
        say!(
            "  {:<12} saldo stores {:>16}   potok {:>16}   {}", "  {:<12} saldo хранит {:>16}   potok {:>16}   {}",
            a.name,
            money(a.saldo_stored, &a.currency),
            money(a.potok, &a.currency),
            verdict(a.potok, a.saldo_stored, a.saldo_recomputed)
        );
    }
    say!("\n§9 reconciliation — debts:", "\nсверка §9 — долги:");
    for d in &report.debts {
        say!(
            "  {:<24} saldo stores {:>14}   potok {:>14}   {}", "  {:<24} saldo хранит {:>14}   potok {:>14}   {}",
            d.counterparty,
            money(d.saldo_stored, &d.currency),
            money(d.potok, &d.currency),
            verdict(d.potok, d.saldo_stored, d.saldo_recomputed)
        );
    }
    say!(
        "\npotok invariants: {}", "\nинварианты potok: {}",
        if report.findings.is_empty() { trs!("matches", "сходится").to_string() } else { tr!("{} findings", "{} находок", report.findings.len()) }
    );
    for f in &report.findings {
        say!("  invariant {}: {}", "  инвариант {}: {}", f.invariant, f.what);
    }

    if report.agrees() {
        say!("\n§9 met: every balance and every remainder matched saldo.", "\n§9 выполнен: каждый баланс и каждый остаток совпали с saldo.");
        if args.dry_run {
            say!("--dry-run: nothing written", "--dry-run: ничего не записано");
        }
        Ok(())
    } else {
        say!("\n§9 NOT met.", "\n§9 НЕ выполнен.");
        std::process::exit(1)
    }
}
