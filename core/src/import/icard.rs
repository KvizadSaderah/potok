//! An iCard statement.
//!
//! An operation here is **not** one line. The extracted text tears it where the
//! PDF wrapped, and there are more tears than whole lines: on the real
//! statement 13 records out of 87 came out whole. The first version of this
//! parser read only those, and 74 operations were lost quietly — caught by a
//! run on the real file, not by a test on a sample.
//!
//! The real shape is a record of several lines:
//!
//! ```text
//! 08.10.2026
//! 19:50 08.10.2026 POS Payment *1234, IKEA MALL SOFIA Sofia BGR, -38.04
//! EUR -38.04 EUR Approved
//! ```
//!
//! A record **begins** with a line of the form `HH:MM DD.MM.YYYY` and **ends**
//! with a line whose last word is a status. On the real statement there are 87
//! such beginnings and 87 such ends, so the marks are paired and no record is
//! lost. The lines between them are glued with a space and parsed as one.
//!
//! The sign is printed here, unlike in Revolut, so the direction need not be
//! restored from the balance. What is missing here: the balance after an
//! operation and the document's totals — so there is nothing to check the
//! parsing with from inside, and that is said aloud rather than hushed up.
use anyhow::Result;

use super::{looks_like_amount, parse_amount, parse_dotted_date, Bank, ParsedRow, Statement};
use crate::money::Currency;

/// Recognised by the issuer. `iCard AD` is the legal name in the footer of
/// every page; the word `iCard` alone is not enough, it may be in a description.
pub fn looks_like(text: &str) -> bool {
    text.contains("iCard AD") || text.contains("Айкарт")
}

/// The statuses iCard ends an operation's line with.
const STATUSES: [&str; 4] = ["Approved", "Declined", "Reversed", "Pending"];

pub fn parse(text: &str) -> Result<Statement> {
    let mut rows = Vec::new();
    let mut currency: Option<Currency> = None;

    let lines: Vec<&str> = text.lines().collect();
    let mut at = 0usize;
    while at < lines.len() {
        if !starts_a_record(lines[at]) {
            at += 1;
            continue;
        }
        let line_no = at + 1;

        // Collect up to a line ending in a status. The ceiling of ten lines is
        // so that a spoilt file does not glue the whole statement into one record.
        let mut joined = String::new();
        let mut taken = 0usize;
        let mut closed = false;
        while at < lines.len() && taken < 10 {
            // The next record began, and this one never said its status: an
            // unknown status, or a broken line. It used to run on into the
            // next record and take its amount — one operation lost, another
            // with half of two descriptions.
            if !joined.is_empty() && starts_a_record(lines[at]) {
                break;
            }
            if !joined.is_empty() {
                joined.push(' ');
            }
            joined.push_str(lines[at].trim());
            at += 1;
            taken += 1;
            if ends_a_record(&joined) {
                closed = true;
                break;
            }
        }
        // Not understood — the whole statement waits rather than half of it
        // going in with a hole where an operation was.
        let not_understood = || {
            anyhow::anyhow!(crate::tr!(
                "line {line_no}: an operation not understood — nothing imported: «{joined}»",
                "строка {line_no}: операция не понята — ничего не загружено: «{joined}»"
            ))
        };
        if !closed {
            return Err(not_understood());
        }
        let Some((row, row_currency)) = parse_record(&joined, line_no) else { return Err(not_understood()) };
        match currency {
            None => currency = Some(row_currency),
            Some(first) if first != row_currency => anyhow::bail!(crate::tr!(
                "line {line_no}: an operation in {row_currency} in a statement in {first} — one currency per account; nothing imported",
                "строка {line_no}: операция в {row_currency} в выписке в {first} — у счёта одна валюта; ничего не загружено"
            )),
            Some(_) => {}
        }
        rows.push(row);
    }

    let Some(currency) = currency else {
        anyhow::bail!(crate::tr!(
            "not a single operation line in the iCard statement",
            "в выписке iCard не нашлось ни одной строки операции"
        ))
    };

    Ok(Statement { bank: Bank::Icard, currency, rows })
}

/// A record's beginning: the time, then the date. No other line looks like it.
fn starts_a_record(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    let Some(time) = tokens.next() else { return false };
    let looks_like_time = time.len() == 5
        && time.as_bytes()[2] == b':'
        && time.chars().enumerate().all(|(i, c)| if i == 2 { c == ':' } else { c.is_ascii_digit() });
    looks_like_time && tokens.next().and_then(parse_dotted_date).is_some()
}

fn ends_a_record(joined: &str) -> bool {
    joined
        .split_whitespace()
        .last()
        .is_some_and(|last| STATUSES.contains(&last))
}

/// Parsing a glued record. Returns `None` if something is missing: skipping a
/// line quietly is better than writing half an operation.
fn parse_record(joined: &str, line_no: usize) -> Option<(ParsedRow, Currency)> {
    let tokens: Vec<&str> = joined.split_whitespace().collect();
    let status = *tokens.last()?;
    let date_at = tokens.iter().position(|t| parse_dotted_date(t).is_some())?;
    let occurred_on = parse_dotted_date(tokens[date_at])?;

    // The tail before the status is «amount currency». Take the last such pair
    // — it is from the column, not from the description, where the amount repeats.
    let before_status = &tokens[..tokens.len() - 1];
    let pair_at = (1..before_status.len()).rev().find(|at| {
        Currency::new(before_status[*at]).is_ok() && looks_like_amount(before_status[at - 1])
    })?;
    let row_currency = Currency::new(before_status[pair_at]).ok()?;
    let amount = parse_amount(before_status[pair_at - 1], row_currency)?;

    // The description runs from the date to where the amount repeats.
    //
    // What «looks like a currency code» must not be thrown away:
    // `Currency::new("POS")` passes — it is three ASCII letters — and the
    // description «POS Payment» lost its first word. Hence the exact repeat.
    let region = &before_status[date_at + 1..pair_at - 1];
    let amount_token = before_status[pair_at - 1];
    let cut = region
        .iter()
        .position(|t| *t == amount_token && looks_like_amount(t))
        .unwrap_or(region.len());
    let description = region[..cut].join(" ").trim_end_matches([',', ' ']).to_string();

    Some((
        ParsedRow {
            line_no,
            occurred_on,
            description,
            amount,
            running_balance: None,
            posted: status == "Approved",
            raw: joined.to_string(),
        },
        row_currency,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape repeats what `pdf-extract` gives on the real statement: the
    /// time, the date, the type, the description, then the amount with its
    /// currency twice and the status. The data is invented.
    const SAMPLE: &str = "\
© iCard AD, www.icard.com
История на трансакции
Дата и час Вальор Тип Описание Сума Валута Статус
15:53 06.10.2026 POS Payment *1234, SHOP 131 02 SOFIA BGR, -2.40 EUR -2.40 EUR Approved
16:20 06.10.2026 Incoming transfer FROM SOMEBODY 100.00 EUR 100.00 EUR Approved
17:05 07.10.2026 POS Payment *1234, OTHER SHOP SOFIA BGR, -28.06 EUR -28.06 EUR Declined
";

    #[test]
    fn a_statement_is_recognised_by_its_issuer() {
        assert!(looks_like(SAMPLE));
        assert!(looks_like("Айкарт АД е водещо дружество"));
        assert!(!looks_like("Revolut Bank UAB"));
    }

    #[test]
    fn every_row_is_read_with_its_sign_as_printed() {
        let statement = parse(SAMPLE).unwrap();
        assert_eq!(statement.bank, Bank::Icard);
        assert_eq!(statement.currency.as_str(), "EUR");
        assert_eq!(statement.rows.len(), 3);
        assert_eq!(statement.rows[0].amount.minor(), -240);
        assert_eq!(statement.rows[1].amount.minor(), 10000);
        assert_eq!(statement.rows[2].amount.minor(), -2806);
    }

    #[test]
    fn the_date_is_the_dotted_one_not_the_time() {
        let statement = parse(SAMPLE).unwrap();
        assert_eq!(statement.rows[0].occurred_on.to_string(), "2026-10-06");
        assert_eq!(statement.rows[2].occurred_on.to_string(), "2026-10-07");
    }

    /// Only `Approved` is money. A declined operation takes no part in the
    /// balance, nor does a pending one in Revolut (invariant 6).
    #[test]
    fn only_an_approved_row_counts_as_money() {
        let statement = parse(SAMPLE).unwrap();
        assert_eq!(statement.posted().count(), 2);
        assert!(!statement.rows[2].posted, "Declined is not posted");
    }

    /// The amount and currency are printed twice — in the description and in the
    /// column. They must not be in the description, or it reads as rubbish.
    #[test]
    fn the_repeated_amount_does_not_leak_into_the_description() {
        let statement = parse(SAMPLE).unwrap();
        assert_eq!(statement.rows[0].description, "POS Payment *1234, SHOP 131 02 SOFIA BGR");
        assert_eq!(statement.rows[1].description, "Incoming transfer FROM SOMEBODY");
        for row in &statement.rows {
            assert!(!row.description.contains("EUR"), "{:?}", row.description);
            assert!(!row.description.contains("2.40"), "{:?}", row.description);
        }
    }

    /// iCard has no balance in the line, and that has to be known: there is
    /// nothing to check the parsing with from inside the document.
    #[test]
    fn this_bank_prints_no_running_balance_so_nothing_self_checks() {
        let statement = parse(SAMPLE).unwrap();
        assert!(statement.rows.iter().all(|r| r.running_balance.is_none()));
    }

    /// A record torn by a wrap has to be put together. That is what the first
    /// version of the parser lost 74 operations out of 87 on — and what no test
    /// on a sample caught while the sample was made of whole lines.
    #[test]
    fn a_record_split_across_lines_is_put_back_together() {
        let split = "\
© iCard AD, www.icard.com
08.10.2026
19:50 08.10.2026 POS Payment *1234, IKEA MALL SOFIA BGR, -38.04
EUR -38.04 EUR Approved

03.10.2026
20:44 03.10.2026 Outgoing bank
transfer
 SOME COMPANY EOOD
BG00TEST00000000000001, -720.00 EUR,
returning back of advance payment -720.00 EUR Approved
";
        let statement = parse(split).unwrap();
        assert_eq!(statement.rows.len(), 2, "both records must come together");
        assert_eq!(statement.rows[0].amount.minor(), -3804);
        assert_eq!(statement.rows[0].description, "POS Payment *1234, IKEA MALL SOFIA BGR");
        assert_eq!(statement.rows[1].amount.minor(), -72000);
        assert!(
            statement.rows[1].description.contains("Outgoing bank transfer"),
            "{:?}",
            statement.rows[1].description
        );
        assert!(statement.rows[1].description.contains("SOME COMPANY EOOD"));
    }

    /// An unclosed record — one with no status line after it — must neither be
    /// written by halves nor eat the records that follow.
    #[test]
    fn a_record_that_never_closes_is_dropped_not_half_written() {
        let broken = "\
© iCard AD, www.icard.com
19:50 08.10.2026 POS Payment SOMETHING -1.00
";
        assert!(parse(broken).is_err(), "an unclosed record must not become an operation");
    }

    #[test]
    fn a_file_with_no_rows_is_refused() {
        assert!(parse("© iCard AD\nИстория на трансакции\n").is_err());
    }
}
