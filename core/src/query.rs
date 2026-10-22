//! Reading: balances and flows. The one place where totals are computed.
//!
//! A total is always a `MoneyBag`, never a number: the ledger is multi-currency,
//! and a screen printing one figure under one symbol either deals with one
//! currency or lies. No conversion happens here — that needs a rate for the
//! date, and without a rate a sum is not shown at all (§2).
use anyhow::Result;
use rusqlite::Connection;

use crate::money::{Currency, Money, MoneyBag};

#[derive(Debug, Clone)]
pub struct AccountBalance {
    pub id: i64,
    pub name: String,
    pub balance: Money,
    /// Posted operations on it — an account with none is one not used yet.
    pub operations: usize,
    pub inclusion: String,
}

pub fn balances(conn: &Connection) -> Result<Vec<AccountBalance>> {
    let mut stmt = conn.prepare(
        "select a.id, a.name, b.currency, b.balance_minor,
                (select count(*) from operation o where o.account_id = a.id and o.state = 'POSTED'),
                a.inclusion
           from account_balance b join account a on a.id = b.account_id
          where a.state = 'ACTIVE'
          -- Accounts with nothing on them last: the list opens on one in use.
          order by not exists (select 1 from operation o where o.account_id = a.id and o.state = 'POSTED'), a.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, name, currency, minor, operations, inclusion) = row?;
        let currency = Currency::new(&currency)?;
        out.push(AccountBalance { id, name, balance: Money::new(minor, currency), operations: operations as usize, inclusion });
    }
    Ok(out)
}

/// Where the money went and where it came from over a period — the point of it.
#[derive(Debug, Clone)]
pub struct Flow {
    /// The operation's category, or failing that its counterparty's, then the
    /// counterparty itself, then the description, and only then «no category».
    ///
    /// The second step is the categorisation rule itself: mark a counterparty
    /// once and all its operations, past and future, come under the category,
    /// because it is read here rather than written into the row. The first step
    /// is left for an exception to the rule on a single operation.
    ///
    /// The fallback to the description is kept for counterparties normalisation
    /// did not recognise: a row with the bank's text beats one big heap.
    pub label: String,
    pub out: MoneyBag,
    pub inn: MoneyBag,
    pub count: usize,
    /// What went out, in the base currency at the rates of the period's last
    /// day; `None` — a currency in it has no rate.
    pub out_base: Option<Money>,
    /// Its part of everything that went out, in percent; `None` when any row
    /// has no rate — a share of a partial total would read as a true one.
    pub share: Option<i64>,
}

/// `from` and `to` are inclusive, in `YYYY-MM-DD`.
/// The name of a flow row. **One expression for everyone** who uses it: for
/// adding the flows up and for drilling in. If drilling in filtered by its own
/// copy of this rule, Enter on «Groceries — 22 items» would one day show the
/// wrong 22 operations.
const FLOW_LABEL: &str = "coalesce(own.name, inherited.name, p.name,
                         nullif(trim(o.description), ''), '—')";

/// Which operations enter the flows for a period: posted income and spends on
/// accounts with full inclusion (`inclusion = 'FULL'`).
/// Assumes the same table aliases as `OPERATION_SELECT`.
const FLOW_FILTER: &str = "o.state = 'POSTED'
            and a.inclusion = 'FULL'
            and o.role in ('income','expense')
            and date(o.occurred_at) between date(?1) and date(?2)";

pub fn flows(conn: &Connection, from: &str, to: &str) -> Result<Vec<Flow>> {
    let sql = format!(
        "select {FLOW_LABEL} as label, o.currency, o.effect_minor, o.role
           from operation o
           join account           a         on a.id         = o.account_id
           left join category     own       on own.id       = o.category_id
           left join counterparty p         on p.id         = o.counterparty_id
           left join category     inherited on inherited.id = p.category_id
          where {FLOW_FILTER}
            and not exists (select 1 from operation_split s where s.operation_id = o.id)
         union all
         -- A split operation enters the flows in parts, each in its own category.
         select c.name, o.currency,
                case when o.effect_minor < 0 then -s.amount_minor else s.amount_minor end,
                o.role
           from operation_split s
           join operation o on o.id = s.operation_id
           join account   a on a.id = o.account_id
           join category  c on c.id = s.category_id
          where {FLOW_FILTER}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([from, to], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;

    // The output order is by descending spend, so collect first, then sort.
    let mut grouped: std::collections::BTreeMap<String, Flow> = Default::default();
    for row in rows {
        let (label, currency, effect, role) = row?;
        let currency = Currency::new(&currency)?;
        let entry = grouped.entry(label.clone()).or_insert_with(|| Flow {
            label,
            out: MoneyBag::new(),
            inn: MoneyBag::new(),
            count: 0,
            out_base: None,
            share: None,
        });
        if effect < 0 {
            entry.out.add(Money::new(-effect, currency));
        } else if role == "expense" {
            // A refund: what went out of this row comes down by it.
            entry.out.add(Money::new(-effect, currency));
        } else {
            entry.inn.add(Money::new(effect, currency));
        }
        entry.count += 1;
    }

    let mut out: Vec<Flow> = grouped.into_values().collect();
    // Biggest first by what it is worth in one currency. Adding up the minor
    // units of different currencies put 15 000 RUB above 1 734 EUR.
    let on = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d").ok();
    for flow in &mut out {
        flow.out_base = match on {
            Some(on) => crate::analytics::to_base(conn, &flow.out, on)?,
            None => None,
        };
    }
    let whole: Option<i64> = out.iter().map(|f| f.out_base.map(|m| m.minor().max(0))).sum();
    if let Some(whole) = whole.filter(|w| *w > 0) {
        for flow in &mut out {
            flow.share = flow.out_base.filter(|m| m.minor() >= 0).map(|m| m.minor() * 100 / whole);
        }
    }
    out.sort_by_key(|f| (f.out_base.is_none(), -f.out_base.map(|m| m.minor()).unwrap_or(0), f.label.clone()));
    Ok(out)
}

/// The totals over all the flows for a period: how much went out and in, by currency.
///
/// Computed here rather than where it is shown — rule §5 holds for the agent
/// too: the core adds money up, the surface only prints.
pub fn flow_totals(flows: &[Flow]) -> (MoneyBag, MoneyBag) {
    let mut out = MoneyBag::new();
    let mut inn = MoneyBag::new();
    for flow in flows {
        out.absorb(&flow.out);
        inn.absorb(&flow.inn);
    }
    (out, inn)
}

/// The operations one flow row was made of — drilling in.
pub fn flow_operations(conn: &Connection, label: &str, from: &str, to: &str) -> Result<Vec<OperationLine>> {
    let sql = format!(
        "{OPERATION_SELECT}
            and {FLOW_FILTER}
            and (({FLOW_LABEL} = ?3
                  and not exists (select 1 from operation_split s where s.operation_id = o.id))
                 or exists (select 1 from operation_split s join category c on c.id = s.category_id
                             where s.operation_id = o.id and c.name = ?3))
          order by o.occurred_at desc, o.id desc"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![from, to, label], operation_line)?;
    rows.map(|row| with_currency(row?)).collect()
}

/// How the spend on a flow row changed against the previous period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// A spend in one currency in both periods: the change in per cent.
    Percent(i64),
    /// Did not spend before — spends now.
    New,
    /// Spent before — does not now.
    Gone,
    /// No spend either side: a row of income only.
    Nothing,
    /// Different currencies: per cent of euro plus leva means nothing.
    NotComparable,
}

#[derive(Debug, Clone)]
pub struct FlowChange {
    pub label: String,
    pub out: MoneyBag,
    pub inn: MoneyBag,
    pub count: usize,
    pub out_before: MoneyBag,
    pub change: Change,
}

/// One currency in the bag — its minor units; zero if the bag is empty.
fn single(bag: &MoneyBag) -> Option<(Option<Currency>, i64)> {
    let all = bag.in_display_order(None);
    match all.as_slice() {
        [] => Some((None, 0)),
        [one] => Some((Some(one.currency()), one.minor())),
        _ => None,
    }
}

fn change(now: &MoneyBag, before: &MoneyBag) -> Change {
    let (Some((now_currency, now)), Some((before_currency, before))) = (single(now), single(before)) else {
        return Change::NotComparable;
    };
    if let (Some(a), Some(b)) = (now_currency, before_currency) {
        if a != b {
            return Change::NotComparable;
        }
    }
    match (now, before) {
        (n, b) if n < 0 || b < 0 => Change::NotComparable,
        (0, 0) => Change::Nothing,
        (_, 0) => Change::New,
        (0, _) => Change::Gone,
        // A per cent is not money: what is rounded here is a share, not a sum.
        _ => Change::Percent((((now - before) as f64 / before as f64) * 100.0).round() as i64),
    }
}

/// The flows for a period beside the previous period.
///
/// Rows that were there before and are gone are here too, at the end: «what I
/// stopped spending on» answers as much as «what I started on».
pub fn compare_flows(
    conn: &Connection,
    (from, to): (&str, &str),
    (before_from, before_to): (&str, &str),
) -> Result<Vec<FlowChange>> {
    let now = flows(conn, from, to)?;
    let before = flows(conn, before_from, before_to)?;
    let mut before_by: std::collections::HashMap<&str, &Flow> =
        before.iter().map(|f| (f.label.as_str(), f)).collect();

    let mut out: Vec<FlowChange> = now
        .iter()
        .map(|flow| {
            let out_before = before_by.remove(flow.label.as_str()).map(|f| f.out.clone()).unwrap_or_default();
            FlowChange {
                label: flow.label.clone(),
                out: flow.out.clone(),
                inn: flow.inn.clone(),
                count: flow.count,
                change: change(&flow.out, &out_before),
                out_before,
            }
        })
        .collect();
    // The gone ones — in the order of the past spend, as they stood then.
    for flow in &before {
        if before_by.contains_key(flow.label.as_str()) && !flow.out.is_empty() {
            out.push(FlowChange {
                label: flow.label.clone(),
                out: MoneyBag::new(),
                inn: MoneyBag::new(),
                count: 0,
                change: Change::Gone,
                out_before: flow.out.clone(),
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct DebtLine {
    pub id: i64,
    pub counterparty: String,
    pub direction: String,
    pub remaining: Money,
    /// How much was lent in all — the sum of the ISSUE movements. Not stored (§3, invariant 2).
    pub issued: Money,
}

/// Debts. The closed ones only if asked: an agent needs the open ones, while a
/// person wanting to see «how I paid» needs the closed ones too.
pub fn debts(conn: &Connection, include_settled: bool) -> Result<Vec<DebtLine>> {
    let mut stmt = conn.prepare(
        "select d.id, p.name, d.direction, d.currency, r.remaining_minor, r.issued_minor
           from debt d
           join counterparty  p on p.id = d.counterparty_id
           join debt_remaining r on r.debt_id = d.id
          where ?1 or r.remaining_minor <> 0
          order by (r.remaining_minor = 0), r.remaining_minor desc",
    )?;
    let rows = stmt.query_map([include_settled], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, counterparty, direction, currency, remaining, issued) = row?;
        let currency = Currency::new(&currency)?;
        out.push(DebtLine {
            id,
            counterparty,
            direction,
            remaining: Money::new(remaining, currency),
            issued: Money::new(issued, currency),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CounterpartyLine {
    pub name: String,
    pub category: Option<String>,
    pub note: Option<String>,
    pub operations: usize,
    pub spent: MoneyBag,
    /// The commonest category among its operations and how many it has — for
    /// when the counterparty itself has none. On the real data Kaufland shows
    /// «—» although 52 of its 60 purchases are already in «Food & Groceries»:
    /// the category did not come across the move whole because of the eight
    /// without it. The hint turns that into one key press.
    pub hint: Option<(String, usize)>,
}

/// Who appears in the ledger, how often and for how much. The list marking
/// starts from: on top are the ones where marking gains the most.
pub fn counterparties(conn: &Connection) -> Result<Vec<CounterpartyLine>> {
    let mut stmt = conn.prepare(
        "select p.name, c.name, o.currency, o.effect_minor, o.role, p.note
           from counterparty p
           left join category  c on c.id = p.category_id
           left join operation o on o.counterparty_id = p.id and o.state = 'POSTED'
          order by p.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut grouped: std::collections::BTreeMap<String, CounterpartyLine> = Default::default();
    for row in rows {
        let (name, category, currency, effect, role, note) = row?;
        let entry = grouped.entry(name.clone()).or_insert_with(|| CounterpartyLine {
            name,
            category,
            note,
            operations: 0,
            spent: MoneyBag::new(),
            hint: None,
        });
        if let (Some(currency), Some(effect)) = (currency, effect) {
            let currency = Currency::new(&currency)?;
            entry.operations += 1;
            if role.as_deref() == Some("expense") {
                entry.spent.add(Money::new(-effect, currency));
            }
        }
    }

    let mut hints = conn.prepare(
        "select p.name, c.name, count(*)
           from operation o
           join counterparty p on p.id = o.counterparty_id
           join category     c on c.id = o.category_id
          where o.state = 'POSTED' and p.category_id is null
          group by p.id, c.id",
    )?;
    for row in hints.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as usize))
    })? {
        let (name, category, count) = row?;
        if let Some(line) = grouped.get_mut(&name) {
            let better = match &line.hint {
                None => true,
                Some((current, n)) => count > *n || (count == *n && category < *current),
            };
            if better {
                line.hint = Some((category, count));
            }
        }
    }

    let mut out: Vec<CounterpartyLine> = grouped.into_values().collect();
    out.sort_by_key(|line| {
        let spent = crate::rate::rank_bag_minor(conn, &line.spent, "9999-12-31");
        (-spent, line.name.clone())
    });
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationLine {
    pub id: i64,
    pub occurred_on: String,
    pub account: String,
    pub description: String,
    pub counterparty: Option<String>,
    pub category: Option<String>,
    pub amount: Money,
    /// Removed as mistaken (invariant 8): visible, but counted nowhere.
    pub removed: bool,
    /// income | expense | transfer | debt_movement | …
    pub role: String,
    /// The second leg's account, if this is a transfer between one's own.
    pub pair_account: Option<String>,
}

/// What was spent each day among these operations: their expenses, the way
/// flows count them. The journal heads each day with it. Pure, like the fold
/// below — it adds what it is given.
pub fn spent_by_day(operations: &[OperationLine]) -> std::collections::HashMap<String, MoneyBag> {
    let mut out: std::collections::HashMap<String, MoneyBag> = Default::default();
    // A refund (an expense with a plus) takes its day's spending down.
    for op in operations.iter().filter(|o| !o.removed && o.role == "expense") {
        out.entry(op.occurred_on.clone()).or_default().add(op.amount.negate());
    }
    out
}

/// Operations folded by counterparty: who, how many, how much — the biggest
/// spend first. Pure: it folds what it is given, a journal or a search.
#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub count: usize,
    pub sum: MoneyBag,
}

pub fn group_by_counterparty(operations: &[OperationLine]) -> Vec<Group> {
    let mut groups: std::collections::BTreeMap<String, Group> = Default::default();
    for op in operations.iter().filter(|o| !o.removed) {
        let name = op.counterparty.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| op.description.clone());
        let group = groups.entry(name.clone()).or_insert_with(|| Group { name, count: 0, sum: MoneyBag::new() });
        group.count += 1;
        group.sum.add(op.amount);
    }
    let mut out: Vec<Group> = groups.into_values().collect();
    out.sort_by_key(|g| (g.sum.in_display_order(None).first().map(|m| m.minor()).unwrap_or(0), g.name.to_lowercase()));
    out
}

/// How the journal is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Newest first — the default.
    Date,
    /// The largest amount, in or out, first.
    Largest,
    /// The smallest first.
    Smallest,
}

pub fn sort(operations: &mut [OperationLine], order: Order) {
    match order {
        Order::Date => operations.sort_by(|a, b| b.occurred_on.cmp(&a.occurred_on).then(b.id.cmp(&a.id))),
        Order::Largest => operations.sort_by_key(|o| std::cmp::Reverse(o.amount.minor().abs())),
        Order::Smallest => operations.sort_by_key(|o| o.amount.minor().abs()),
    }
}

/// The shared part of selecting operations: account, counterparty and category
/// already resolved. The category comes off the operation, or the counterparty (§7a).
macro_rules! operation_base {
    () => {
        "
    select o.id, date(o.occurred_at), a.name, o.description,
           p.name, coalesce(own.name, inherited.name),
           o.currency, o.effect_minor, o.state, o.role, pa.name
      from operation o
      join account a on a.id = o.account_id
      left join operation    po        on po.id        = o.pair_id
      left join account      pa        on pa.id        = po.account_id
      left join counterparty p         on p.id         = o.counterparty_id
      left join category     own       on own.id       = o.category_id
      left join category     inherited on inherited.id = p.category_id"
    };
}

/// The posted ones — what counts everywhere.
const OPERATION_SELECT: &str = concat!(operation_base!(), "\n     where o.state = 'POSTED'");

/// The posted and the removed — for the journal, where a removed one has to be
/// visible to be brought back, and for one operation's details.
const OPERATION_SELECT_WITH_REMOVED: &str =
    concat!(operation_base!(), "\n     where o.state in ('POSTED', 'REVERSED')");

fn operation_line(row: &rusqlite::Row<'_>) -> rusqlite::Result<(OperationLine, String)> {
    Ok((
        OperationLine {
            id: row.get(0)?,
            occurred_on: row.get(1)?,
            account: row.get(2)?,
            description: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            counterparty: row.get(4)?,
            category: row.get(5)?,
            // The currency is checked below, outside the closure: an error of
            // another type cannot be returned here, and quietly putting in EUR
            // would be a lie.
            amount: Money::new(row.get(7)?, Currency::new("XXX").expect("XXX is an ISO 4217 code")),
            removed: row.get::<_, String>(8)? == "REVERSED",
            role: row.get(9)?,
            pair_account: row.get(10)?,
        },
        row.get::<_, String>(6)?,
    ))
}

fn with_currency((mut line, currency): (OperationLine, String)) -> Result<OperationLine> {
    line.amount = Money::new(line.amount.minor(), Currency::new(&currency)?);
    Ok(line)
}

/// The journal of operations, the new on top, whole. The category arrives
/// resolved — a screen must not know it comes off the counterparty (§5).
///
/// Whole rather than in pages. It used to be 500 at a time with a promise of
/// «loading as the cursor moves», and there was no loading: on the real ledger
/// of 1774 operations the 1274 older ones could not be scrolled to. Pages here
/// were an optimisation for a network a local database does not have.
/// ponytail: the whole journal in memory; pages if a ledger reaches ~10^5 rows.
pub fn operations(
    conn: &Connection,
    account_id: Option<i64>,
    with_removed: bool,
) -> Result<Vec<OperationLine>> {
    let select = if with_removed { OPERATION_SELECT_WITH_REMOVED } else { OPERATION_SELECT };
    let sql = format!(
        "{select}
            and (?1 is null or o.account_id = ?1)
          order by o.occurred_at desc, o.id desc"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![account_id], operation_line)?;
    rows.map(|row| with_currency(row?)).collect()
}

/// One operation by id — for the details pane and a transfer's second leg.
pub fn operation_by_id(conn: &Connection, id: i64) -> Result<Option<OperationLine>> {
    let sql = format!("{OPERATION_SELECT_WITH_REMOVED} and o.id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([id], operation_line)?;
    rows.next().map(|row| with_currency(row?)).transpose()
}

/// What an agent looks for. All optional; an empty filter means the latest.
#[derive(Debug, Clone, Default)]
pub struct OperationFilter {
    /// A substring of the description or the counterparty's name, case aside.
    pub text: Option<String>,
    pub account: Option<String>,
    pub category: Option<String>,
    /// Operations with no category only — where marking starts.
    pub uncategorized: bool,
    /// `YYYY-MM-DD`, inclusive.
    pub from: Option<String>,
    pub to: Option<String>,
    /// A counterparty's exact name — Enter on it shows its operations.
    pub counterparty: Option<String>,
    /// One loaded statement — Enter on it shows what it brought.
    pub import_file: Option<i64>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct OperationSearch {
    pub operations: Vec<OperationLine>,
    /// More was found than given back. Without this flag an agent reads the
    /// first fifty rows as «that is all» and concludes from half the data.
    pub truncated: bool,
}

/// Searching operations, for an agent.
///
/// The text is compared in Rust, not in SQL: `like` and `lower()` in SQLite
/// without ICU fold case for ASCII only, and «еконт» would not find «Еконт».
/// ponytail: a walk over every operation picked by date, account and category.
/// On a personal ledger that is thousands of rows and milliseconds; if one day
/// it stops being enough — FTS5 with its own table, not before.
pub fn search_operations(conn: &Connection, filter: &OperationFilter) -> Result<OperationSearch> {
    let sql = format!(
        "{OPERATION_SELECT}
            and (?1 is null or a.name = ?1)
            and (?2 is null or coalesce(own.name, inherited.name) = ?2)
            and (?3 = 0 or coalesce(own.name, inherited.name) is null)
            and (?4 is null or date(o.occurred_at) >= date(?4))
            and (?5 is null or date(o.occurred_at) <= date(?5))
            and (?6 is null or p.name = ?6)
            and (?7 is null or o.import_row_id in (select id from import_row where file_id = ?7))
          order by o.occurred_at desc, o.id desc"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![
            filter.account,
            filter.category,
            filter.uncategorized as i64,
            filter.from,
            filter.to,
            filter.counterparty,
            filter.import_file,
        ],
        operation_line,
    )?;

    let needle = filter.text.as_deref().map(str::to_lowercase).filter(|t| !t.is_empty());
    // «31.15» searches the amount too: finding a purchase by the price on the
    // receipt is quicker than recalling how the bank wrote the seller down.
    let amount = needle
        .as_deref()
        .filter(|t| crate::import::looks_like_amount(t))
        .and_then(|t| Money::parse(t, Currency::new("XXX").expect("XXX is an ISO 4217 code")).ok())
        .map(|m| m.minor().abs());
    let mut operations = Vec::new();
    let mut truncated = false;
    for row in rows {
        let line = with_currency(row?)?;
        if let Some(needle) = &needle {
            let haystack = format!(
                "{} {}",
                line.description.to_lowercase(),
                line.counterparty.as_deref().unwrap_or("").to_lowercase()
            );
            let by_amount = amount == Some(line.amount.minor().abs());
            if !by_amount && !haystack.contains(needle.as_str()) {
                continue;
            }
        }
        if operations.len() == filter.limit {
            truncated = true;
            break;
        }
        operations.push(line);
    }
    Ok(OperationSearch { operations, truncated })
}

#[derive(Debug, Clone)]
pub struct ImportedFile {
    pub id: i64,
    pub bank: String,
    pub filename: String,
    pub imported_at: String,
    pub rows: usize,
    pub operations: usize,
    pub archived_at: String,
}

/// What was loaded and when. The import screen is first of all the answer to
/// «have I loaded this already?», and it must be visible without SQL.
pub fn import_journal(conn: &Connection) -> Result<Vec<ImportedFile>> {
    let mut stmt = conn.prepare(
        "select f.bank, f.filename, f.imported_at, f.archived_at, f.id,
                (select count(*) from import_row r where r.file_id = f.id),
                (select count(*) from operation o
                   join import_row r on r.id = o.import_row_id
                  where r.file_id = f.id)
           from import_file f
          order by f.imported_at desc, f.id desc",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ImportedFile {
            bank: row.get(0)?,
            filename: row.get(1)?,
            imported_at: row.get(2)?,
            archived_at: row.get(3)?,
            id: row.get(4)?,
            rows: row.get::<_, i64>(5)? as usize,
            operations: row.get::<_, i64>(6)? as usize,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The categories' names — for choosing while marking.
pub fn categories(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("select name from category order by name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone)]
pub struct AccountChoice {
    pub id: i64,
    pub name: String,
}

pub fn account_choices(conn: &Connection) -> Result<Vec<AccountChoice>> {
    let mut stmt =
        conn.prepare("select id, name from account where state = 'ACTIVE' order by name")?;
    let rows = stmt.query_map([], |r| {
        Ok(AccountChoice { id: r.get(0)?, name: r.get(1)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn ledger() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('Revolut','EUR','BANK');
             insert into account (name,currency,kind) values ('Касса','BGN','CASH');
             insert into category (name) values ('Доставка');
             insert into counterparty (name, category_id) values ('Еконт', 1);
             insert into counterparty (name) values ('Kaufland');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
                 values (1,'expense','2026-09-01T00:00:00Z',350,'EUR',-350,'Еконт',1),
                        (1,'expense','2026-09-02T00:00:00Z',624,'EUR',-624,'Kaufland',2),
                        (1,'expense','2026-09-03T00:00:00Z',807,'EUR',-807,'Kaufland',2),
                        (2,'expense','2026-09-04T00:00:00Z',1200,'BGN',-1200,'Пазар',null);
             insert into operation (account_id,role,state,occurred_at,amount_minor,currency,effect_minor,description)
                 values (1,'expense','PENDING','2026-09-05T00:00:00Z',999,'EUR',-999,'Kaufland');",
        )
        .unwrap();
        conn
    }

    fn search(conn: &Connection, filter: OperationFilter) -> OperationSearch {
        search_operations(conn, &OperationFilter { limit: 50, ..filter }).unwrap()
    }

    /// Cyrillic case: `lower()` in SQLite without ICU does not fold it, hence
    /// the comparison in Rust. Without it «еконт» would not find «Еконт».
    #[test]
    fn searching_ignores_case_in_any_alphabet() {
        let conn = ledger();
        let found = search(&conn, OperationFilter { text: Some("еконт".into()), ..Default::default() });
        assert_eq!(found.operations.len(), 1);
        assert_eq!(found.operations[0].description, "Еконт");
        let found = search(&conn, OperationFilter { text: Some("KAUF".into()), ..Default::default() });
        assert_eq!(found.operations.len(), 2, "a pending one does not count");
    }

    /// The truncation flag — without which an agent reads page one as «all».
    #[test]
    fn a_search_says_when_it_stopped_short() {
        let conn = ledger();
        let found = search_operations(&conn, &OperationFilter { limit: 2, ..Default::default() }).unwrap();
        assert_eq!(found.operations.len(), 2);
        assert!(found.truncated, "four found, two given back — it has to be said");
        let all = search(&conn, OperationFilter::default());
        assert_eq!(all.operations.len(), 4);
        assert!(!all.truncated);
    }

    /// The category comes off the counterparty too — the filter has to know
    /// that, or what a rule marked looks unmarked.
    #[test]
    fn the_category_filter_sees_categories_inherited_from_the_counterparty() {
        let conn = ledger();
        let found = search(&conn, OperationFilter { category: Some("Доставка".into()), ..Default::default() });
        assert_eq!(found.operations.len(), 1);
        let unmarked = search(&conn, OperationFilter { uncategorized: true, ..Default::default() });
        assert_eq!(unmarked.operations.len(), 3, "Kaufland twice and the market");
        assert!(unmarked.operations.iter().all(|o| o.category.is_none()));
    }

    #[test]
    fn a_search_narrows_by_account_and_dates() {
        let conn = ledger();
        let cash = search(&conn, OperationFilter { account: Some("Касса".into()), ..Default::default() });
        assert_eq!(cash.operations.len(), 1);
        assert_eq!(cash.operations[0].amount.currency().as_str(), "BGN");
        let window = search(
            &conn,
            OperationFilter {
                from: Some("2026-09-02".into()),
                to: Some("2026-09-03".into()),
                ..Default::default()
            },
        );
        assert_eq!(window.operations.len(), 2, "the bounds are inclusive");
    }

    /// Drilling in shows exactly the operations that made up the row — for
    /// every row, not for one convenient one.
    #[test]
    fn drilling_into_a_flow_finds_exactly_what_it_counted() {
        let conn = ledger();
        let flows = flows(&conn, "2026-09-01", "2026-09-30").unwrap();
        assert!(!flows.is_empty());
        for flow in &flows {
            let found = flow_operations(&conn, &flow.label, "2026-09-01", "2026-09-30").unwrap();
            assert_eq!(found.len(), flow.count, "the row {:?}", flow.label);
        }
        let delivery = flow_operations(&conn, "Доставка", "2026-09-01", "2026-09-30").unwrap();
        assert_eq!(delivery[0].description, "Еконт", "a category inherited from the counterparty");
    }

    #[test]
    fn a_search_finds_by_the_amount_on_the_receipt() {
        let conn = ledger();
        let found = search(&conn, OperationFilter { text: Some("6.24".into()), ..Default::default() });
        assert_eq!(found.operations.len(), 1);
        assert_eq!(found.operations[0].description, "Kaufland");
        let found = search(&conn, OperationFilter { text: Some("12.00".into()), ..Default::default() });
        assert_eq!(found.operations.len(), 1, "in another currency too");
    }

    fn two_months() -> Connection {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK'), ('B','BGN','CASH');
             insert into category (name) values ('Food'), ('Cinema'), ('Taxi'), ('Market');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,category_id) values
               (1,'expense','2026-08-03T00:00:00Z',10000,'EUR',-10000,'a',1),
               (1,'expense','2026-09-03T00:00:00Z',12000,'EUR',-12000,'a',1),
               (1,'expense','2026-09-04T00:00:00Z', 1500,'EUR',-1500,'b',2),
               (1,'expense','2026-08-05T00:00:00Z',  800,'EUR',-800,'c',3),
               (1,'expense','2026-08-06T00:00:00Z',  500,'EUR',-500,'d',4),
               (2,'expense','2026-09-06T00:00:00Z',  900,'BGN',-900,'d',4);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_flow_is_compared_with_the_period_before_it() {
        let conn = two_months();
        let rows = compare_flows(&conn, ("2026-09-01", "2026-09-30"), ("2026-08-01", "2026-08-31")).unwrap();
        let by = |label: &str| rows.iter().find(|r| r.label == label).unwrap().change;
        assert_eq!(by("Food"), Change::Percent(20), "120 against 100");
        assert_eq!(by("Cinema"), Change::New);
        assert_eq!(by("Taxi"), Change::Gone, "what is gone shows too");
        assert_eq!(by("Market"), Change::NotComparable, "leva against euro is not per cent");
        assert_eq!(rows.last().unwrap().label, "Taxi", "the gone ones at the end");
    }

    /// The hint: the commonest category among an unmarked counterparty's
    /// operations — the real Kaufland case, 52 of 60.
    #[test]
    fn an_unmarked_counterparty_is_told_what_its_operations_mostly_are() {
        let conn = db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK');
             insert into category (name) values ('Food'), ('Home');
             insert into counterparty (name) values ('Kaufland');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id,category_id) values
               (1,'expense','2026-09-01T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-09-02T00:00:00Z',100,'EUR',-100,1,1),
               (1,'expense','2026-09-03T00:00:00Z',100,'EUR',-100,1,2),
               (1,'expense','2026-09-04T00:00:00Z',100,'EUR',-100,1,null);",
        )
        .unwrap();
        let lines = counterparties(&conn).unwrap();
        assert_eq!(lines[0].hint, Some(("Food".to_string(), 2)));
        assert_eq!(lines[0].operations, 4);
    }

    /// A pending operation appears nowhere, search included (invariant 6).
    #[test]
    fn a_pending_operation_is_never_found() {
        let conn = ledger();
        let found = search(&conn, OperationFilter::default());
        assert!(found.operations.iter().all(|o| o.amount.minor() != -999));
    }

    /// Rows of several currencies are ordered and shared out in the base
    /// currency. Adding minor units across currencies put 15 000 RUB first
    /// and at 304% of a month spent in euros.
    #[test]
    fn flows_in_several_currencies_are_compared_in_the_base_one() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('E','EUR','BANK'), ('R','RUB','CASH');
             insert into category (name) values ('Rent'), ('Shop');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,category_id) values
               (1,'expense','2026-10-03T00:00:00Z',160000,'EUR',-160000,1),
               (2,'expense','2026-10-06T00:00:00Z',1500000,'RUB',-1500000,2);",
        )
        .unwrap();
        let rows = flows(&conn, "2026-10-01", "2026-10-20").unwrap();
        assert_eq!(rows.iter().map(|f| f.label.as_str()).collect::<Vec<_>>(), ["Rent", "Shop"], "no rate: the rouble row last");
        assert!(rows.iter().all(|f| f.share.is_none()), "no share of a partial total");

        let rub = crate::Currency::new("RUB").unwrap();
        let eur = crate::Currency::new("EUR").unwrap();
        crate::rate::set(&conn, rub, eur, crate::rate::parse("0.01").unwrap(), "2026-10-01").unwrap();
        let rows = flows(&conn, "2026-10-01", "2026-10-20").unwrap();
        assert_eq!(rows[0].label, "Rent", "1 600 EUR before 15 000 RUB = 150 EUR");
        assert_eq!(rows.iter().map(|f| f.share).collect::<Vec<_>>(), [Some(91), Some(8)]);
        assert_eq!(rows[1].out_base.map(|m| m.minor()), Some(15000));
    }

    /// Only posted income and expense are flows: a removed duplicate, a
    /// pending row and a transfer between own accounts are not. No test held
    /// this until the 2026-10-20 audit counted removed operations in by hand
    /// and nothing failed.
    #[test]
    fn removed_pending_and_transfers_stay_out_of_the_flows() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (name,currency,kind) values ('A','EUR','BANK'), ('B','EUR','BANK');
             insert into operation (id,account_id,role,state,occurred_at,amount_minor,currency,effect_minor,description) values
               (1,1,'expense','POSTED',  '2026-10-02T00:00:00Z',1000,'EUR',-1000,'Shop'),
               (2,1,'expense','REVERSED','2026-10-02T00:00:00Z',1000,'EUR',-1000,'Shop'),
               (3,1,'expense','PENDING', '2026-10-03T00:00:00Z', 700,'EUR', -700,'Shop'),
               (4,1,'transfer','POSTED', '2026-10-04T00:00:00Z',5000,'EUR',-5000,'To B'),
               (5,2,'transfer','POSTED', '2026-10-04T00:00:00Z',5000,'EUR', 5000,'From A');
             update operation set pair_id = 5 where id = 4;
             update operation set pair_id = 4 where id = 5;",
        )
        .unwrap();
        let rows = flows(&conn, "2026-10-01", "2026-10-31").unwrap();
        let (out, inn) = flow_totals(&rows);
        assert_eq!(out.in_display_order(None).iter().map(|m| m.minor()).collect::<Vec<_>>(), [1000], "{rows:?}");
        assert!(inn.is_empty(), "{rows:?}");
        assert_eq!(rows.iter().map(|f| f.count).sum::<usize>(), 1);
    }

    #[test]
    fn off_budget_and_excluded_accounts_stay_out_of_flows_and_refunds_adjust_counterparty_spend() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind,inclusion) values
               (1,'Main','EUR','BANK','FULL'),
               (2,'Stash','EUR','SAVINGS','OFF_BUDGET'),
               (3,'Other','EUR','OTHER','EXCLUDED');
             insert into category (id,name) values (1,'Clothes'), (2,'Food');
             insert into counterparty (id,name,category_id) values (1,'Zara',1), (2,'Lidl',2);
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,counterparty_id) values
               (1,'expense','2026-10-02T00:00:00Z',10000,'EUR',-10000,1),
               (1,'expense','2026-10-05T00:00:00Z', 4000,'EUR',  4000,1),
               (2,'expense','2026-10-03T00:00:00Z',50000,'EUR',-50000,2),
               (3,'expense','2026-10-04T00:00:00Z',90000,'EUR',-90000,2);",
        )
        .unwrap();
        let rows = flows(&conn, "2026-10-01", "2026-10-31").unwrap();
        assert_eq!(rows.len(), 1, "OFF_BUDGET and EXCLUDED accounts are not counted in flows");
        assert_eq!(rows[0].out.in_display_order(None)[0].minor(), 6000, "100 spent - 40 refunded = 60");
        assert_eq!(flow_operations(&conn, "Food", "2026-10-01", "2026-10-31").unwrap().len(), 0);

        let cps = counterparties(&conn).unwrap();
        let zara = cps.iter().find(|c| c.name == "Zara").unwrap();
        assert_eq!(zara.spent.in_display_order(None)[0].minor(), 6000, "refund reduces counterparty spent");
    }
}
