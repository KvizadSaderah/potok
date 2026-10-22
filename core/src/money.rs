//! Money is a whole number of minor units and a currency. Never `f64`.
//!
//! The specification (§2) asks that adding amounts in different currencies
//! be impossible. That is done here not by a phantom type parameter — the
//! currency arrives from the database at run time, and such a parameter would
//! have to be erased at once — but by **the absence of the operation**:
//! `Money` has no `impl Add`.
//!
//! Only like with like adds up (`try_add`, an error when they differ), or
//! through `MoneyBag`, which by construction keeps a separate total per
//! currency. «Adding different currencies into one heap» is then not a
//! mistake to be prevented but code that cannot be written.
//!
//! There is no rounding here at all. It lives in `Display` and only there (§2).
use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency([u8; 3]);

#[derive(Debug, PartialEq, Eq)]
pub enum MoneyError {
    BadCurrency(String),
    Mismatch { left: Currency, right: Currency },
    Unparsable(String),
}

impl std::error::Error for MoneyError {}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            MoneyError::BadCurrency(code) => {
                crate::tr!("{code:?} is not an ISO 4217 currency code", "{code:?} не похоже на код валюты ISO 4217")
            }
            MoneyError::Mismatch { left, right } => crate::tr!("cannot add {left} and {right}", "нельзя складывать {left} и {right}"),
            MoneyError::Unparsable(text) => crate::tr!("cannot read the amount {text:?}", "сумма {text:?} не разбирается"),
        };
        f.write_str(&text)
    }
}

impl Currency {
    pub fn new(code: &str) -> Result<Self, MoneyError> {
        let bytes = code.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(|b| b.is_ascii_alphabetic()) {
            return Err(MoneyError::BadCurrency(code.to_string()));
        }
        let mut out = [0u8; 3];
        for (slot, b) in out.iter_mut().zip(bytes) {
            *slot = b.to_ascii_uppercase();
        }
        Ok(Self(out))
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("three ASCII letters by construction")
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money {
    minor: i64,
    currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    pub fn zero(currency: Currency) -> Self {
        Self { minor: 0, currency }
    }

    pub fn minor(&self) -> i64 {
        self.minor
    }

    pub fn currency(&self) -> Currency {
        self.currency
    }

    pub fn abs(&self) -> Self {
        Self { minor: self.minor.abs(), currency: self.currency }
    }

    pub fn negate(&self) -> Self {
        Self { minor: -self.minor, currency: self.currency }
    }

    /// The only way to add two amounts. Different currencies are an error,
    /// not a zero and not a silent conversion.
    pub fn try_add(&self, other: Money) -> Result<Money, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::Mismatch { left: self.currency, right: other.currency });
        }
        Ok(Money { minor: self.minor + other.minor, currency: self.currency })
    }

    /// Reads what banks print: `€23.67`, `-2.40`, `4,873.40`, `1 234,56`.
    ///
    /// The currency is not guessed from the symbol — the caller passes it,
    /// because the caller knows which account the row belongs to. A symbol in
    /// the text is merely dropped.
    pub fn parse(text: &str, currency: Currency) -> Result<Money, MoneyError> {
        let mut digits = String::with_capacity(text.len());
        let mut negative = false;
        let mut seen_digit = false;
        for ch in text.chars() {
            match ch {
                '-' | '\u{2212}' if !seen_digit => negative = true,
                '0'..='9' => {
                    seen_digit = true;
                    digits.push(ch);
                }
                '.' | ',' => digits.push('.'),
                _ => {}
            }
        }
        if !seen_digit {
            return Err(MoneyError::Unparsable(text.to_string()));
        }
        // The last separator is the decimal one when one or two digits follow
        // it. When three do («1,234») it is a thousands group, provided every
        // group is well formed. Anything else («5.0.1», «12.», «12.3456») is
        // refused rather than multiplied by a hundred.
        let valid_thousands = |s: &str| -> bool {
            let mut groups = s.split('.');
            let Some(first) = groups.next() else { return false };
            (1..=3).contains(&first.len())
                && first.chars().all(|c| c.is_ascii_digit())
                && groups.all(|g| g.len() == 3 && g.chars().all(|c| c.is_ascii_digit()))
        };
        let (whole, frac) = match digits.rfind('.') {
            Some(at) if matches!(digits.len() - at - 1, 1 | 2) => {
                let before = &digits[..at];
                let ok = if before.contains('.') {
                    valid_thousands(before)
                } else {
                    !before.is_empty() && before.chars().all(|c| c.is_ascii_digit())
                };
                if !ok {
                    return Err(MoneyError::Unparsable(text.to_string()));
                }
                (before, &digits[at + 1..])
            }
            Some(_) if valid_thousands(&digits) => (digits.as_str(), ""),
            Some(_) => return Err(MoneyError::Unparsable(text.to_string())),
            None => (digits.as_str(), ""),
        };
        let whole: String = whole.chars().filter(|c| c.is_ascii_digit()).collect();
        let units: i64 = whole.parse().map_err(|_| MoneyError::Unparsable(text.to_string()))?;
        let cents: i64 = if frac.is_empty() {
            0
        } else {
            format!("{frac:0<2}").parse().map_err(|_| MoneyError::Unparsable(text.to_string()))?
        };
        let minor = units
            .checked_mul(100)
            .and_then(|u| u.checked_add(cents))
            .ok_or_else(|| MoneyError::Unparsable(text.to_string()))?;
        Ok(Money { minor: if negative { -minor } else { minor }, currency })
    }
}

/// A total over unlike amounts: a separate number per currency.
///
/// A screen printing one total under one symbol either deals in a single
/// currency or lies. This type does not let it lie.
#[derive(Debug, Clone, Default)]
pub struct MoneyBag {
    totals: BTreeMap<Currency, i64>,
}

impl MoneyBag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, amount: Money) {
        *self.totals.entry(amount.currency).or_insert(0) += amount.minor;
    }

    /// Pours in another bag: every currency into its own.
    pub fn absorb(&mut self, other: &MoneyBag) {
        for (currency, minor) in &other.totals {
            *self.totals.entry(*currency).or_insert(0) += minor;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.totals.values().all(|v| *v == 0)
    }

    /// The currencies in order, the base one first: a single-currency ledger
    /// reads as it did, and a mixed one leads with the currency the person
    /// thinks in.
    pub fn in_display_order(&self, base: Option<Currency>) -> Vec<Money> {
        let mut out: Vec<Money> =
            self.totals.iter().map(|(c, m)| Money::new(*m, *c)).collect();
        if let Some(base) = base {
            out.sort_by_key(|m| (m.currency() != base, m.currency()));
        }
        out
    }
}

/// An exact decimal string from minor units, without a currency: `-4873.40`.
/// No thousands groups and no `f64`.
pub fn format_minor(minor: i64) -> String {
    let negative = minor < 0;
    let abs = minor.unsigned_abs();
    format!("{}{}.{:02}", if negative { "-" } else { "" }, abs / 100, abs % 100)
}

impl Money {
    /// An exact decimal string for a machine: `-4873.40`. No thousands
    /// groups, no currency symbol, no `f64` anywhere on the way.
    ///
    /// An agent needs it (§6): a floating-point number in JSON reads back as
    /// `4873.4`, and adding such numbers is the very mistake everything here
    /// is built against. A string beside the whole minor units leaves no room
    /// for guessing. This is formatting, not rounding: minor units are whole
    /// already.
    pub fn decimal(&self) -> String {
        format_minor(self.minor)
    }
}

/// The only place where rounding is allowed (§2).
impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let negative = self.minor < 0;
        let abs = self.minor.unsigned_abs();
        let units = abs / 100;
        let cents = abs % 100;
        // Groups of three with a non-breaking space: in a terminal a column of
        // digits reads only when the groups are the same width.
        let mut grouped = String::new();
        let digits = units.to_string();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                grouped.push('\u{202f}');
            }
            grouped.push(ch);
        }
        let s = format!(
            "{}{}.{:02} {}",
            if negative { "-" } else { "" },
            grouped,
            cents,
            self.currency
        );
        f.pad(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eur() -> Currency {
        Currency::new("eur").unwrap()
    }

    #[test]
    fn a_currency_is_three_letters_or_nothing() {
        assert_eq!(eur().as_str(), "EUR");
        assert!(Currency::new("EURO").is_err());
        assert!(Currency::new("E1R").is_err());
        assert!(Currency::new("").is_err());
    }

    /// What banks print, in every spelling met so far.
    #[test]
    fn what_a_bank_prints_parses_to_whole_minor_units() {
        let c = eur();
        assert_eq!(Money::parse("€23.67", c).unwrap().minor(), 2367);
        assert_eq!(Money::parse("-2.40", c).unwrap().minor(), -240);
        assert_eq!(Money::parse("4,873.40", c).unwrap().minor(), 487340);
        assert_eq!(Money::parse("1 234,56", c).unwrap().minor(), 123456);
        assert_eq!(Money::parse("€0.80", c).unwrap().minor(), 80);
        assert_eq!(Money::parse("100", c).unwrap().minor(), 10000);
        assert!(Money::parse("EUR", c).is_err());
    }

    /// A thousands group must not become cents: `1,234` is one thousand two
    /// hundred thirty-four, not twelve point thirty-four.
    #[test]
    fn a_thousands_separator_is_not_a_decimal_point() {
        let c = eur();
        assert_eq!(Money::parse("1,234", c).unwrap().minor(), 123400);
        assert_eq!(Money::parse("1,234.56", c).unwrap().minor(), 123456);
        assert_eq!(Money::parse("12.5", c).unwrap().minor(), 1250, "one digit after the point is tens of cents, not thousands");
        assert_eq!(Money::parse("507,1", c).unwrap().minor(), 50710);
        for bad in ["5.0.1", "12.", ".5", "12.3456", "1234,567"] {
            assert!(Money::parse(bad, c).is_err(), "{bad}");
        }
    }

    #[test]
    fn money_of_two_currencies_refuses_to_add() {
        let euros = Money::new(100, eur());
        let dollars = Money::new(100, Currency::new("USD").unwrap());
        assert!(euros.try_add(dollars).is_err());
        assert_eq!(euros.try_add(euros).unwrap().minor(), 200);
    }

    /// This is the bug the type exists for: in the old system amounts
    /// in different currencies added up into one figure under one symbol.
    #[test]
    fn a_bag_keeps_every_currency_apart() {
        let mut bag = MoneyBag::new();
        bag.add(Money::new(50000, eur()));
        bag.add(Money::new(19000, Currency::new("USD").unwrap()));
        bag.add(Money::new(10000, eur()));
        let totals = bag.in_display_order(Some(eur()));
        assert_eq!(totals.len(), 2);
        assert_eq!(totals[0].currency(), eur(), "the base currency comes first");
        assert_eq!(totals[0].minor(), 60000);
        assert_eq!(totals[1].minor(), 19000);
    }

    #[test]
    fn the_machine_form_is_exact_and_ungrouped() {
        assert_eq!(Money::new(487340, eur()).decimal(), "4873.40");
        assert_eq!(Money::new(-240, eur()).decimal(), "-2.40");
        assert_eq!(Money::new(-5, eur()).decimal(), "-0.05", "the minus on an amount below one");
        assert_eq!(Money::new(0, eur()).decimal(), "0.00");
        assert_eq!(Money::new(i64::MIN + 1, eur()).decimal(), "-92233720368547758.07");
    }

    #[test]
    fn display_groups_thousands_and_always_shows_two_places() {
        assert_eq!(Money::new(487340, eur()).to_string(), "4\u{202f}873.40 EUR");
        assert_eq!(Money::new(-240, eur()).to_string(), "-2.40 EUR");
        assert_eq!(Money::new(5, eur()).to_string(), "0.05 EUR");
        assert_eq!(Money::new(0, eur()).to_string(), "0.00 EUR");
    }
}
