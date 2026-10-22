//! Synthetic statements with a known answer.
//!
//! There are two real statements on the one machine this runs on, and real
//! ones never enter the repository. These are generated in the shape
//! `pdf-extract` gives the real ones — headings, page breaks, pending rows,
//! records broken across lines — with what is awkward in them on purpose:
//! a balance below zero, thousands, a number at the end of a description,
//! a status other than «Approved», money moved between two balances of one
//! card. Every one is parsed, written to a ledger and checked against what the
//! generator knows it said. Added 2026-10-20, after these found five ways the
//! parsers lost or invented an operation.
#![cfg(test)]

use chrono::{Duration, NaiveDate};
use rusqlite::Connection;

use crate::{db, import, query, Currency, Money};

/// A small deterministic generator: the same seed, the same statement.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// What a generated statement said: its completed rows (day, signed minor),
/// its pending ones, and what each side came to.
#[derive(Debug, Default)]
struct Truth {
    posted: Vec<(NaiveDate, i64)>,
    pending: usize,
    opening: i64,
    closing: i64,
    out: i64,
    inn: i64,
    /// Rows moved inside the account — counted in neither side of the flows.
    internal: usize,
}

/// `1234.56` as Revolut prints it: `€1,234.56`, `-€54.49` below zero.
fn euro(minor: i64) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let abs = minor.abs();
    let whole = (abs / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{sign}€{grouped}.{:02}", abs % 100)
}

/// `-1600.00` as iCard prints it, with thousands when it feels like it.
fn icard_amount(minor: i64, thousands: bool) -> String {
    let sign = if minor < 0 { "-" } else { "" };
    let abs = minor.abs();
    let whole = (abs / 100).to_string();
    let whole = if thousands && whole.len() > 3 { format!("{},{}", &whole[..whole.len() - 3], &whole[whole.len() - 3..]) } else { whole };
    format!("{sign}{whole}.{:02}", abs % 100)
}

const SHOPS: [&str; 8] = ["Kaufland", "BILLA", "Lidl", "Ikea", "Apple", "Urban Mobility Center", "Еконт", "Soﬁa Pharmacy"];

fn revolut(seed: u64, rows: usize) -> (String, Truth) {
    let mut rng = Rng(seed);
    let mut truth = Truth { opening: 10_000 + rng.below(50_000) as i64, ..Truth::default() };
    let mut body = String::new();
    let mut balance = truth.opening;
    let mut day = NaiveDate::from_ymd_opt(2025, 12, 20).unwrap();
    for i in 0..rows {
        if i > 0 && i % 9 == 0 {
            // A page break: the footer, the next page's heading.
            body.push_str(&format!("Page {} of 9\n\nDate Description Money out Money in Balance\n", i / 9 + 1));
        }
        day += Duration::days(rng.below(3) as i64);
        let income = rng.chance(20);
        let size = if rng.chance(10) { 100_000 + rng.below(500_000) as i64 } else { 50 + rng.below(8_000) as i64 };
        // Spending that takes the balance below zero is allowed: it happened.
        let amount = if income { size } else { -size };
        balance += amount;
        let shop = SHOPS[rng.below(SHOPS.len() as u64) as usize];
        // Some money in is a shop giving money back: a refund, which takes
        // spending down instead of counting as income.
        let refund = income && rng.chance(30);
        let mut description = if refund { format!("Provisional refund for {shop} claim") } else { shop.to_string() };
        if !refund && rng.chance(15) {
            description.push_str(&format!(" {}.{:02}", rng.below(90) + 1, rng.below(100)));
        }
        body.push_str(&format!("{} {description} {} {}\n", day.format("%b %-d, %Y"), euro(size), euro(balance)));
        if rng.chance(60) {
            body.push_str(&format!("To: {description}, Sofia\nCard: 516794******7890\n"));
        }
        if rng.chance(10) {
            body.push_str("Revolut Rate €1.00 = 55.41 TRY (ECB rate* €1.00 = 55.38 TRY) 359.99 TRY\n");
        }
        truth.posted.push((day, amount));
        if amount < 0 {
            truth.out += -amount;
        } else if refund {
            truth.out -= amount;
        } else {
            truth.inn += amount;
        }
    }
    truth.closing = balance;
    let mut pending = String::new();
    for _ in 0..1 + rng.below(4) {
        let what = if rng.chance(50) { format!("Top up {}.00", rng.below(90) + 10) } else { "Contabo".to_string() };
        pending.push_str(&format!("{} {what} {}\n\nTo: somebody\n", day.format("%b %-d, %Y"), euro(100 + rng.below(9_000) as i64)));
        truth.pending += 1;
    }
    let text = format!(
        "\n\nEUR Statement\n\n Generated on {}\nRevolut Bank UAB\n\n© 2026 Revolut Bank UAB\n Page   of 1 9\n\nBalance summary\n\n\
         Product Opening balance Money out Money in Closing \nbalance\n\n\
         Account (Current Account) {} {} {} {}\n\nTotal {} {} {} {}\n\n\
         Pending from December 20, 2025 to {}\n\nStart date Description Money out Money in\n\n{pending}\n\
         Transactions from December 20, 2025 to {}\n\nDate Description Money out Money in Balance\n\n{body}",
        day.format("%b %-d, %Y"),
        euro(truth.opening), euro(truth.out), euro(truth.inn), euro(truth.closing),
        euro(truth.opening), euro(truth.out), euro(truth.inn), euro(truth.closing),
        day.format("%B %-d, %Y"),
        day.format("%B %-d, %Y"),
    );
    (text, truth)
}

fn icard(seed: u64, rows: usize) -> (String, Truth) {
    let mut rng = Rng(seed);
    let mut truth = Truth::default();
    let mut body = String::new();
    let mut day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
    let mut i = 0;
    while i < rows {
        day -= Duration::days(rng.below(2) as i64);
        let time = format!("{:02}:{:02}", rng.below(24), rng.below(60));
        // Both halves of money moved between the card's two balances.
        if rng.chance(8) {
            let size = 1_000 + rng.below(90_000) as i64;
            for (sign, words) in [(1, "to 91017809780 / EUR from\n20177318399"), (-1, "from 20687666782 / EUR to\n91017809780")] {
                body.push_str(&format!(
                    "\n{}\n{time} {} Balance\ntransfer Transfer {words} / EUR balance {} EUR Approved\n",
                    day.format("%d.%m.%Y"),
                    day.format("%d.%m.%Y"),
                    icard_amount(sign * size, false)
                ));
                truth.posted.push((day, sign * size));
                truth.internal += 1;
            }
            i += 2;
            continue;
        }
        let income = rng.chance(15);
        let size = if rng.chance(10) { 100_000 + rng.below(500_000) as i64 } else { 50 + rng.below(8_000) as i64 };
        let amount = if income { size } else { -size };
        let status = match rng.below(20) {
            0 => "Declined",
            1 => "Pending",
            2 => "Reversed",
            _ => "Approved",
        };
        let shop = SHOPS[rng.below(SHOPS.len() as u64) as usize].to_uppercase();
        let thousands = rng.chance(50);
        let printed = icard_amount(amount, thousands);
        // The record breaks wherever the PDF wrapped it.
        let refund = income && rng.chance(30);
        let kind = if refund { "POS Payment Reversal of*1234," } else { "POS Payment *1234," };
        let words = format!("{kind} {shop} SOFIA BGR, {} EUR {printed} EUR {status}", icard_amount(amount, false));
        let tokens: Vec<&str> = words.split(' ').collect();
        let mut record = format!("{time} {} ", day.format("%d.%m.%Y"));
        for (at, token) in tokens.iter().enumerate() {
            record.push_str(token);
            record.push(if at + 1 < tokens.len() && rng.chance(12) { '\n' } else { ' ' });
        }
        body.push_str(&format!("\n{}\n{}\n", day.format("%d.%m.%Y"), record.trim_end()));
        if i > 0 && i % 7 == 0 {
            body.push_str("\n© iCard AD, www.icard.com\nДата и час Вальор Тип Описание Сума Валута Статус\n");
        }
        if status == "Approved" {
            truth.posted.push((day, amount));
            if amount < 0 {
                truth.out += -amount;
            } else if refund {
                truth.out -= amount;
            } else {
                truth.inn += amount;
            }
        } else {
            truth.pending += 1;
        }
        i += 1;
    }
    truth.closing = truth.posted.iter().map(|(_, a)| a).sum();
    let text = format!(
        "\n© iCard AD, www.icard.com\nАйкарт АД е водещо дружество\nИстория на трансакции\n\nИзбран период: 01.09.2026 - 16.10.2026\n\n\
         Дата и час Вальор Тип Описание Сума Валута Статус\n{body}"
    );
    (text, truth)
}

fn posted(statement: &import::Statement) -> Vec<(NaiveDate, i64)> {
    statement.posted().map(|r| (r.occurred_on, r.amount.minor())).collect()
}

/// Write a statement to a ledger with one account, as `potok import` does.
fn import_into(text: &str, bank: &str, opening: i64, label: &str) -> (Connection, import::Statement) {
    let dir = std::env::temp_dir().join(format!("potok-mock-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("statement.txt");
    std::fs::write(&path, text).unwrap();
    let mut conn = db::open_memory().unwrap();
    import::store::create_account(&conn, bank, Currency::new("EUR").unwrap(), "BANK", opening).unwrap();
    let statement = import::parse(text).unwrap();
    let plan = import::review::plan(&conn, bank, &path, &statement, NaiveDate::MAX).unwrap();
    import::store::apply(&mut conn, &path, &statement, &plan, &dir.join("archive")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (conn, statement)
}

/// Every completed row read with its day and its sign, no pending one taken
/// for money, on a hundred statements each with something awkward in it.
#[test]
fn a_hundred_revolut_statements_are_read_row_for_row() {
    for seed in 1..=100 {
        let (text, truth) = revolut(seed, 40);
        let statement = import::parse(&text).unwrap_or_else(|e| panic!("seed {seed}: {e:#}\n{text}"));
        assert_eq!(posted(&statement), truth.posted, "seed {seed}");
        assert_eq!(statement.pending_count(), truth.pending, "seed {seed}: pending");
        let totals = import::revolut::declared_totals(&text).unwrap();
        assert_eq!((totals.opening.minor(), totals.closing.minor()), (truth.opening, truth.closing), "seed {seed}");
    }
}

#[test]
fn a_hundred_icard_statements_are_read_row_for_row() {
    for seed in 1..=100 {
        let (text, truth) = icard(seed, 40);
        let statement = import::parse(&text).unwrap_or_else(|e| panic!("seed {seed}: {e:#}\n{text}"));
        assert_eq!(posted(&statement), truth.posted, "seed {seed}");
        assert_eq!(statement.pending_count(), truth.pending, "seed {seed}: not approved");
    }
}

/// Written to a ledger: the balance is the statement's closing one, the
/// month's flows are what it says came and went — the moves between the
/// card's balances in neither, refunds off the spending rather than income.
#[test]
fn written_statements_balance_to_the_cent_and_flow_what_they_say() {
    for seed in [3, 17, 42, 99] {
        let (text, truth) = revolut(seed, 60);
        let (conn, _) = import_into(&text, "Revolut", truth.opening, &format!("rev{seed}"));
        let balance = query::balances(&conn).unwrap()[0].balance.minor();
        assert_eq!(balance, truth.closing, "seed {seed}: balance");
        let (out, inn) = query::flow_totals(&query::flows(&conn, "2000-01-01", "2100-01-01").unwrap());
        let minor = |bag: &crate::MoneyBag| bag.in_display_order(None).iter().map(Money::minor).sum::<i64>();
        assert_eq!((minor(&out), minor(&inn)), (truth.out, truth.inn), "seed {seed}: flows");

        let (text, truth) = icard(seed, 60);
        let (conn, _) = import_into(&text, "iCard", 0, &format!("icard{seed}"));
        let balance = query::balances(&conn).unwrap()[0].balance.minor();
        assert_eq!(balance, truth.closing, "seed {seed}: balance");
        let (out, inn) = query::flow_totals(&query::flows(&conn, "2000-01-01", "2100-01-01").unwrap());
        assert_eq!((minor(&out), minor(&inn)), (truth.out, truth.inn), "seed {seed}: flows without the internal moves");
        let transit: i64 = conn.query_row("select count(*) from operation where role = 'passthrough'", [], |r| r.get(0)).unwrap();
        assert_eq!(transit as usize, truth.internal, "seed {seed}");
    }
}

/// What cannot be read stops the import and says where — it does not leave a
/// hole in the ledger, and it does not invent money.
#[test]
fn what_cannot_be_read_is_refused_with_its_line() {
    let revolut_head = "EUR Statement\nRevolut Bank UAB\n Account (Current Account) €100.00 €0.00 €0.00 €100.00\n";
    // A completed row whose balance is missing: it used to pass for pending.
    let broken = format!("{revolut_head}Date Description Money out Money in Balance\nSep 4, 2026 Shop €10.00\n");
    let error = import::parse(&broken).unwrap_err().to_string();
    assert!(error.contains("line 5") || error.contains("строка 5"), "{error}");

    let icard_head = "© iCard AD\nДата и час Вальор Тип Описание Сума Валута Статус\n";
    let unknown_status = format!(
        "{icard_head}10:00 02.10.2026 POS Payment *1234, SHOP, -5.00 EUR -5.00 EUR Approved\n\
         10:00 03.10.2026 POS Payment *1234, SHOP, -6.00 EUR -6.00 EUR Settled\n\
         10:00 04.10.2026 POS Payment *1234, SHOP, -7.00 EUR -7.00 EUR Approved\n"
    );
    let error = import::parse(&unknown_status).unwrap_err().to_string();
    assert!(error.contains("-6.00") && error.contains("Settled"), "{error}");

    let two_currencies = format!(
        "{icard_head}10:00 02.10.2026 POS Payment *1234, SHOP, -5.00 EUR -5.00 EUR Approved\n\
         10:00 03.10.2026 POS Payment *1234, SHOP BG, -9.78 BGN -9.78 BGN Approved\n"
    );
    let error = import::parse(&two_currencies).unwrap_err().to_string();
    assert!(error.contains("BGN"), "{error}");
}

/// A pending row whose description ends in a number is still pending, and a
/// completed one whose description does is still read.
#[test]
fn a_number_in_a_description_is_not_an_amount() {
    let text = "EUR Statement\nRevolut Bank UAB\n Account (Current Account) €100.00 €12.50 €0.00 €87.50\n\
                Pending from September 1, 2026 to October 16, 2026\nStart date Description Money out Money in\n\
                Oct 16, 2026 Top up 50.00 €50.00\n\
                Date Description Money out Money in Balance\nSep 5, 2026 Order 12.50 €12.50 €87.50\n";
    let statement = import::parse(text).unwrap();
    assert_eq!(statement.pending_count(), 1);
    assert_eq!(posted(&statement), vec![(NaiveDate::from_ymd_opt(2026, 9, 5).unwrap(), -1250)]);
    assert_eq!(statement.rows.iter().find(|r| r.posted).unwrap().description, "Order 12.50");
}

/// A refund is counted against the shop it came from: by the menu, undone
/// back, and for the old rows all at once — «Reversal of YELLOW» moves to
/// YELLOW and takes YELLOW's category's spending down.
#[test]
fn a_refund_takes_its_shops_spending_down() {
    use crate::edit::{set_counting, Counting};
    assert_eq!(crate::counterparty::refund_of("Reversal of YELLOW! +359291119 BGR").as_deref(), Some("YELLOW! +359291119 BGR"));
    assert_eq!(crate::counterparty::refund_of("Provisional refund for Mango Bulgaria Eood claim From: X").as_deref(), Some("Mango Bulgaria Eood"));
    assert_eq!(crate::counterparty::refund_of("Transfer from MILA"), None, "money from a person is not a refund");

    let mut conn = db::open_memory().unwrap();
    conn.execute_batch(
        "insert into account (name,currency,kind) values ('Card','EUR','CARD');
         insert into category (name) values ('Taxi');
         insert into counterparty (name, category_id) values ('YELLOW', 1), ('Reversal of YELLOW', null);
         insert into counterparty_alias (alias, counterparty_id) values ('yellow', 1), ('reversal of yellow', 2);
         insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id) values
           (1,1,'expense','2026-09-01T00:00:00Z',2000,'EUR',-2000,'YELLOW! +359291119 BGR',1),
           (2,1,'income','2026-09-02T00:00:00Z',500,'EUR',500,'Reversal of YELLOW! +359291119 BGR',2),
           (4,1,'income','2026-10-02T00:00:00Z',100,'EUR',100,'Reversal of YELLOW! +359291119 BGR',2),
           (3,1,'income','2026-09-03T00:00:00Z',9000,'EUR',9000,'Transfer from MILA',null);",
    )
    .unwrap();
    let taxi = |conn: &Connection| {
        let rows = query::flows(conn, "2026-09-01", "2026-09-30").unwrap();
        let row = |label: &str| rows.iter().find(|f| f.label == label).map(|f| (f.out.in_display_order(None).iter().map(Money::minor).sum::<i64>(), f.inn.in_display_order(None).iter().map(Money::minor).sum::<i64>()));
        (row("Taxi"), row("Reversal of YELLOW"))
    };
    assert_eq!(taxi(&conn), (Some((2000, 0)), Some((0, 500))), "before: the refund is income of its own");

    assert_eq!(crate::counterparty::refunds_counted_as_income(&conn).unwrap(), vec![(2, Some(2), Some(1)), (4, Some(2), Some(1))]);
    // Two of one spelling: moving the first takes the second with it.
    assert_eq!(crate::counterparty::take_refunds(&mut conn).unwrap(), (2, 1));
    assert!(crate::counterparty::refunds_counted_as_income(&conn).unwrap().is_empty());
    assert_eq!(taxi(&conn), (Some((1500, 0)), None), "after: Taxi spent 15, nothing earned");
    let (out, inn) = query::flow_totals(&query::flows(&conn, "2026-09-01", "2026-09-30").unwrap());
    assert_eq!(out.in_display_order(None).iter().map(Money::minor).sum::<i64>(), 1500);
    assert_eq!(inn.in_display_order(None).iter().map(Money::minor).sum::<i64>(), 9000, "MILA's money stays income");
    let balance = query::balances(&conn).unwrap()[0].balance.minor();
    assert_eq!(balance, -2000 + 500 + 100 + 9000, "the balance never moves");

    // The menu both ways, and the journal undoes it.
    set_counting(&mut conn, 2, Counting::Counted).unwrap();
    assert_eq!(taxi(&conn).0, Some((2000, 500)), "income again — now under the shop it came from");
    let last = crate::journal::last_id(&conn).unwrap();
    crate::journal::undo(&mut conn, last).unwrap();
    assert_eq!(taxi(&conn).0, Some((1500, 0)), "undone back to a refund");
    assert!(set_counting(&mut conn, 1, Counting::Refund).is_err(), "money that went out is not a refund");
    let ops = query::operations(&conn, None, false).unwrap();
    let spent = query::spent_by_day(&ops);
    assert_eq!(spent.get("2026-09-02").map(|b| b.in_display_order(None).iter().map(Money::minor).sum::<i64>()), Some(-500), "a refund's day costs less");
}
