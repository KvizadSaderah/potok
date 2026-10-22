//! Exchange rates — typed by hand or fetched — and totals in one currency.
//!
//! A rate is stored for a day; a conversion uses the latest one on or before
//! that day, either way round, and through EUR when there is no direct pair.
//! A currency with no rate is not guessed — the total says which is missing.
//!
//! Fetching is on request (and once a day if the setting is on): the European
//! Central Bank for EUR against the world, the Bank of Russia for RUB, which
//! the ECB stopped publishing in 2022. Through `curl`, which every Mac and
//! Linux has, rather than an HTTP stack in the binary for two GETs a day.
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{tr, Currency, Money};

const SCALE: i128 = 1_000_000_000;

/// `0.0098` → rate in billionths. Also `4.63812E-05`, which the Bank of
/// Russia writes for currencies worth a fraction of a kopeck — shifted as a
/// decimal string, not through a float.
pub fn parse(text: &str) -> Result<i64> {
    let text = text.trim().replace(',', ".");
    let text = match text.to_uppercase().split_once('E') {
        Some((mantissa, exponent)) => {
            let exponent: i32 = exponent.parse().map_err(|_| anyhow::anyhow!(tr!("not a rate: «{text}»", "не похоже на курс: «{text}»")))?;
            if !(-18..=18).contains(&exponent) || mantissa.len() > 32 {
                bail!(tr!("not a rate: «{text}»", "не похоже на курс: «{text}»"));
            }
            let (whole, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
            let digits = format!("{whole}{frac}");
            let point = whole.len() as i32 + exponent;
            let plain = if point <= 0 {
                format!("0.{}{digits}", "0".repeat((-point) as usize))
            } else if point as usize >= digits.len() {
                format!("{digits}{}", "0".repeat(point as usize - digits.len()))
            } else {
                format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
            };
            // Beyond nine places a rate rounds to nothing here; keep nine.
            match plain.split_once('.') {
                Some((w, f)) if f.len() > 9 => format!("{w}.{}", &f[..9]),
                _ => plain,
            }
        }
        None => text,
    };
    let (whole, frac) = text.split_once('.').unwrap_or((&text, ""));
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if !digits(whole) || (!frac.is_empty() && !digits(frac)) || frac.len() > 9 {
        bail!(tr!("not a rate: «{text}» — something like 0.0098", "не похоже на курс: «{text}» — вроде 0.0098"));
    }
    let whole_val = whole
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!(tr!("not a rate: «{text}» — something like 0.0098", "не похоже на курс: «{text}» — вроде 0.0098")))?;
    let frac_val = format!("{frac:0<9}")
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!(tr!("not a rate: «{text}» — something like 0.0098", "не похоже на курс: «{text}» — вроде 0.0098")))?;
    let value = whole_val
        .checked_mul(SCALE as i64)
        .and_then(|w| w.checked_add(frac_val))
        .ok_or_else(|| anyhow::anyhow!(tr!("not a rate: «{text}» — something like 0.0098", "не похоже на курс: «{text}» — вроде 0.0098")))?;
    if value <= 0 {
        bail!(tr!("a rate is more than zero", "курс — больше нуля"));
    }
    Ok(value)
}

pub fn show(rate_e9: i64) -> String {
    let text = format!("{}.{:09}", rate_e9 / SCALE as i64, rate_e9 % SCALE as i64);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 1 `from` = `rate` `to`, from `on` onwards.
pub fn set(conn: &Connection, from: Currency, to: Currency, rate_e9: i64, on: &str) -> Result<()> {
    if from == to {
        bail!(tr!("a rate is between two different currencies", "курс — между двумя разными валютами"));
    }
    crate::db::atomic(conn, || {
        conn.execute(
            "insert into rate (on_date, base, quote, rate_e9, source) values (?1, ?2, ?3, ?4, 'manual')
             on conflict (on_date, base, quote) do update set rate_e9 = excluded.rate_e9, source = excluded.source",
            params![on, from.as_str(), to.as_str(), rate_e9],
        )?;
        crate::journal::log(
            conn,
            &tr!("rate 1 {from} = {} {to} from {on}", "курс 1 {from} = {} {to} с {on}", show(rate_e9)),
            None,
        )?;
        Ok(())
    })
}

/// How many `to` one `from` is worth on `on`: the latest rate on or before
/// that day, stored either way round.
pub fn rate(conn: &Connection, from: Currency, to: Currency, on: &str) -> Result<Option<i128>> {
    if from == to {
        return Ok(Some(SCALE));
    }
    // The latest on or before the day; failing that, the nearest after it,
    // up to three days. The Bank of Russia publishes tomorrow's rate today
    // and dates it tomorrow — and the ECB has not quoted RUB since 2022 — so
    // on the evening of the 19th the only RUB rate there is says the 20th,
    // and «on or before» alone found none.
    let latest = |base: Currency, quote: Currency| -> Result<Option<(String, i64)>> {
        Ok(conn
            .query_row(
                "select on_date, rate_e9 from rate
                  where base = ?1 and quote = ?2 and on_date <= date(?3, '+3 days')
                  order by on_date <= ?3 desc,
                           case when on_date <= ?3 then on_date end desc,
                           on_date asc
                  limit 1",
                params![base.as_str(), quote.as_str(), on],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    };
    let pair = |from: Currency, to: Currency| -> Result<Option<i128>> {
        let direct = latest(from, to)?;
        let inverse = latest(to, from)?;
        // Either way round, the one on or before the day wins over one after
        // it — a feed's tomorrow-dated rate must not override one typed for
        // today — and between two of a kind, the later.
        let rank = |day: &String| (day.as_str() <= on, day.clone());
        Ok(match (direct, inverse) {
            (Some((d, r)), Some((i, _))) if rank(&d) >= rank(&i) => Some(r as i128),
            (_, Some((_, r))) => Some(SCALE * SCALE / r as i128),
            (Some((_, r)), None) => Some(r as i128),
            (None, None) => None,
        })
    };
    if let Some(rate) = pair(from, to)? {
        return Ok(Some(rate));
    }
    // Through EUR: the ECB quotes everything against it, the Bank of Russia
    // quotes EUR too, so RUB → USD is RUB → EUR → USD.
    let eur = Currency::new("EUR").expect("EUR is a code");
    if from == eur || to == eur {
        return Ok(None);
    }
    Ok(match (pair(from, eur)?, pair(eur, to)?) {
        (Some(a), Some(b)) => Some(a * b / SCALE),
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub from: Currency,
    pub to: Currency,
    /// How many `to` one `from` is worth today; `None` — no rate.
    pub rate: Option<i64>,
    /// The stored rows it comes from: (from, to, day, source) — what can be
    /// deleted if it is wrong.
    pub stored: Vec<(String, String, String, String)>,
}

/// What matters for this ledger: every account currency against the base.
pub fn table(conn: &Connection, on: &str) -> Result<Vec<Row>> {
    let base = crate::settings::base_currency(conn)?;
    let mut currencies: Vec<Currency> = crate::query::balances(conn)?.iter().map(|a| a.balance.currency()).collect();
    currencies.sort();
    currencies.dedup();
    let mut out = Vec::new();
    for from in currencies.into_iter().filter(|c| *c != base) {
        let mut stmt = conn.prepare(
            "select base, quote, on_date, source from rate
              where ((base = ?1 and quote = ?2) or (base = ?2 and quote = ?1)
                     or (base = ?1 and quote = 'EUR') or (base = 'EUR' and quote = ?1)
                     or (base = ?2 and quote = 'EUR') or (base = 'EUR' and quote = ?2))
                and on_date <= ?3
              order by on_date desc limit 4",
        )?;
        let stored = stmt
            .query_map(params![from.as_str(), base.as_str(), on], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let rate = rate(conn, from, base, on)?.map(|r| r as i64);
        out.push(Row { from, to: base, rate, stored });
    }
    Ok(out)
}

/// Delete one rate — a typo, or a day the feed got wrong.
pub fn delete(conn: &Connection, from: &str, to: &str, on: &str) -> Result<()> {
    crate::db::atomic(conn, || {
        let gone = conn.execute("delete from rate where base = ?1 and quote = ?2 and on_date = ?3", params![from, to, on])?;
        if gone == 0 {
            bail!(tr!("no such rate", "такого курса нет"));
        }
        crate::journal::log(conn, &tr!("rate {from}→{to} of {on} deleted", "курс {from}→{to} от {on} удалён"), None)?;
        Ok(())
    })
}

/// Rates from a feed: (from, to, rate in billionths, day).
type Fetched = Vec<(String, String, i64, String)>;

/// ECB daily reference rates: 1 EUR = rate X, on the `time` day.
pub fn parse_ecb(xml: &str) -> Result<Fetched> {
    let day = attr(xml, "time").ok_or_else(|| anyhow::anyhow!("ECB: no date"))?;
    let day = chrono::NaiveDate::parse_from_str(&day, "%Y-%m-%d")
        .map_err(|_| anyhow::anyhow!("ECB: invalid date"))?
        .to_string();
    let mut out = Vec::new();
    for piece in xml.split("<Cube currency=").skip(1) {
        let piece = format!("currency={piece}");
        if let (Some(code), Some(rate)) = (attr(&piece, "currency"), attr(&piece, "rate")) {
            if Currency::new(&code).is_ok() {
                out.push(("EUR".to_string(), code, parse(&rate)?, day.clone()));
            }
        }
    }
    if out.is_empty() {
        bail!("ECB: no rates in the answer");
    }
    Ok(out)
}

/// Bank of Russia daily rates: 1 X = VunitRate RUB, on the `Date` day.
pub fn parse_cbr(xml: &str) -> Result<Fetched> {
    let date = attr(xml, "Date").ok_or_else(|| anyhow::anyhow!("CBR: no date"))?;
    let day = chrono::NaiveDate::parse_from_str(&date, "%d.%m.%Y")?.to_string();
    let mut out = Vec::new();
    for piece in xml.split("<Valute ").skip(1) {
        let (Some(code), Some(rate)) = (tag(piece, "CharCode"), tag(piece, "VunitRate").or_else(|| tag(piece, "Value"))) else {
            continue;
        };
        if Currency::new(&code).is_err() {
            continue;
        }
        // One unreadable currency is skipped, not the whole answer.
        if let Ok(rate) = parse(&rate) {
            out.push((code, "RUB".to_string(), rate, day.clone()));
        }
    }
    if out.is_empty() {
        bail!("CBR: no rates in the answer");
    }
    Ok(out)
}

fn attr(text: &str, name: &str) -> Option<String> {
    let at = text.find(&format!("{name}="))? + name.len() + 1;
    let quote = text[at..].chars().next()?;
    let rest = &text[at + quote.len_utf8()..];
    Some(rest[..rest.find(quote)?].to_string())
}

fn tag(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let at = text.find(&open)? + open.len();
    let rest = &text[at..];
    Some(rest[..rest.find('<')?].trim().to_string())
}

/// What a fetch brought.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Updated {
    pub stored: usize,
    /// Feeds that failed, with why — a fetch is never all or nothing.
    pub failed: Vec<String>,
}

fn get(url: &str) -> Result<String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "20", url])
        .output()
        .map_err(|e| anyhow::anyhow!(tr!("curl did not start: {e}", "curl не запустился: {e}")))?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    // The Bank of Russia answers in windows-1251; only ASCII tags and numbers
    // are read, so a lossy decode loses nothing that is used.
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Store fetched rates, one journal line for the lot.
pub fn store(conn: &Connection, rates: &Fetched, source: &str) -> Result<usize> {
    crate::db::atomic(conn, || {
        let mut stmt = conn.prepare(
            "insert into rate (on_date, base, quote, rate_e9, source) values (?1, ?2, ?3, ?4, ?5)
             on conflict (on_date, base, quote) do update set rate_e9 = excluded.rate_e9, source = excluded.source",
        )?;
        for (from, to, rate, day) in rates {
            stmt.execute(params![day, from, to, rate, source])?;
        }
        Ok(rates.len())
    })
}

/// Fetch the ECB and the Bank of Russia and store what came.
pub fn update(conn: &Connection) -> Result<Updated> {
    let mut out = Updated::default();
    for (source, url, parser) in [
        ("ecb", "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml", parse_ecb as fn(&str) -> Result<Fetched>),
        ("cbr", "https://www.cbr.ru/scripts/XML_daily.asp", parse_cbr as fn(&str) -> Result<Fetched>),
    ] {
        match get(url).and_then(|xml| parser(&xml)) {
            Ok(rates) => out.stored += store(conn, &rates, source)?,
            Err(error) => out.failed.push(format!("{source}: {error}")),
        }
    }
    if out.stored > 0 {
        crate::settings::set(conn, crate::settings::RATES_FETCHED, &chrono::Local::now().date_naive().to_string())?;
        crate::journal::log(
            conn,
            &tr!("rates fetched: {}{}", "курсы загружены: {}{}", out.stored, if out.failed.is_empty() { String::new() } else { format!(" ({})", out.failed.join("; ")) }),
            None,
        )?;
    }
    Ok(out)
}

/// Fetch if the setting asks for it and the last fetch was not today.
pub fn update_if_due(conn: &Connection, today: chrono::NaiveDate) -> Result<Option<Updated>> {
    let auto = crate::settings::get(conn, crate::settings::RATES_AUTO)?.as_deref() == Some("on");
    let last = crate::settings::get(conn, crate::settings::RATES_FETCHED)?;
    if !auto || last.as_deref() == Some(today.to_string().as_str()) {
        return Ok(None);
    }
    update(conn).map(Some)
}

/// `money` in `to`, rounded half away from zero; `None` — no rate.
pub fn convert(conn: &Connection, money: Money, to: Currency, on: &str) -> Result<Option<Money>> {
    let Some(rate) = rate(conn, money.currency(), to, on)? else { return Ok(None) };
    let raw = money.minor() as i128 * rate;
    let rounded = (raw + raw.signum() * SCALE / 2) / SCALE;
    Ok(Some(Money::new(rounded as i64, to)))
}

/// Convert every currency in `bag` that has a rate to `to` on `on`, summing in
/// full precision (`i128`) before a single rounding at the end (§2).
/// Returns `(converted_sum, missing_currencies)`.
pub fn convert_bag_partial(
    conn: &Connection,
    bag: &crate::MoneyBag,
    to: Currency,
    on: &str,
) -> Result<(Money, Vec<Currency>)> {
    let mut raw_sum = 0i128;
    let mut missing = Vec::new();
    for money in bag.in_display_order(None) {
        if money.minor() == 0 {
            continue;
        }
        match rate(conn, money.currency(), to, on)? {
            Some(r) => raw_sum += money.minor() as i128 * r,
            None => missing.push(money.currency()),
        }
    }
    let rounded = (raw_sum + raw_sum.signum() * SCALE / 2) / SCALE;
    Ok((Money::new(rounded as i64, to), missing))
}

/// Convert a `MoneyBag` to `to` on `on`, rounding once at the end; `None` if
/// any non-zero currency in the bag has no rate.
pub fn convert_bag(
    conn: &Connection,
    bag: &crate::MoneyBag,
    to: Currency,
    on: &str,
) -> Result<Option<Money>> {
    let (converted, missing) = convert_bag_partial(conn, bag, to, on)?;
    if missing.is_empty() {
        Ok(Some(converted))
    } else {
        Ok(None)
    }
}

/// Comparable magnitude of `bag` in the ledger's base currency on `on` (for
/// sorting only, never displayed). Currencies with a rate are converted in
/// full precision; currencies without a rate fall back to their own minor
/// units so unrated rows still order among themselves.
pub fn rank_bag_minor(conn: &Connection, bag: &crate::MoneyBag, on: &str) -> i64 {
    let Ok(base) = crate::settings::base_currency(conn) else {
        return bag.in_display_order(None).iter().map(|m| m.minor()).sum();
    };
    let mut raw_sum = 0i128;
    let mut unrated_minor = 0i64;
    for money in bag.in_display_order(None) {
        if money.minor() == 0 {
            continue;
        }
        match rate(conn, money.currency(), base, on).ok().flatten() {
            Some(r) => raw_sum += money.minor() as i128 * r,
            None => unrated_minor += money.minor(),
        }
    }
    let converted = (raw_sum + raw_sum.signum() * SCALE / 2) / SCALE;
    converted as i64 + unrated_minor
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Total {
    pub total: Money,
    /// Currencies held with no rate to the base — left out, and said so.
    pub missing: Vec<Currency>,
}

/// Everything on active, non-excluded accounts in the base currency, rounded once at the end (§2).
pub fn total(conn: &Connection, on: &str) -> Result<Total> {
    let base = crate::settings::base_currency(conn)?;
    let mut raw_sum = 0i128;
    let mut missing = Vec::new();
    for account in crate::query::balances(conn)?.into_iter().filter(|a| a.inclusion != "EXCLUDED") {
        match rate(conn, account.balance.currency(), base, on)? {
            Some(r) => raw_sum += account.balance.minor() as i128 * r,
            None if account.balance.minor() == 0 => {}
            None => {
                let c = account.balance.currency();
                if !missing.contains(&c) {
                    missing.push(c);
                }
            }
        }
    }
    let sum = (raw_sum + raw_sum.signum() * SCALE / 2) / SCALE;
    Ok(Total { total: Money::new(sum as i64, base), missing })
}

/// The latest rate of every pair, newest first: (from, to, rate, since).
pub fn latest(conn: &Connection) -> Result<Vec<(String, String, i64, String)>> {
    let mut stmt = conn.prepare(
        "select base, quote, rate_e9, on_date from rate r
          where on_date = (select max(on_date) from rate x where x.base = r.base and x.quote = r.quote)
          order by on_date desc, base",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(code: &str) -> Currency {
        Currency::new(code).unwrap()
    }

    #[test]
    fn totals_convert_by_the_latest_rate_either_way_round() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('Revolut','EUR','BANK',10000), ('Tinkoff','RUB','BANK',1000000), ('Wise','USD','BANK',500);",
        )
        .unwrap();
        assert_eq!(parse("0.0098").unwrap(), 9_800_000);
        assert_eq!(show(9_800_000), "0.0098");
        assert!(parse("abc").is_err() && parse("0").is_err() && parse("1.0000000001").is_err());
        assert_eq!(parse("4,63812E-05").unwrap(), 46_381);
        assert_eq!(parse("1.5E2").unwrap(), 150_000_000_000);
        assert!(parse("1E-12").is_err(), "too small to keep");

        let t = total(&conn, "2026-10-19").unwrap();
        assert_eq!((t.total.minor(), t.missing.len()), (10000, 2), "no rates — said, not guessed");

        set(&conn, c("RUB"), c("EUR"), parse("0.01").unwrap(), "2026-10-01").unwrap();
        set(&conn, c("EUR"), c("USD"), parse("1.25").unwrap(), "2026-10-01").unwrap();
        let t = total(&conn, "2026-10-19").unwrap();
        assert_eq!(t.missing, Vec::<Currency>::new());
        assert_eq!(t.total.minor(), 10000 + 10000 + 400, "100 EUR + 10000 RUB·0.01 + 5 USD/1.25");
        assert_eq!(total(&conn, "2026-09-20").unwrap().missing.len(), 2, "no rate long before it was set");
        assert_eq!(total(&conn, "2026-09-30").unwrap().missing, Vec::<Currency>::new(), "a rate a day ahead stands in");

        // The Bank of Russia's evening rate is dated tomorrow, and it is the
        // only RUB rate there is: today takes it. A rate from today, once
        // there is one, wins over it.
        let conn = crate::db::open_memory().unwrap();
        set(&conn, c("EUR"), c("RUB"), parse("95.5").unwrap(), "2026-10-20").unwrap();
        assert_eq!(rate(&conn, c("RUB"), c("EUR"), "2026-10-19").unwrap(), Some(SCALE * SCALE / 95_500_000_000));
        set(&conn, c("EUR"), c("RUB"), parse("94").unwrap(), "2026-10-19").unwrap();
        assert_eq!(rate(&conn, c("EUR"), c("RUB"), "2026-10-19").unwrap(), Some(94_000_000_000), "today's own rate first");
        assert!(set(&conn, c("EUR"), c("EUR"), 1, "2026-10-01").is_err());

        // Typed today one way round, the feed's tomorrow the other way: the
        // one for today wins, whichever side it is stored on.
        let conn = crate::db::open_memory().unwrap();
        set(&conn, c("RUB"), c("EUR"), parse("0.0098").unwrap(), "2026-10-19").unwrap();
        set(&conn, c("EUR"), c("RUB"), parse("90").unwrap(), "2026-10-20").unwrap();
        assert_eq!(rate(&conn, c("RUB"), c("EUR"), "2026-10-19").unwrap(), Some(9_800_000), "today's typed rate, not tomorrow's inverse");
    }

    #[test]
    fn feeds_are_read_and_a_pair_goes_through_eur() {
        let ecb = "<Cube><Cube time='2026-10-16'><Cube currency='USD' rate='1.1253'/><Cube currency='GBP' rate='0.84385'/></Cube></Cube>";
        let cbr = "<?xml?><ValCurs Date=\"17.10.2026\" name=\"x\"><Valute ID=\"R1\"><CharCode>EUR</CharCode><Nominal>1</Nominal><Value>91,5</Value><VunitRate>91,5</VunitRate></Valute>\
                   <Valute ID=\"R2\"><CharCode>DZD</CharCode><Nominal>100</Nominal><Value>62,2</Value><VunitRate>0,622</VunitRate></Valute></ValCurs>";
        let e = parse_ecb(ecb).unwrap();
        assert_eq!(e[0], ("EUR".into(), "USD".into(), 1_125_300_000, "2026-10-16".into()));
        let r = parse_cbr(cbr).unwrap();
        assert_eq!(r[1], ("DZD".into(), "RUB".into(), 622_000_000, "2026-10-17".into()), "per one unit, not per nominal");
        assert!(parse_ecb("<html>").is_err() && parse_cbr("nonsense").is_err());

        let conn = crate::db::open_memory().unwrap();
        store(&conn, &e, "ecb").unwrap();
        store(&conn, &r, "cbr").unwrap();
        let rub = Money::new(915_000, c("RUB"));
        let usd = convert(&conn, rub, c("USD"), "2026-10-19").unwrap().unwrap();
        assert_eq!(usd.minor(), 11253, "9150 RUB = 100 EUR = 112.53 USD");
        delete(&conn, "EUR", "RUB", "2026-10-17").unwrap();
        assert!(convert(&conn, rub, c("USD"), "2026-10-19").unwrap().is_none());
    }

    /// §2: rounding every account/currency in between before adding gives an
    /// error. Two remainders of 0.4 cent add to 0.8 cent → 1 cent, not 0.
    #[test]
    fn summing_converted_amounts_rounds_once_at_the_end() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,opening_minor) values ('A','RUB','BANK',100), ('B','RUB','BANK',100);",
        )
        .unwrap();
        set(&conn, c("RUB"), c("EUR"), parse("0.004").unwrap(), "2026-10-01").unwrap();
        assert_eq!(convert(&conn, Money::new(100, c("RUB")), c("EUR"), "2026-10-19").unwrap().unwrap().minor(), 0);
        assert_eq!(total(&conn, "2026-10-19").unwrap().total.minor(), 1, "0.4 + 0.4 = 0.8 цента → 1 цент");
    }

    #[test]
    fn astronomical_exponents_overflows_and_excluded_accounts_are_handled_safely() {
        assert!(parse("1e2000000000").is_err());
        assert!(parse("1e-2000000000").is_err());
        assert!(parse("99999999999.9").is_err(), "overflows i64 * SCALE");
        assert_eq!(attr("time=«2026-10-16»", "time"), None, "multibyte non-matching quote does not panic");
        assert!(parse_ecb("<Cube time='not-a-date'><Cube currency='USD' rate='1.1'/></Cube>").is_err());

        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind,inclusion,opening_minor) values
               ('Main','EUR','BANK','FULL',10000),
               ('Savings','EUR','SAVINGS','OFF_BUDGET',20000),
               ('Held','EUR','OTHER','EXCLUDED',99900);",
        )
        .unwrap();
        assert_eq!(total(&conn, "2026-10-19").unwrap().total.minor(), 30000, "EXCLUDED stays out of total");
    }
}
