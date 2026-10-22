//! A Revolut statement.
//!
//! In the extracted text a completed operation is one line:
//!
//! ```text
//! Sep 1, 2026 Ikea €23.67 €344.25
//! ```
//!
//! the date, the description, the amount, the balance after the operation. A
//! pending operation looks the same but **with no balance** — no `Balance` column:
//!
//! ```text
//! Oct 16, 2026 Contabo €6.60
//! ```
//!
//! That is the mark of a pending one: structural rather than textual. On the
//! real statement 7 pending ones separated this way from 198 completed, and the
//! statement itself says its balance counts the completed only.
use anyhow::Result;

use super::{looks_like_amount, parse_amount, parse_month_day_year, Bank, ParsedRow, Statement};
use crate::money::{Currency, Money};

/// Recognised by who issued the statement, not by any mention of the bank.
///
/// It was `text.contains("Revolut")` — and a real iCard statement was taken for
/// Revolut, because the word appears in the descriptions of transfers between
/// one's own accounts. The mark is the issuer's legal name plus the currency heading.
pub fn looks_like(text: &str) -> bool {
    text.contains("Revolut Bank") && currency_of(text).is_ok()
}

/// The statement's currency. The first page's heading is `EUR Statement`.
fn currency_of(text: &str) -> Result<Currency> {
    for line in text.lines().take(40) {
        let line = line.trim();
        if let Some(code) = line.strip_suffix(" Statement") {
            if let Ok(currency) = Currency::new(code) {
                return Ok(currency);
            }
        }
    }
    anyhow::bail!(crate::tr!(
        "no line like \"EUR Statement\" in the Revolut statement",
        "в выписке Revolut не нашлась строка вида \"EUR Statement\""
    ))
}

pub fn parse(text: &str) -> Result<Statement> {
    let currency = currency_of(text)?;
    let mut rows = Vec::new();
    // Which section a line is in — said by the headings, not guessed from
    // the line. Guessing by «one amount — pending» took a completed row whose
    // balance did not parse for a pending one and dropped it, and a pending
    // row whose description ended in a number («Top up 50.00 €50.00») for a
    // completed one and wrote it. (Mock statements, 2026-10-20.)
    let mut pending_section = false;

    for (index, raw) in text.lines().enumerate() {
        // A table's heading says which: completed operations have a Balance
        // column, pending ones («Start date … Money in») do not.
        let trimmed = raw.trim_start();
        if trimmed.starts_with("Pending from") {
            pending_section = true;
            continue;
        }
        if (trimmed.starts_with("Date ") || trimmed.starts_with("Start date ")) && trimmed.contains("Money out") {
            pending_section = !trimmed.contains("Balance");
            continue;
        }
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        if tokens.len() < 4 {
            continue;
        }
        let Some(occurred_on) = parse_month_day_year(&tokens) else { continue };

        // The amounts stand at the line's end. Take them off the tail while they come.
        let mut tail: Vec<Money> = Vec::new();
        let mut end = tokens.len();
        while end > 3 && looks_like_amount(tokens[end - 1]) {
            match parse_amount(tokens[end - 1], currency) {
                Some(money) => {
                    tail.push(money);
                    end -= 1;
                }
                None => break,
            }
        }
        tail.reverse();
        // A date with no amount after it is a heading («Sep 1, 2026 – Oct 16,
        // 2026»), not an operation.
        if tail.is_empty() {
            continue;
        }
        // A completed row ends in the amount and the balance; a pending one in
        // the amount. Anything before them is the description's own number.
        let wanted = if pending_section { 1 } else { 2 };
        if tail.len() < wanted {
            anyhow::bail!(crate::tr!(
                "line {}: an operation without its {} — nothing imported, the statement is not understood: «{}»",
                "строка {}: операция без {} — ничего не загружено, выписка не понята: «{}»",
                index + 1,
                if pending_section { crate::trs!("amount", "суммы") } else { crate::trs!("balance", "остатка") },
                raw.trim()
            ));
        }
        let spare = tail.len() - wanted;
        let description = tokens[3..end + spare].join(" ");
        if description.is_empty() {
            continue;
        }
        let tail = &tail[spare..];

        let (amount, running_balance, posted) = match tail {
            [amount, balance] => (*amount, Some(*balance), true),
            [amount] => (*amount, None, false),
            _ => continue,
        };

        rows.push(ParsedRow {
            line_no: index + 1,
            occurred_on,
            description,
            amount,
            running_balance,
            posted,
            raw: raw.to_string(),
        });
    }

    if rows.is_empty() {
        anyhow::bail!(crate::tr!(
            "not a single operation line in the Revolut statement",
            "в выписке Revolut не нашлось ни одной строки операции"
        ));
    }

    // Revolut does not print the sign in the text: a spend and an income look
    // alike, and the direction shows only by where the balance stepped. It is
    // restored along the chain — which is also the check on the row (§7).
    //
    // The chain has to be started with something, or the first row stays
    // signless: there is nowhere to step from it. It starts from the opening
    // balance the statement prints about itself in its summary. With no summary
    // the first row honestly stays as parsed, and that shows by the chain coming
    // out one shorter.
    let mut previous: Option<Money> = declared_totals(text).map(|t| t.opening);
    for row in rows.iter_mut().filter(|r| r.posted) {
        if let (Some(prev), Some(balance)) = (previous, row.running_balance) {
            let step = balance.minor() - prev.minor();
            if step < 0 {
                row.amount = row.amount.abs().negate();
            } else {
                row.amount = row.amount.abs();
            }
        }
        previous = row.running_balance;
    }

    Ok(Statement { bank: Bank::Revolut, currency, rows })
}

/// The totals the statement prints about itself: the opening balance, what went
/// out, what came in, the closing balance. Needed to check the parsing (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredTotals {
    pub opening: Money,
    pub out: Money,
    pub inn: Money,
    pub closing: Money,
}

pub fn declared_totals(text: &str) -> Option<DeclaredTotals> {
    let currency = currency_of(text).ok()?;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if !trimmed.starts_with("Total") && !trimmed.starts_with("Account") {
            continue;
        }
        let amounts: Vec<Money> = trimmed
            .split_whitespace()
            .filter(|t| looks_like_amount(t))
            .filter_map(|t| parse_amount(t, currency))
            .collect();
        if let [opening, out, inn, closing] = amounts.as_slice() {
            return Some(DeclaredTotals {
                opening: *opening,
                out: *out,
                inn: *inn,
                closing: *closing,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape repeats what `pdf-extract` gives on the real statement. The
    /// figures and names are invented: real statements never reach the repository.
    const SAMPLE: &str = "\
EUR Statement
Generated on Oct 16, 2026
Revolut Bank UAB
Balance summary
 Product Opening balance Money out Money in Closing balance
 Account (Current Account) €100.00 €30.00 €5.00 €75.00
 Total €100.00 €30.00 €5.00 €75.00
Date Description Money out Money in Balance
Sep 1, 2026 Shop €20.00 €80.00
Sep 2, 2026 Refund €5.00 €85.00
Sep 3, 2026 Coffee shop on the corner €10.00 €75.00
Pending from September 1, 2026 to October 16, 2026
 Start date Description Money out Money in
Oct 16, 2026 Hosting €6.60
";

    #[test]
    fn a_statement_is_recognised_and_its_currency_read_from_the_heading() {
        assert!(looks_like(SAMPLE));
        let statement = parse(SAMPLE).unwrap();
        assert_eq!(statement.bank, Bank::Revolut);
        assert_eq!(statement.currency.as_str(), "EUR");
    }

    /// A pending operation differs by having no balance in its line, not by a
    /// word in the text. On the real statement 7 separated this way from 198.
    #[test]
    fn a_row_without_a_running_balance_is_pending() {
        let statement = parse(SAMPLE).unwrap();
        assert_eq!(statement.rows.len(), 4);
        assert_eq!(statement.posted().count(), 3);
        assert_eq!(statement.pending_count(), 1);
        let pending = statement.rows.iter().find(|r| !r.posted).unwrap();
        assert_eq!(pending.description, "Hosting");
        assert!(pending.running_balance.is_none());
    }

    /// Revolut prints no sign: a spend and an income read alike. The direction
    /// is restored only by where the balance stepped.
    #[test]
    fn direction_is_recovered_from_the_balance_step() {
        let statement = parse(SAMPLE).unwrap();
        let posted: Vec<_> = statement.posted().collect();
        assert_eq!(posted[0].amount.minor(), -2000, "100.00 → 80.00, so a spend");
        assert_eq!(posted[1].amount.minor(), 500, "80.00 → 85.00, so an income");
        assert_eq!(posted[2].amount.minor(), -1000);
    }

    /// A description of several words must not be eaten by parsing the tail.
    #[test]
    fn a_description_of_several_words_survives() {
        let statement = parse(SAMPLE).unwrap();
        let row = statement.posted().nth(2).unwrap();
        assert_eq!(row.description, "Coffee shop on the corner");
    }

    /// The chain of balances checks every row, not only the batch's total.
    #[test]
    fn the_balance_chain_closes_on_every_row() {
        let statement = parse(SAMPLE).unwrap();
        let posted: Vec<_> = statement.posted().collect();
        let opening = declared_totals(SAMPLE).unwrap().opening;
        let mut previous = opening;
        for row in &posted {
            let balance = row.running_balance.unwrap();
            assert_eq!(
                balance.minor() - previous.minor(),
                row.amount.minor(),
                "row {} does not agree with the balance",
                row.line_no
            );
            previous = balance;
        }
    }

    /// The totals the statement prints about itself are what the parsing checks against.
    #[test]
    fn the_declared_totals_are_read_and_add_up() {
        let totals = declared_totals(SAMPLE).unwrap();
        assert_eq!(totals.opening.minor(), 10000);
        assert_eq!(totals.out.minor(), 3000);
        assert_eq!(totals.inn.minor(), 500);
        assert_eq!(totals.closing.minor(), 7500);
        assert_eq!(
            totals.opening.minor() - totals.out.minor() + totals.inn.minor(),
            totals.closing.minor()
        );
    }

    /// What was parsed must agree with what the statement said about itself.
    #[test]
    fn what_was_parsed_agrees_with_what_the_statement_declares() {
        let statement = parse(SAMPLE).unwrap();
        let totals = declared_totals(SAMPLE).unwrap();
        let (mut out, mut inn) = (0i64, 0i64);
        for row in statement.posted() {
            if row.amount.minor() < 0 {
                out -= row.amount.minor();
            } else {
                inn += row.amount.minor();
            }
        }
        assert_eq!(out, totals.out.minor());
        assert_eq!(inn, totals.inn.minor());
        assert_eq!(totals.opening.minor() - out + inn, totals.closing.minor());
    }

    #[test]
    fn something_that_is_not_a_statement_is_refused_rather_than_guessed() {
        assert!(parse("EUR Statement\nRevolut Bank UAB\nnothing here\n").is_err());
    }
}

#[cfg(test)]
mod detection_tests {
    use super::*;

    /// Another bank's statement with the word Revolut in its descriptions —
    /// transfers between one's own accounts. That is what the real iCard
    /// statement looked like, and on it a substring detector picked the wrong bank.
    #[test]
    fn a_mention_of_the_bank_in_a_description_is_not_a_statement_of_that_bank() {
        let other = "\
© iCard AD, www.icard.com
Дата и час Вальор Тип Описание Сума Валута Статус
15:53 06.10.2026 Transfer TO REVOLUT**1234 Revolut -50.00 EUR -50.00 EUR Approved
";
        assert!(!looks_like(other), "an iCard statement was taken for Revolut");
    }
}
