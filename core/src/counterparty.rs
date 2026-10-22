//! A counterparty out of the bank's description.
//!
//! A description is not the shop's name but what the processor wrote. One
//! and the same shop arrives like this:
//!
//! ```text
//! Revolut:  Kaufland
//! iCard:    POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR
//! ```
//!
//! Normalisation reduces both to the key `kaufland`, and from there it is one
//! counterparty. The rules below are drawn from real statements rather than
//! imagined: the list of prefixes and noise words is what actually turned up
//! in 285 operations from two banks.
//!
//! **What normalisation cannot do, and it is worth knowing.** `НОРТУИНД
//! БЪЛГАРИЯ ЕООД` and `NORTHWIND BULGARIA EOOD` are one company, and no
//! normalisation will bring them together: the alphabets differ. `dm drogerie`
//! and `DM BULGARIA EOOD` stay apart too. Aliases and `merge` are there for
//! that: merged by hand once, it holds from then on. Pretending it happens by
//! itself would be worse than saying so plainly.
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// The payment kinds a bank opens a description with. They are stripped
/// because they describe how it was paid, not who was paid.
const PREFIXES: [&str; 16] = [
    "pos payment reversal of",
    "pos payment",
    // Both turned up in iCard while checking the saldo migration: saldo
    // stripped them, potok did not, and one withdrawal gave two signatures.
    "atm deposit",
    "cash withdrawal",
    "online payment",
    "card payment",
    "outgoing bank transfer",
    "received bank transfer",
    "incoming bank transfer",
    "incoming transfer",
    "balance transfer",
    "transfer to",
    "transfer from",
    "bank transfer",
    "atm cash withdrawal",
    "fee",
];

/// Words that tell one merchant from another in no way: the legal form, the
/// country, the city. The list is short and open to additions — that is more
/// honest than a heuristic which one day eats a name.
const NOISE: [&str; 21] = [
    "eood", "ood", "ead", "ad", "ltd", "gmbh", "еоод", "оод", "ад",
    "bulgaria", "balgariya", "bulgariya", "българия", "бългaрия",
    "sofia", "sofiya", "sof", "soﬁa", "varna", "dublin", "gr.",
];

/// A description becomes the key by which two banks recognise one shop.
///
/// When the description holds an IBAN, the IBAN alone becomes the key. It is
/// the steadiest identifier a bank gives at all, and it settles two things
/// that otherwise do not settle:
///
/// * a transfer's description carries a note with a date — «rental payment
///   for 09.2026, due to rent contract from 22.03.2026» — and next month it
///   differs. By words it made a new counterparty every month;
/// * `НОРТУИНД БЪЛГАРИЯ ЕООД` and `NORTHWIND BULGARIA EOOD` are one company in
///   two alphabets. Their IBAN is one, and they come together by themselves.
pub fn normalize(description: &str) -> String {
    let key = normalize_with(description, true);
    // The payment kind is sometimes all a description holds: «ATM Cash
    // withdrawal, -1.00 EUR». Strip the kind and nothing is left, so the key
    // is empty, while the same row out of a PDF gives «atm cash withdrawal»
    // after the card mask. With nothing to strip, the kind is the name.
    if key.is_empty() {
        normalize_with(description, false)
    } else {
        key
    }
}

/// The service lines Revolut prints under an operation, which saldo glued
/// into the description: `Kaufland To: Kaufland Bulgaria, Sofiya Card: 4165…`.
/// potok takes the first line only, `Kaufland`. The merchant's name runs up
/// to the first such label; everything after it is detail. The list is taken
/// from real data (To ×917, Card ×847, From ×129, Reference ×127, Fee ×3), not
/// imagined; without it 185 of 198 Revolut rows were not recognised as known.
const DETAIL_LABELS: [&str; 6] = [
    " to: ",
    " card: ",
    " from: ",
    " reference: ",
    " fee: ",
    // The rate line under a payment in another currency, without a colon:
    // «Paytr. Wisecp Revolut Rate = 55.84 TRY (ECB rate* = 55.98 TRY)». The
    // last of 198 rows that the other rules still did not recognise.
    " revolut rate = ",
];

fn normalize_with(description: &str, strip_prefixes: bool) -> String {
    // Ligatures out of the PDF: `pdf-extract` gives «Soﬁa» as one glyph, and
    // without this step `IKEA MALL SOFIA Soﬁa BGR` and `IKEA MALL SOFIA Sofia
    // BGR` are different keys.
    let unligatured = description
        .replace('\u{fb01}', "fi")
        .replace('\u{fb02}', "fl")
        .replace('\u{fb00}', "ff")
        .replace('\u{fb03}', "ffi")
        .replace('\u{fb04}', "ffl");
    let mut lowered = format!(" {} ", unligatured.to_lowercase());
    if let Some(cut) = DETAIL_LABELS.iter().filter_map(|label| lowered.find(label)).min() {
        lowered.truncate(cut);
    }

    // An IBAN beats everything else: see the note above.
    if let Some(iban) = lowered
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()))
        .find(|token| is_iban(token))
    {
        return iban.to_string();
    }

    // Prefixes are stripped round and round while they strip: iCard writes
    // «Balance transfer Transfer from …», two of them in a row, and one pass
    // left «transfer from eur …». The longest match wins — otherwise «fee»
    // cuts into «fee for transfers…» before the exact match does.
    let mut body = lowered.trim();
    if strip_prefixes {
        loop {
            let mut longest: Option<&str> = None;
            for prefix in PREFIXES {
                if body.starts_with(prefix) && longest.is_none_or(|l| prefix.len() > l.len()) {
                    longest = Some(prefix);
                }
            }
            match longest {
                Some(prefix) => {
                    body = body[prefix.len()..].trim_start_matches([' ', ',', ':']).trim()
                }
                None => break,
            }
        }
    }

    let kept: Vec<&str> = body
        .split_whitespace()
        .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '*'))
        .map(without_card_mask)
        .filter(|token| !token.is_empty())
        .filter(|token| !is_card_mask(token))
        // Asterisks left from a mask somebody has already cleaned: saldo
        // keeps «Revolut** Dublin IRL» — the digits gone, the stars not.
        .map(|token| token.trim_matches('*'))
        .filter(|token| !token.is_empty())
        .filter(|token| !is_iban(token))
        .filter(|token| !token.chars().all(|c| c.is_ascii_digit()))
        // An amount and a currency inside a description are not a name. An
        // iCard transfer description holds «-1600.00 EUR», and without this
        // step the counterparty was called «ELENA GEORGIEVA 1600.00 EUR», and
        // an operation for another amount gave a second one just like it.
        .filter(|token| !crate::import::looks_like_amount(token))
        .filter(|token| !is_currency_code(token))
        .filter(|token| !NOISE.contains(&token.trim_end_matches('.')))
        .filter(|token| !is_country_code(token))
        .collect();

    // A dangling preposition at the end: after the card mask goes from «Apple
    // Pay deposit by *9012», «apple pay deposit by» is left. In the middle a
    // preposition means something («cash withdrawal at …»), at the end it does
    // not.
    // The same at the start: «Cash withdrawal at Unicredit» begins with «at»
    // once the payment kind is stripped. The ATM is the counterparty itself,
    // and a preposition before it names nothing.
    let preposition = |word: &&str| matches!(*word, "by" | "at" | "to" | "from" | "for" | "of");
    let mut kept = kept;
    while kept.last().is_some_and(preposition) {
        kept.pop();
    }
    let lead = kept.iter().take_while(|w| preposition(w)).count();
    let kept = &kept[lead..];

    kept.join(" ")
}

/// `*1234,`, `**5678*`, `*9012` and `revolut**5678*` are card masks. They
/// differ from card to card, name no merchant, and in a key do nothing but
/// harm: one payee reached from two cards read as two.
///
/// The asterisk is checked not only at the start: `revolut**5678*` arrived as
/// one token and used to pass straight through, giving the counterparties
/// `revolut**5678*` and `revolut**3456*` instead of one.
fn is_card_mask(token: &str) -> bool {
    token.starts_with('*') && token.chars().any(|c| c.is_ascii_digit())
}

/// Cuts off a card mask glued to a name: `revolut**5678*` → `revolut`.
fn without_card_mask(token: &str) -> &str {
    match token.find("**") {
        Some(at) if token[at..].chars().any(|c| c.is_ascii_digit()) => &token[..at],
        _ => token,
    }
}

/// A currency code as a separate word. A list, not a three-letter rule:
/// otherwise «DOM» or «BAR» would vanish out of a shop's name.
fn is_currency_code(token: &str) -> bool {
    matches!(token, "eur" | "usd" | "bgn" | "gbp" | "chf" | "pln" | "ron" | "try" | "uzs")
}

/// Two letters, two digits and at least ten more characters — an IBAN.
fn is_iban(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 14
        && bytes[..2].iter().all(|b| b.is_ascii_alphabetic())
        && bytes[2..4].iter().all(|b| b.is_ascii_digit())
        && bytes[4..].iter().all(|b| b.is_ascii_alphanumeric())
}

/// The three-letter country code iCard puts at the end: `BGR`, `IRL`. It
/// cannot be told from a three-letter name, so a list rather than a rule.
fn is_country_code(token: &str) -> bool {
    matches!(token, "bgr" | "irl" | "deu" | "usa" | "gbr" | "nld" | "ltu" | "pol")
}

/// Rename a counterparty by hand. If the name is taken by another, the two
/// are most likely one counterparty, and they are merged rather than renamed.
pub fn rename(conn: &Connection, from: &str, to: &str) -> Result<()> {
    let to = to.trim();
    if to.is_empty() {
        anyhow::bail!(crate::tr!("the new name is empty", "новое имя пустое"));
    }
    if to == from {
        anyhow::bail!(crate::tr!("the name is the same", "имя то же самое"));
    }
    crate::db::atomic(conn, || {
        let taken: bool = conn.query_row(
            "select exists (select 1 from counterparty where name = ?1)",
            params![to],
            |r| r.get(0),
        )?;
        if taken {
            anyhow::bail!(crate::tr!(
                "«{to}» already exists — if it is the same one, merge them instead of renaming",
                "«{to}» уже есть — если это один и тот же, свести их, а не переименовывать"
            ));
        }
        let changed = conn.execute(
            "update counterparty set name = ?1, renamed_at = datetime('now') where name = ?2",
            params![to, from],
        )?;
        if changed == 0 {
            anyhow::bail!(crate::tr!("no counterparty «{from}»", "нет контрагента «{from}»"));
        }
        crate::journal::log(
            conn,
            &crate::tr!("counterparty «{from}» renamed to «{to}»", "контрагент «{from}» переименован в «{to}»"),
            Some(crate::journal::Undo::Rename(to.to_string(), from.to_string())),
        )?;
        Ok(())
    })
}

/// «This is another counterparty»: this operation's spelling (its normalised
/// description) now means `to` — for it, for every operation of the same
/// counterparty with the same spelling, and for future imports. `to` is the
/// name of an existing counterparty, case aside, or a new one. Returns how
/// many operations moved.
pub fn reassign(conn: &mut Connection, operation_id: i64, to: &str) -> Result<usize> {
    let to = to.trim();
    if to.is_empty() {
        anyhow::bail!(crate::tr!("whom is empty", "кому — пусто"));
    }
    let transaction = conn.transaction()?;
    let (description, from): (Option<String>, Option<i64>) = transaction
        .query_row("select description, counterparty_id from operation where id = ?1", [operation_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .ok_or_else(|| anyhow::anyhow!(crate::tr!("no operation #{operation_id}", "нет операции #{operation_id}")))?;
    let alias = normalize(description.as_deref().unwrap_or(""));
    if alias.is_empty() {
        anyhow::bail!(crate::tr!(
            "the operation has no spelling to recognise a counterparty by",
            "у операции нет написания, по которому узнавать контрагента"
        ));
    }
    let target = match transaction
        .query_row("select id from counterparty where lower(name) = lower(?1)", params![to], |r| r.get::<_, i64>(0))
        .optional()?
    {
        Some(id) => id,
        None => {
            transaction.execute(
                "insert into counterparty (name, renamed_at) values (?1, datetime('now'))",
                params![to],
            )?;
            transaction.last_insert_rowid()
        }
    };
    if Some(target) == from {
        anyhow::bail!(crate::tr!("the operation is already with «{to}»", "операция и так у «{to}»"));
    }
    let mut moved: Vec<i64> = Vec::new();
    {
        let mut stmt =
            transaction.prepare("select id, coalesce(description, '') from operation where counterparty_id is ?1")?;
        for row in stmt.query_map(params![from], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, text) = row?;
            if normalize(&text) == alias {
                moved.push(id);
            }
        }
    }
    for id in &moved {
        transaction.execute("update operation set counterparty_id = ?1 where id = ?2", params![target, id])?;
    }
    let previous: Option<i64> = transaction
        .query_row("select counterparty_id from counterparty_alias where alias = ?1", params![alias], |r| r.get(0))
        .optional()?;
    transaction.execute(
        "insert or replace into counterparty_alias (alias, counterparty_id) values (?1, ?2)",
        params![alias, target],
    )?;
    let name: String = transaction.query_row("select name from counterparty where id = ?1", [target], |r| r.get(0))?;
    crate::journal::log(
        &transaction,
        &crate::tr!("spelling «{alias}» → «{name}»: {} operations", "написание «{alias}» → «{name}»: операций {}", moved.len()),
        Some(crate::journal::Undo::Respell { alias: alias.clone(), counterparty: from, alias_was: previous, operations: moved.clone() }),
    )?;
    transaction.commit()?;
    Ok(moved.len())
}

/// Finds a counterparty, creating nothing: for reviewing a statement first.
pub fn find(conn: &Connection, description: &str) -> Result<Option<i64>> {
    let alias = normalize(description);
    if alias.is_empty() {
        return Ok(None);
    }
    if let Some(id) = conn
        .query_row(
            "select counterparty_id from counterparty_alias where alias = ?1",
            params![alias],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(Some(id));
    }
    Ok(conn
        .query_row(
            "select id from counterparty where name = ?1",
            params![display_name(description, &alias)],
            |r| r.get::<_, i64>(0),
        )
        .optional()?)
}

/// What a refund says it is a refund of: «Reversal of YELLOW! +359…» →
/// «YELLOW! +359…», «Provisional refund for Mango Bulgaria Eood claim» →
/// «Mango Bulgaria Eood». `None` — not a refund by its words.
///
/// The words, not the counterparty: money from a shop you pay is not always
/// a refund, and money from a person never is (a debt, a gift).
pub fn refund_of(description: &str) -> Option<String> {
    let lower = description.to_lowercase();
    for marker in ["reversal of", "provisional refund for", "refund for", "refund from", "refund of", "возврат от", "возврат за"] {
        if let Some(at) = lower.find(marker) {
            // The lower-case copy finds the place; the original is cut there
            // only where that is a character boundary in it too.
            let rest = description.get(at + marker.len()..)?;
            let rest = rest.trim_start_matches(['*', ' ', ':']);
            let end = rest.to_lowercase().find(" claim").unwrap_or(rest.len());
            let merchant = rest[..end].trim();
            return (!merchant.is_empty()).then(|| merchant.to_string());
        }
    }
    None
}

/// The counterparty a refund belongs to — the shop it came back from — if
/// that shop is known. A refund then takes its spending down in the shop's
/// own category.
pub fn refund_counterparty(conn: &Connection, description: &str) -> Result<Option<i64>> {
    match refund_of(description) {
        Some(merchant) => find(conn, &merchant),
        None => Ok(None),
    }
}

/// A refund still counted as income: (operation, its counterparty now, the
/// shop's counterparty if known).
pub type RefundAsIncome = (i64, Option<i64>, Option<i64>);

/// Money in that its words call a refund but that still counts as income.
pub fn refunds_counted_as_income(conn: &Connection) -> Result<Vec<RefundAsIncome>> {
    let mut stmt = conn.prepare(
        "select id, description, counterparty_id from operation
          where state = 'POSTED' and role = 'income' and effect_minor > 0 and description is not null
          order by occurred_at, id",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (id, description, counterparty) in rows {
        if refund_of(&description).is_some() {
            out.push((id, counterparty, refund_counterparty(conn, &description)?));
        }
    }
    Ok(out)
}

/// Count each of them as a refund, and move its spelling to the shop it came
/// from when the shop is known — «Reversal of YELLOW» to «YELLOW», so the
/// next statement's refund lands there too. Each a journal line, each undone
/// on its own. Returns (refunds, moved to their shop).
pub fn take_refunds(conn: &mut Connection) -> Result<(usize, usize)> {
    let mut counted = 0;
    let mut moved = 0;
    for (id, _, shop) in refunds_counted_as_income(conn)? {
        crate::edit::set_counting(conn, id, crate::edit::Counting::Refund)?;
        counted += 1;
        // Read now, not from the list: moving one spelling takes every
        // operation with it, so the next one may be at its shop already.
        let now: Option<i64> = conn.query_row("select counterparty_id from operation where id = ?1", [id], |r| r.get(0))?;
        if let Some(shop) = shop.filter(|s| Some(*s) != now) {
            let name: String = conn.query_row("select name from counterparty where id = ?1", [shop], |r| r.get(0))?;
            reassign(conn, id, &name)?;
            moved += 1;
        }
    }
    Ok((counted, moved))
}

/// Finds a counterparty by its spelling, or creates a new one.
///
/// A new one takes its name from the description as written — `Kaufland`
/// reads better than `kaufland` — while the normalised alias stays the key.
pub fn resolve_or_create(conn: &Connection, description: &str) -> Result<Option<i64>> {
    let alias = normalize(description);
    // A description with nothing left after normalisation but an amount and a
    // currency names no counterparty. That is how a counterparty called
    // «1000.00 EUR» used to appear.
    if alias.is_empty() {
        return Ok(None);
    }

    if let Some(id) = conn
        .query_row(
            "select counterparty_id from counterparty_alias where alias = ?1",
            params![alias],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(Some(id));
    }

    let display = display_name(description, &alias);
    // A counterparty's name is unique: two different spellings may lead to one
    // name, and then it is one counterparty rather than two alike.
    let id = match conn
        .query_row("select id from counterparty where name = ?1", params![display], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
    {
        Some(id) => id,
        None => {
            conn.execute("insert into counterparty (name) values (?1)", params![display])?;
            conn.last_insert_rowid()
        }
    };
    conn.execute(
        "insert into counterparty_alias (alias, counterparty_id) values (?1, ?2)",
        params![alias, id],
    )?;
    Ok(Some(id))
}

/// The head of a description — up to Revolut's first service label, in its
/// original case.
/// The labels are ASCII, so the place is found in `to_ascii_lowercase()`:
/// every byte there sits where it sits in the original, and slicing the
/// result is safe. In `to_lowercase()` it is not: some letters have a lower
/// case of a different byte length, and the slice would slip or split a char.
fn head(description: &str) -> &str {
    let lower = description.to_ascii_lowercase();
    let cut = DETAIL_LABELS
        .iter()
        .filter_map(|label| lower.find(label))
        .min()
        .unwrap_or(description.len());
    &description[..cut]
}

/// The name to show: the same key, but in the description's own words, so that
/// a list reads «Kaufland» rather than «kaufland».
///
/// From the head of the description only, and without repeats case aside. The
/// first version took words from the whole description, and saldo keeps a
/// Revolut tail where the name repeats: «Transfer from ANNA T Reference: From
/// Anna T From: ANNA T». That gave 45 of 300 names like «LUCA ROSSI Luca».
fn display_name(description: &str, alias: &str) -> String {
    let head = head(description);

    // An IBAN key makes a poor name: a list should read «NORTHWIND BULGARIA»,
    // not twenty-two characters. Take the words before the IBAN, dropping the
    // operation type and the legal form — what normalisation does, but to show.
    if is_iban(alias) {
        let before = head.to_ascii_lowercase().find(alias).map(|at| &head[..at]).unwrap_or(head);
        let words: Vec<&str> = before
            .split_whitespace()
            .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|token| !token.is_empty())
            .filter(|token| !NOISE.contains(&token.to_lowercase().as_str()))
            .filter(|token| {
                let lower = token.to_lowercase();
                !PREFIXES.iter().any(|prefix| prefix.split(' ').any(|word| word == lower))
            })
            .collect();
        return match words.is_empty() {
            true => alias.to_string(),
            false => words.join(" "),
        };
    }

    let wanted: Vec<&str> = alias.split(' ').collect();
    let mut taken: Vec<&str> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for token in head.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.');
        let folded = cleaned
            .to_lowercase()
            .replace('\u{fb01}', "fi")
            .replace('\u{fb02}', "fl");
        if wanted.contains(&folded.as_str()) && !seen.contains(&folded) {
            seen.push(folded);
            taken.push(cleaned);
        }
    }
    if taken.is_empty() {
        alias.to_string()
    } else {
        taken.join(" ")
    }
}

/// How many names were rebuilt, and which could not be changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Refreshed {
    pub renamed: usize,
    /// (now, would like): the new name is taken by another counterparty. Merging
    /// them here is not mine to decide — that is a person's call, and `merge`.
    pub taken: Vec<(String, String)>,
}

/// Rebuilds every counterparty's name by the rules as they now are.
///
/// The name is stored, while the rules that derive it from a description change
/// — which is how 45 names stayed crooked after `display_name` was fixed. A
/// name is rebuilt from the descriptions of its own operations: the commonest,
/// and the shorter when tied. A counterparty with no operations (a debt only) is
/// left alone: its name came from a clean debt field, not from a description.
pub fn refresh_names(conn: &mut Connection) -> Result<Refreshed> {
    let transaction = conn.transaction()?;
    let mut current: Vec<(i64, String)> = Vec::new();
    {
        let mut stmt = transaction.prepare("select id, name from counterparty order by id")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            current.push(row?);
        }
    }
    let by_hand: std::collections::HashSet<i64> = {
        let mut stmt = transaction.prepare("select id from counterparty where renamed_at is not null")?;
        let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<_>>()?;
        ids
    };

    let mut target: std::collections::HashMap<i64, String> = Default::default();
    {
        let mut stmt = transaction.prepare(
            "select description from operation where counterparty_id = ?1 and description is not null",
        )?;
        for (id, name) in &current {
            // A name given by hand stays: a person chose it.
            if by_hand.contains(id) {
                target.insert(*id, name.clone());
                continue;
            }
            let mut votes: std::collections::HashMap<String, usize> = Default::default();
            for description in stmt.query_map(params![id], |r| r.get::<_, String>(0))? {
                let description = description?;
                let alias = normalize(&description);
                if !alias.is_empty() {
                    *votes.entry(display_name(&description, &alias)).or_insert(0) += 1;
                }
            }
            let best = votes
                .into_iter()
                .max_by(|(a, an), (b, bn)| an.cmp(bn).then(b.len().cmp(&a.len())).then(b.cmp(a)));
            target.insert(*id, best.map(|(n, _)| n).unwrap_or_else(|| name.clone()));
        }
    }

    // A new name that collided with another's falls back to the current one; a
    // fallback can create a new collision, so — until there are none.
    let mut out = Refreshed::default();
    loop {
        let mut count: std::collections::HashMap<&str, usize> = Default::default();
        for name in target.values() {
            *count.entry(name.as_str()).or_insert(0) += 1;
        }
        let clashing: Vec<i64> = current
            .iter()
            .filter(|(id, name)| target[id] != *name && count[target[id].as_str()] > 1)
            .map(|(id, _)| *id)
            .collect();
        if clashing.is_empty() {
            break;
        }
        for id in clashing {
            let (_, name) = current.iter().find(|(c, _)| *c == id).expect("from the same list");
            out.taken.push((name.clone(), target[&id].clone()));
            target.insert(id, name.clone());
        }
    }

    // A name is unique, and SQLite checks that per row rather than at the end:
    // swapping two names would fail halfway. Hence temporary names first.
    let changing: Vec<(i64, &String)> =
        current.iter().filter(|(id, name)| target[id] != *name).map(|(id, _)| (*id, &target[id])).collect();
    for (id, _) in &changing {
        transaction.execute(
            "update counterparty set name = ?1 where id = ?2",
            params![format!("\u{0}refresh {id}"), id],
        )?;
    }
    for (id, name) in &changing {
        transaction.execute("update counterparty set name = ?1 where id = ?2", params![name, id])?;
    }
    if !changing.is_empty() {
        crate::journal::log(&transaction, &crate::tr!("counterparties renamed: {}", "переименовано контрагентов: {}", changing.len()), None)?;
    }
    transaction.commit()?;
    out.renamed = changing.len();
    Ok(out)
}

/// Gives a counterparty a category — which is the categorisation rule itself.
/// Operations, past and future, take it on their own: the category is read
/// through `coalesce`, so nothing has to be rewritten.
/// Ties a spelling to a counterparty: so a bank may call it whatever it likes
/// and it still leads to one. The spelling is stored as a normalisation key,
/// not as the bank's text — the same key an import looks up.
///
/// Returns the key that came out: «POS Payment *1234, BILLA» gives «billa»,
/// and a person is better off seeing that.
pub fn set_alias(conn: &Connection, text: &str, counterparty: &str) -> Result<String> {
    let alias = normalize(text);
    if alias.is_empty() {
        anyhow::bail!(crate::tr!(
            "no key comes out of {text:?} — normalisation left nothing",
            "из {text:?} не выходит ключа — нормализация оставила пустоту"
        ));
    }
    crate::db::atomic(conn, || {
        let id: i64 = conn
            .query_row("select id from counterparty where name = ?1", params![counterparty], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!(crate::tr!("no counterparty {counterparty:?}", "нет контрагента {counterparty:?}")))?;
        let was: Option<i64> =
            conn.query_row("select counterparty_id from counterparty_alias where alias = ?1", params![alias], |r| r.get(0)).optional()?;
        if was == Some(id) {
            return Ok(alias.clone());
        }
        conn.execute("insert or replace into counterparty_alias (alias, counterparty_id) values (?1, ?2)", params![alias, id])?;
        crate::journal::log(
            conn,
            &crate::tr!("spelling «{alias}» → «{counterparty}»", "написание «{alias}» → «{counterparty}»"),
            Some(crate::journal::Undo::Alias(alias.clone(), was)),
        )?;
        Ok(alias.clone())
    })
}

/// Removes a spelling. Operations already tied to the counterparty stay with
/// it: a spelling is for the next statement, not for past rows.
pub fn drop_alias(conn: &Connection, alias: &str) -> Result<()> {
    let alias = normalize(alias);
    crate::db::atomic(conn, || {
        let was: Option<i64> =
            conn.query_row("select counterparty_id from counterparty_alias where alias = ?1", params![alias], |r| r.get(0)).optional()?;
        let Some(was) = was else {
            anyhow::bail!(crate::tr!("no spelling «{alias}»", "нет написания «{alias}»"));
        };
        conn.execute("delete from counterparty_alias where alias = ?1", params![alias])?;
        crate::journal::log(
            conn,
            &crate::tr!("spelling «{alias}» removed", "написание «{alias}» убрано"),
            Some(crate::journal::Undo::Alias(alias.clone(), Some(was))),
        )?;
        Ok(())
    })
}

/// A note by hand: what the place the bank called «NORD OOD» actually is. An
/// empty string clears it. Written to the journal and undoable.
pub fn set_note(conn: &Connection, counterparty: &str, note: &str) -> Result<()> {
    crate::db::atomic(conn, || {
        let was: Option<String> = conn
            .query_row("select note from counterparty where name = ?1", params![counterparty], |r| r.get(0))
            .optional()?
            .flatten();
        let note = note.trim();
        let value = (!note.is_empty()).then(|| note.to_string());
        let changed = conn.execute("update counterparty set note = ?1 where name = ?2", params![value, counterparty])?;
        if changed == 0 {
            anyhow::bail!(crate::tr!(
                "no counterparty {counterparty:?}; `potok counterparties` shows who there is",
                "нет контрагента {counterparty:?}; `potok counterparties` покажет, кто есть"
            ));
        }
        crate::journal::log(
            conn,
            &match &value {
                Some(note) => crate::tr!("note on «{counterparty}»: {note}", "заметка на «{counterparty}»: {note}"),
                None => crate::tr!("note on «{counterparty}» removed", "заметка с «{counterparty}» снята"),
            },
            Some(crate::journal::Undo::Note(counterparty.to_string(), was)),
        )?;
        Ok(())
    })
}

pub fn set_category(conn: &Connection, counterparty: &str, category: &str) -> Result<usize> {
    crate::db::atomic(conn, || {
        let before: Option<String> = conn
            .query_row(
                "select c.name from counterparty p left join category c on c.id = p.category_id where p.name = ?1",
                params![counterparty],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let category_id = match conn
            .query_row("select id from category where name = ?1", params![category], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            Some(id) => id,
            None => {
                conn.execute("insert into category (name) values (?1)", params![category])?;
                conn.last_insert_rowid()
            }
        };
        let changed = conn.execute(
            "update counterparty set category_id = ?1 where name = ?2",
            params![category_id, counterparty],
        )?;
        if changed == 0 {
            anyhow::bail!(crate::tr!(
                "no counterparty {counterparty:?}; `potok counterparties` shows who there is",
                "нет контрагента {counterparty:?}; `potok counterparties` покажет, кто есть"
            ));
        }
        // An operation's own category that matched the counterparty's new one says
        // nothing more — and gets in the way: the next recategorisation of the
        // counterparty would miss those rows. After the move from saldo Kaufland has
        // 52 of them. Their ids go into Undo, so undoing restores their own category.
        let cleared_operations: Vec<i64> = {
            let mut stmt = conn.prepare(
                "select id from operation
                  where category_id = ?1
                    and counterparty_id = (select id from counterparty where name = ?2)
                  order by id",
            )?;
            let rows = stmt.query_map(params![category_id, counterparty], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        conn.execute(
            "update operation set category_id = null
              where category_id = ?1
                and counterparty_id = (select id from counterparty where name = ?2)",
            params![category_id, counterparty],
        )?;
        let affected: i64 = conn.query_row(
            "select count(*) from operation o
               join counterparty p on p.id = o.counterparty_id
              where p.name = ?1 and o.category_id is null",
            params![counterparty],
            |r| r.get(0),
        )?;
        crate::journal::log(
            conn,
            &crate::tr!(
                "category «{counterparty}»: {} → {category}",
                "категория «{counterparty}»: {} → {category}",
                before.as_deref().unwrap_or("—")
            ),
            Some(crate::journal::Undo::CounterpartyCategory(
                counterparty.to_string(),
                before,
                cleared_operations,
            )),
        )?;
        Ok(affected as usize)
    })
}

/// «This operation is not Other but Healthcare — and so are the rest of its».
///
/// Makes the category the counterparty's rule (`set_category`) and strips the
/// own category off its operations still counted under `from`: without that the
/// rule never reaches them — an operation's own category wins
/// (`query::FLOW_LABEL`). After the move from saldo the categories sit on the
/// operations, so without this step «move the rest» moves nothing.
///
/// Returns (how many operations came under the rule, how many stripped of `from`).
pub fn take_over(conn: &Connection, counterparty: &str, category: &str, from: Option<&str>) -> Result<(usize, usize)> {
    let under_rule = set_category(conn, counterparty, category)?;
    let Some(from) = from.filter(|name| *name != category) else { return Ok((under_rule, 0)) };
    crate::db::atomic(conn, || {
        let moved: Vec<i64> = {
            let mut stmt = conn.prepare(
                "select o.id from operation o
                   join counterparty p on p.id = o.counterparty_id
                   join category     c on c.id = o.category_id
                  where p.name = ?1 and c.name = ?2
                  order by o.id",
            )?;
            let rows = stmt.query_map(params![counterparty, from], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if moved.is_empty() {
            return Ok((under_rule, 0));
        }
        let mut done = 0usize;
        for id in &moved {
            done += conn.execute("update operation set category_id = null where id = ?1", params![id])?;
        }
        crate::journal::log(
            conn,
            &crate::tr!(
                "«{counterparty}»: {done} operations taken out of {from} — the counterparty's {category} applies",
                "«{counterparty}»: {done} операций вынуто из {from} — действует {category} контрагента"
            ),
            Some(crate::journal::Undo::OwnCategories(moved, from.to_string())),
        )?;
        Ok((under_rule + done, done))
    })
}

/// Counterparties without a category whose operations are already, at
/// `min_percent` or more, in one category — the hint `C` offers one at a
/// time. After the saldo migration the categories lived on operations, and
/// every new statement put BILLA, LIDL and KAUFLAND outside «Food».
/// (name, category, operations in it, operations).
pub fn confident_hints(conn: &Connection, min_percent: usize) -> Result<Vec<(String, String, usize, usize)>> {
    Ok(crate::query::counterparties(conn)?
        .into_iter()
        .filter(|line| line.category.is_none() && line.operations > 0)
        .filter_map(|line| {
            let (category, count) = line.hint?;
            (count * 100 >= min_percent * line.operations).then_some((line.name, category, count, line.operations))
        })
        .collect())
}

/// Take every such hint: one journal entry per counterparty, each undone on
/// its own; all or none.
pub fn take_confident_hints(conn: &mut Connection, min_percent: usize) -> Result<usize> {
    let transaction = conn.transaction()?;
    let hints = confident_hints(&transaction, min_percent)?;
    for (name, category, _, _) in &hints {
        set_category(&transaction, name, category)?;
    }
    transaction.commit()?;
    Ok(hints.len())
}

/// Folds two counterparties into one: aliases and operations move, the empty
/// one is deleted. What closes the gap between alphabets and spellings.
pub fn merge(conn: &mut Connection, from: &str, into: &str) -> Result<usize> {
    let ids: Vec<i64> = [from, into]
        .iter()
        .map(|name| {
            conn.query_row("select id from counterparty where name = ?1", params![name], |r| {
                r.get::<_, i64>(0)
            })
            .optional()
            .map_err(anyhow::Error::from)
            .and_then(|found| found.ok_or_else(|| anyhow::anyhow!(crate::tr!("no counterparty {name:?}", "нет контрагента {name:?}"))))
        })
        .collect::<Result<_>>()?;
    let (from_id, into_id) = (ids[0], ids[1]);
    if from_id == into_id {
        anyhow::bail!(crate::tr!("that is the same counterparty", "это один и тот же контрагент"));
    }

    let transaction = conn.transaction()?;
    transaction.execute(
        "update counterparty_alias set counterparty_id = ?1 where counterparty_id = ?2",
        params![into_id, from_id],
    )?;
    let moved = transaction.execute(
        "update operation set counterparty_id = ?1 where counterparty_id = ?2",
        params![into_id, from_id],
    )?;
    transaction.execute(
        "update debt set counterparty_id = ?1 where counterparty_id = ?2",
        params![into_id, from_id],
    )?;
    transaction.execute("delete from counterparty where id = ?1", params![from_id])?;
    // Not undoable: which spellings and operations belonged to «from» cannot be
    // recovered after a merge — that is what a merge means.
    crate::journal::log(&transaction, &crate::tr!("counterparty «{from}» merged into «{into}»: {moved} operations", "контрагент «{from}» сведён в «{into}»: {moved} операций"), None)?;
    transaction.commit()?;
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// Every sample below is a real shape of a description from a Revolut or
    /// iCard statement, with names and numbers replaced. None is made up: on
    /// invented samples the iCard parser once showed green tests while losing 74
    /// operations out of 87.
    /// The bank calls the place «NORD OOD», and only a person knows what it is.
    /// A note is their word, and it is undoable.
    #[test]
    fn a_note_says_what_the_bank_would_not() {
        let mut conn = crate::db::open_memory().unwrap();
        conn.execute_batch("insert into counterparty (name) values ('NORD');").unwrap();
        let note = |conn: &Connection| -> Option<String> {
            conn.query_row("select note from counterparty where name = 'NORD'", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(note(&conn), None);
        set_note(&conn, "NORD", "  the coffee place by the office  ").unwrap();
        assert_eq!(note(&conn).as_deref(), Some("the coffee place by the office"), "edge spaces are not stored");
        set_note(&conn, "NORD", "a bakery").unwrap();
        assert_eq!(note(&conn).as_deref(), Some("a bakery"));
        assert!(set_note(&conn, "no such thing", "x").is_err());

        let entry: i64 = conn.query_row("select max(id) from change_log where undo like 'note%'", [], |r| r.get(0)).unwrap();
        crate::journal::undo(&mut conn, entry).unwrap();
        assert_eq!(note(&conn).as_deref(), Some("the coffee place by the office"), "undo restored the previous note");

        set_note(&conn, "NORD", "   ").unwrap();
        assert_eq!(note(&conn), None, "an empty note clears it");
    }

    /// The move from saldo put the categories on the operations, not on the
    /// counterparty: its rule never reaches them until they are stripped.
    #[test]
    fn taking_a_shop_over_moves_the_operations_that_sat_in_the_old_category() {
        let mut conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('iCard','EUR','CARD');
             insert into category (name) values ('Other'), ('Healthcare'), ('Gifts');
             insert into counterparty (name) values ('Pharmacy');
             insert into operation (id,account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,1,'expense','2026-10-01T00:00:00Z',1000,'EUR',-1000,'PHARMACY',1,1),
               (2,1,'expense','2026-10-02T00:00:00Z',2000,'EUR',-2000,'PHARMACY',1,1),
               (3,1,'expense','2026-10-03T00:00:00Z',3000,'EUR',-3000,'PHARMACY',1,3);",
        )
        .unwrap();
        let own = |conn: &Connection, id: i64| -> Option<String> {
            conn.query_row(
                "select c.name from operation o left join category c on c.id = o.category_id where o.id = ?1",
                params![id],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap()
        };

        // One operation fixed by hand, the rest still under «Other».
        crate::edit::set_own_category(&conn, 1, Some("Healthcare")).unwrap();
        let (under_rule, moved) = take_over(&conn, "Pharmacy", "Healthcare", Some("Other")).unwrap();
        assert_eq!(moved, 1, "only the one sitting in Other was pulled out");
        assert!(under_rule >= 2, "{under_rule}");
        assert_eq!(own(&conn, 1), None, "the one-off edit is gone — taken from the counterparty");
        assert_eq!(own(&conn, 2), None, "followed the rule");
        assert_eq!(own(&conn, 3).as_deref(), Some("Gifts"), "a different own category is untouched");
        assert_eq!(
            conn.query_row("select c.name from counterparty p join category c on c.id = p.category_id where p.name = 'Pharmacy'", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "Healthcare"
        );

        // Undo restores «Other» itself, not the counterparty's category.
        let entry: i64 = conn.query_row("select max(id) from change_log where undo like 'own-categories%'", [], |r| r.get(0)).unwrap();
        crate::journal::undo(&mut conn, entry).unwrap();
        assert_eq!(own(&conn, 2).as_deref(), Some("Other"), "undo restored the previous category");
    }


    #[test]
    fn a_clean_merchant_name_survives_untouched() {
        assert_eq!(normalize("Kaufland"), "kaufland");
        assert_eq!(normalize("dm drogerie"), "dm drogerie");
        assert_eq!(normalize("Mr. Bricolage"), "mr. bricolage");
    }

    /// The whole point of normalisation: one shop out of two banks.
    #[test]
    fn the_same_shop_from_two_banks_gets_one_key() {
        assert_eq!(
            normalize("POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR"),
            normalize("Kaufland")
        );
        assert_eq!(
            normalize("POS Payment *1234, LIDL BALGARIYA EOOD SOFIYA BGR"),
            normalize("Lidl")
        );
        assert_eq!(normalize("POS Payment *1234, BILLA 131 02 SOFIA BGR"), normalize("BILLA"));
    }

    /// A PDF ligature: `pdf-extract` gives «Soﬁa» as a single glyph.
    #[test]
    fn a_ligature_from_the_pdf_does_not_make_a_second_counterparty() {
        assert_eq!(
            normalize("POS Payment *1234, IKEA MALL SOFIA So\u{fb01}a BGR"),
            normalize("POS Payment *1234, IKEA MALL SOFIA Sofia BGR")
        );
        assert_eq!(normalize("POS Payment *1234, PEPKO 211073 So\u{fb01}a BGR"), "pepko");
    }

    /// A card mask glued to the name: `revolut**5678*`. Different cards, one
    /// payee.
    #[test]
    fn a_card_mask_inside_a_token_is_cut_off() {
        assert_eq!(
            normalize("Online payment *1234, Revolut**5678* Dublin IRL"),
            normalize("Online payment *1234, Revolut**3456* Dublin IRL")
        );
        // A preposition left hanging by the mask goes too: «… deposit by» → «… deposit».
        assert_eq!(normalize("Apple Pay deposit by *9012"), "apple pay deposit");
        // In the middle a preposition means something and stays.
        assert_eq!(normalize("Payment at Fib_atm branch"), "payment at fib_atm branch");
        // A cash withdrawal: the payment type goes, and the ATM becomes the counterparty.
        assert_eq!(normalize("Cash withdrawal at Fib_atm"), "fib_atm");
    }

    /// An amount in a description must not reach the key: otherwise every new
    /// amount gives a new counterparty.
    #[test]
    fn an_amount_in_the_description_is_not_part_of_the_name() {
        let first = normalize("Transfer to SOMEBODY, -1600.00 EUR, rent");
        let second = normalize("Transfer to SOMEBODY, -1700.00 EUR, rent");
        assert_eq!(first, second);
        assert_eq!(first, "somebody rent");
    }

    /// A description holding nothing but an amount names no counterparty.
    #[test]
    fn a_description_of_nothing_but_money_yields_no_key() {
        assert_eq!(normalize("-1000.00 EUR"), "");
        assert_eq!(normalize("*1234, 500.00 BGN"), "");
    }

    /// Two prefixes in a row — a real iCard shape.
    #[test]
    fn prefixes_are_stripped_until_none_are_left() {
        assert_eq!(normalize("POS Payment Reversal of*1234, NORD OOD SOFIA BGR"), "nord");
        assert!(!normalize("Balance transfer Transfer from 206 / EUR to 910 / EUR balance")
            .contains("transfer"));
    }

    /// One person in both directions — one counterparty.
    #[test]
    fn money_to_and_from_the_same_person_is_one_counterparty() {
        assert_eq!(normalize("Transfer to MILA STOEVA"), normalize("Transfer from MILA STOEVA"));
        assert_eq!(normalize("Transfer to MILA STOEVA"), "mila stoeva");
    }

    /// An IBAN beats the words — otherwise a memo with a date gives a new
    /// counterparty every month.
    #[test]
    fn an_iban_outranks_the_words_around_it() {
        let september = normalize(
            "Outgoing bank transfer SOMEBODY BG00TEST00000000000001, -1600.00 EUR, \
             rental payment for 09.2026",
        );
        let october = normalize(
            "Outgoing bank transfer SOMEBODY BG00TEST00000000000001, -1600.00 EUR, \
             rental payment for 10.2026, due to contract from 22.03.2026",
        );
        assert_eq!(september, october);
        // The key is lower case like every other: case means nothing in it.
        assert_eq!(september, "bg00test00000000000001");
    }

    /// What normalisation by words cannot do: one payee in two alphabets. Their
    /// IBAN is shared, and they come together on their own.
    #[test]
    fn two_alphabets_meet_on_the_iban() {
        let latin = "Outgoing bank transfer SOME COMPANY EOOD BG00TEST00000000000002";
        let cyrillic = "Received bank transfer СЪМ КОМПАНИ ЕООД BG00TEST00000000000002, 1234.56";
        assert_eq!(normalize(latin), normalize(cyrillic));
    }

    // ── Shapes found while checking the move from saldo, 2026-10-18 ───────
    //
    // Each pair below is one and the same operation: on the left as potok reads
    // it from the PDF, on the right as saldo stores it. Before these rules the
    // key did not connect them, and importing a statement over the moved data
    // doubled 200 operations out of 285.

    #[test]
    fn revolut_detail_lines_are_not_part_of_the_name() {
        assert_eq!(normalize("Kaufland"), normalize("Kaufland To: Kaufland Bulgaria, Sofiya Card: 400000******0000"));
        assert_eq!(normalize("BILLA"), normalize("BILLA To: Billa 175 02, Sofia Card: 400000******0000"));
        assert_eq!(
            normalize("Transfer to SOMEBODY"),
            normalize("Transfer to SOMEBODY Reference: To SOMEBODY"),
        );
        assert_eq!(normalize("Vinted"), normalize("Vinted To: Vinted, Vilnius Card: 500000*****"));
        assert_eq!(
            normalize("Paytr. Wisecp"),
            normalize("Paytr. Wisecp Revolut Rate = 55.84 TRY (ECB rate* = 55.98 TRY) 359.99"),
            "the rate line of a payment in another currency"
        );
    }

    /// «To» without a colon is part of the name, not a label.
    #[test]
    fn a_word_that_merely_looks_like_a_label_stays() {
        assert_eq!(normalize("Transfer to SOMEBODY"), "somebody");
        assert_eq!(normalize("Back To School Store"), "back to school store");
    }

    #[test]
    fn icard_payment_types_saldo_stripped_are_stripped_here_too() {
        assert_eq!(
            normalize("ATM Deposit , ATM ICARD MALL SOFIA E SOFIA BGR"),
            normalize("ATM ICARD MALL SOFIA E SOFIA BGR, 109.50 EUR"),
        );
        assert_eq!(
            normalize("Cash withdrawal *1234, ATM ICARD MALL SOFIA E SOFIA BGR"),
            normalize("ATM ICARD MALL SOFIA E SOFIA BGR, -20.00 EUR"),
        );
    }

    /// The payment type is sometimes all a description holds.
    #[test]
    fn when_the_payment_type_is_all_there_is_it_is_the_name() {
        assert_eq!(normalize("Fee *1234, ATM Cash withdrawal"), normalize("ATM Cash withdrawal, -1.00 EUR"));
        assert_eq!(normalize("ATM Cash withdrawal, -1.00 EUR"), "atm cash withdrawal");
    }

    /// A mask someone has already cleaned: no digits, the asterisks left.
    #[test]
    fn stars_left_from_a_cleaned_mask_do_not_split_a_counterparty() {
        assert_eq!(
            normalize("Online payment *1234, Revolut**5678* Dublin IRL"),
            normalize("Revolut** Dublin IRL, -1000.00 EUR"),
        );
    }

    /// The name to show reads in words rather than as a key, and for an IBAN it
    /// is taken from the words before it.
    #[test]
    fn the_display_name_is_readable() {
        let conn = db::open_memory().unwrap();
        resolve_or_create(&conn, "POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR").unwrap();
        resolve_or_create(
            &conn,
            "Outgoing bank transfer SOME COMPANY EOOD BG00TEST00000000000002",
        )
        .unwrap();
        let names: Vec<String> = conn
            .prepare("select name from counterparty order by name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(names.contains(&"KAUFLAND".to_string()), "{names:?}");
        assert!(names.contains(&"SOME COMPANY".to_string()), "{names:?}");
    }

    /// The real shape this was found on: the name repeats in Revolut's service
    /// tail in another case. The names are invented.
    #[test]
    fn a_name_is_taken_from_before_the_detail_lines_once() {
        let conn = db::open_memory().unwrap();
        for (description, expected) in [
            ("Transfer from ANNA TESTOVA Reference: From Anna T From: ANNA TESTOVA", "ANNA TESTOVA"),
            ("AmRest To: Amrest Kofi Eood, 1104 Sofiya, BGR Card: 400000******0000", "AmRest"),
            ("dm drogerie To: Dm Drogerie Markt, Sofia Card: 400000******0000", "dm drogerie"),
        ] {
            let id = resolve_or_create(&conn, description).unwrap().unwrap();
            let name: String = conn
                .query_row("select name from counterparty where id = ?1", params![id], |r| r.get(0))
                .unwrap();
            assert_eq!(name, expected, "out of {description:?}");
        }
    }

    /// A letter whose lower case is longer in bytes, before an IBAN: a position
    /// found after `to_lowercase()` would have slipped in the original string.
    #[test]
    fn a_letter_that_grows_when_lowered_does_not_break_the_iban_cut() {
        let description = "Outgoing bank transfer İSTANBUL ŞİRKETİ BG00TEST00000000000002";
        let alias = normalize(description);
        assert_eq!(alias, "bg00test00000000000002");
        assert_eq!(display_name(description, &alias), "İSTANBUL ŞİRKETİ");
    }

    /// Names written by the old rules are rebuilt by the rules as they now are.
    #[test]
    fn stored_names_are_rebuilt_by_the_current_rules() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into counterparty (id, name) values (1, 'ANNA TESTOVA Anna'), (2, 'Kaufland');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
                 values (1,'income','2026-09-01T00:00:00Z',100,'EUR',100,
                         'Transfer from ANNA TESTOVA Reference: From Anna T From: ANNA TESTOVA',1),
                        (1,'expense','2026-09-01T00:00:00Z',100,'EUR',-100,'Kaufland',2);",
        )
        .unwrap();
        let refreshed = refresh_names(&mut conn).unwrap();
        assert_eq!(refreshed, Refreshed { renamed: 1, taken: vec![] });
        let name: String =
            conn.query_row("select name from counterparty where id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "ANNA TESTOVA");
        assert_eq!(refresh_names(&mut conn).unwrap().renamed, 0, "a second run changes nothing");
    }

    /// The new name is taken — do not fold silently, keep it and say so.
    #[test]
    fn a_name_already_taken_is_left_alone_and_reported() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into counterparty (id, name) values (1, 'dm drogerie Dm'), (2, 'dm drogerie');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
                 values (1,'expense','2026-09-01T00:00:00Z',100,'EUR',-100,'dm drogerie To: Dm Drogerie',1),
                        (1,'expense','2026-09-02T00:00:00Z',100,'EUR',-100,'dm drogerie',2);",
        )
        .unwrap();
        let refreshed = refresh_names(&mut conn).unwrap();
        assert_eq!(refreshed.renamed, 0);
        assert_eq!(refreshed.taken, vec![("dm drogerie Dm".to_string(), "dm drogerie".to_string())]);
    }

    /// Swapping two names: SQLite checks uniqueness per row, and a direct
    /// rename would fail halfway.
    #[test]
    fn two_names_can_swap() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into counterparty (id, name) values (1, 'Lidl'), (2, 'Kaufland');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
                 values (1,'expense','2026-09-01T00:00:00Z',100,'EUR',-100,'Kaufland',1),
                        (1,'expense','2026-09-02T00:00:00Z',100,'EUR',-100,'Lidl',2);",
        )
        .unwrap();
        assert_eq!(refresh_names(&mut conn).unwrap().renamed, 2);
    }

    #[test]
    fn the_same_description_always_resolves_to_the_same_counterparty() {
        let conn = db::open_memory().unwrap();
        let first = resolve_or_create(&conn, "Kaufland").unwrap();
        let second =
            resolve_or_create(&conn, "POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR").unwrap();
        assert_eq!(first, second);
        let count: i64 =
            conn.query_row("select count(*) from counterparty", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "one shop must not have two counterparties");
    }

    fn with_operations(conn: &Connection) {
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');",
        )
        .unwrap();
        for description in ["Kaufland", "POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR"] {
            let id = resolve_or_create(conn, description).unwrap();
            conn.execute(
                "insert into operation
                    (account_id, role, occurred_at, amount_minor, currency, effect_minor,
                     description, counterparty_id)
                 values (1,'expense','2026-09-01T00:00:00Z',1000,'EUR',-1000,?1,?2)",
                params![description, id],
            )
            .unwrap();
        }
    }

    /// Marking up a counterparty is the rule itself: past operations come under
    /// the category without rows being rewritten, because the query reads it.
    #[test]
    fn categorising_a_counterparty_reaches_the_operations_already_imported() {
        let conn = db::open_memory().unwrap();
        with_operations(&conn);

        // The counterparty's name comes from the first spelling met, here Revolut.
        let affected = set_category(&conn, "Kaufland", "Groceries").unwrap();
        assert_eq!(affected, 2);

        let flows = crate::query::flows(&conn, "2026-09-01", "2026-09-30").unwrap();
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0].label, "Groceries");
        assert_eq!(flows[0].count, 2);

        // Not one operation row rewritten: the category is on the counterparty.
        let own: i64 = conn
            .query_row("select count(*) from operation where category_id is not null", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(own, 0, "the category must not be written into the operations");
    }

    /// An own category that matched the counterparty's new one is stripped —
    /// otherwise the next recategorisation of it would miss those rows.
    #[test]
    fn marking_a_counterparty_absorbs_the_copies_of_that_category() {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into category (name) values ('Food'), ('Home');
             insert into counterparty (name) values ('Kaufland');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id,category_id) values
               (1,'expense','2026-09-01T00:00:00Z',100,'EUR',-100,'Kaufland',1,1),
               (1,'expense','2026-09-02T00:00:00Z',100,'EUR',-100,'Kaufland',1,2),
               (1,'expense','2026-09-03T00:00:00Z',100,'EUR',-100,'Kaufland',1,null);",
        )
        .unwrap();
        assert_eq!(set_category(&conn, "Kaufland", "Food").unwrap(), 2, "the copy of «Food» and the empty one");
        let own: Vec<Option<i64>> = conn
            .prepare("select category_id from operation order by id").unwrap()
            .query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(own, vec![None, Some(2), None], "the «Home» exception kept its own");
        // and now recategorising the counterparty reaches all but the exception
        set_category(&conn, "Kaufland", "Groceries").unwrap();
        let flows = crate::query::flows(&conn, "2026-09-01", "2026-09-30").unwrap();
        let groceries = flows.iter().find(|f| f.label == "Groceries").unwrap();
        assert_eq!(groceries.count, 2);
    }

    #[test]
    fn categorising_someone_who_is_not_there_says_so() {
        let conn = db::open_memory().unwrap();
        let error = set_category(&conn, "No such thing", "Groceries").unwrap_err().to_string();
        assert!(error.contains("potok counterparties"), "{error}");
    }

    /// A merge is what closes the gap normalisation cannot.
    #[test]
    fn merging_moves_aliases_and_operations_and_leaves_one_counterparty() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch("insert into account (name,currency,kind) values ('A','EUR','BANK');")
            .unwrap();
        for description in ["dm drogerie", "POS Payment *1234, DM BULGARIA EOOD SOFIYA BGR"] {
            let id = resolve_or_create(&conn, description).unwrap();
            conn.execute(
                "insert into operation
                    (account_id, role, occurred_at, amount_minor, currency, effect_minor,
                     description, counterparty_id)
                 values (1,'expense','2026-09-01T00:00:00Z',500,'EUR',-500,?1,?2)",
                params![description, id],
            )
            .unwrap();
        }
        let before: i64 =
            conn.query_row("select count(*) from counterparty", [], |r| r.get(0)).unwrap();
        assert_eq!(before, 2, "normalisation does not fold them — that is the case for merge");

        let moved = merge(&mut conn, "DM", "dm drogerie").unwrap();
        assert_eq!(moved, 1);

        let after: i64 =
            conn.query_row("select count(*) from counterparty", [], |r| r.get(0)).unwrap();
        assert_eq!(after, 1);

        // And the next import of the same spelling lands on the folded one.
        let id = resolve_or_create(&conn, "POS Payment *1234, DM BULGARIA EOOD SOFIYA BGR")
            .unwrap()
            .unwrap();
        let name: String = conn
            .query_row("select name from counterparty where id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "dm drogerie", "the alias must lead to the folded one");
    }

    #[test]
    fn merging_something_into_itself_is_refused() {
        let mut conn = db::open_memory().unwrap();
        resolve_or_create(&conn, "Kaufland").unwrap();
        assert!(merge(&mut conn, "Kaufland", "Kaufland").is_err());
    }

    #[test]
    fn merging_moves_debts_along_with_operations() {
        let mut conn = db::open_memory().unwrap();
        resolve_or_create(&conn, "Northwind Old").unwrap();
        resolve_or_create(&conn, "Northwind").unwrap();
        let id = crate::debt::open(
            &mut conn,
            &crate::debt::NewDebt {
                who: "Northwind Old".into(),
                direction: "I_OWE".into(),
                currency: "EUR".into(),
                opened_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                due_on: None,
                opening_minor: 5000,
                note: None,
            },
            None,
        )
        .unwrap();
        merge(&mut conn, "Northwind Old", "Northwind").unwrap();
        let detail = crate::detail::debt(&conn, id).unwrap().unwrap();
        assert_eq!(detail.counterparty, "Northwind");
    }

    /// A hint taken for all at once only where it is near certain.
    #[test]
    fn confident_hints_are_taken_and_the_rest_left() {
        let mut conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into category (name) values ('Food'), ('Other');
             insert into counterparty (name) values ('BILLA'), ('Mixed');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id,category_id) values
               (1,'expense','2026-10-01T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-02T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-03T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-04T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-05T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-06T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-07T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-08T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-09T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-10-10T00:00:00Z',100,'EUR',-100,1,null),
               (1,'expense','2026-10-01T00:00:00Z',100,'EUR',-100,2,1),
               (1,'expense','2026-10-02T00:00:00Z',100,'EUR',-100,2,2);",
        )
        .unwrap();
        let hints = confident_hints(&conn, 90).unwrap();
        assert_eq!(hints, vec![("BILLA".to_string(), "Food".to_string(), 9, 10)], "Mixed is 50/50");
        assert_eq!(take_confident_hints(&mut conn, 90).unwrap(), 1);
        let category: Option<String> = conn
            .query_row("select c.name from counterparty p join category c on c.id = p.category_id where p.name = 'BILLA'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(category.as_deref(), Some("Food"));
        assert!(confident_hints(&conn, 90).unwrap().is_empty());
    }
}
