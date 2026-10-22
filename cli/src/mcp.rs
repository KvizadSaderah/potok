//! `potok mcp` — the ledger for an agent, over stdio, read-only (§6).
//!
//! The protocol is written by hand over `serde_json`, with no official Rust SDK.
//! There is one reason and it was checked: the SDK pulls in tokio and
//! asynchrony, while this binary is synchronous throughout — an interface
//! without a single `async` was the main gain of local-first (§5). Dragging a
//! runtime in for seven tools would bring back what was just deleted.
//!
//! The subset of MCP needed is small: `initialize`, `ping`, `tools/list`,
//! `tools/call`. A message is one line of JSON-RPC 2.0 on stdin, the answer one
//! line on stdout. **Nothing but answers is written to stdout**: one debug line
//! breaks the client's parsing of the stream. Errors go to stderr.
//!
//! Compatibility was checked with the official client from
//! `@modelcontextprotocol/sdk` 1.30.0 — the one `saldo`'s MCP server is written on.
//!
//! Writing is not given to an agent at all, and what holds that is not the
//! absence of tools but the database itself: the connection is opened through
//! `db::open_read_only`, where SQLite refuses every write. Writing tools will
//! arrive together with an `--allow-writes` flag, not before (§6).
use std::io::{self, BufRead, Write};

use anyhow::Result;
use chrono::NaiveDate;
use potok_core::query::{self, OperationFilter};
use potok_core::{verify, Money, MoneyBag};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

use potok_core::period::Period;

/// The protocol versions a subset of which is implemented here. The same list
/// as in SDK 1.30.0, without `2024-10-07` — a draft nobody sends.
///
/// ponytail: the tools and their answers have one shape in all four versions,
/// so supporting them all means answering with the same version. If the next
/// version changes the shape of `tools/call`, it will not be here, and the
/// client will get our latest — which is what the specification requires.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "\
Read-only view of a personal ledger on this machine. \
Amounts are exact: `amount` is a decimal string, `minor` the same value in \
cents. Never add amounts of different currencies; totals are already split \
per currency. Pending operations are not money and never appear. \
Negative amounts left the account, positive ones arrived. \
Nothing here can change the ledger.";

pub fn run(conn: Connection) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // The date is taken per message: the server lives as long as the client,
        // and «this month» after midnight on the first is already another one.
        let today = chrono::Local::now().date_naive();
        if let Some(response) = handle(&conn, &line, today) {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// One message → an answer, or nothing if it is a notification.
pub fn handle(conn: &Connection, line: &str, today: NaiveDate) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => return Some(failure(Value::Null, -32700, &format!("parse error: {error}"))),
    };
    let Some(object) = message.as_object() else {
        // Batches of messages were removed from the protocol in 2025-06-18;
        // accepting them would mean supporting what clients no longer send.
        return Some(failure(Value::Null, -32600, "expected a single JSON-RPC object"));
    };

    let id = object.get("id").cloned();
    let method = object.get("method").and_then(Value::as_str).unwrap_or("");
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));

    // With no `id` it is a notification: `notifications/initialized`,
    // `notifications/cancelled`. They must not be answered.
    let id = match id {
        Some(Value::Null) | None => return None,
        Some(id) => id,
    };

    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call(conn, &params, today),
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => failure(id, code, &message),
    })
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version =
        if PROTOCOL_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSIONS[0] };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "potok", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

// ── Tools ───────────────────────────────────────────────────────────────────

/// Hints to the client: changes nothing, safe to repeat, does not go outside.
fn read_only() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

fn tool(name: &str, title: &str, description: &str, properties: Value) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "additionalProperties": false },
        "annotations": read_only(),
    })
}

fn date_property(what: &str) -> Value {
    json!({ "type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$", "description": what })
}

/// The names come from `saldo`'s MCP server: an agent that knew the old one
/// will not have to relearn. All but one: `list_pending_imports` became
/// `list_imports`, because there are no unconfirmed imports here — an import is
/// written whole at once or not written at all.
pub fn tools() -> Vec<Value> {
    vec![
        tool(
            "get_financial_overview",
            "Financial overview",
            "Balances of every account, what came in and went out over a period \
             grouped by category or counterparty, and open debts. Start here. \
             Defaults to the current accounting month.",
            json!({
                "from": date_property("First day, inclusive. Defaults to the start of this accounting month — the ledger's month rule, as the screens use."),
                "to": date_property("Last day, inclusive. Defaults to today."),
            }),
        ),
        tool(
            "list_accounts",
            "List accounts",
            "Every active account with its currency and current balance. A balance is \
             computed from posted operations, never stored.",
            json!({}),
        ),
        tool(
            "search_operations",
            "Search operations",
            "Find posted operations, newest first. `text` matches the description or the \
             counterparty, case-insensitively in any alphabet. If `truncated` is true there \
             are more matches than were returned — narrow the search rather than concluding \
             these are all.",
            json!({
                "text": { "type": "string", "description": "Substring of description or counterparty." },
                "account": { "type": "string", "description": "Exact account name." },
                "category": { "type": "string", "description": "Exact category name." },
                "uncategorized": { "type": "boolean", "description": "Only operations with no category." },
                "from": date_property("First day, inclusive."),
                "to": date_property("Last day, inclusive."),
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 },
            }),
        ),
        tool(
            "list_debts",
            "List debts",
            "Open debts in both directions. The remaining amount is computed from the \
             debt's movements, never stored.",
            json!({}),
        ),
        tool(
            "list_counterparties",
            "List counterparties",
            "Who the money went to or came from, with how many operations and how much was \
             spent, largest first. A counterparty's category applies to all its operations — \
             this is the list to look at when deciding what to categorise.",
            json!({
                "uncategorized_only": { "type": "boolean", "default": false },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100 },
            }),
        ),
        tool(
            "list_imports",
            "List imports",
            "Which bank statements were loaded and when, with how many rows each produced. \
             Answers \"has this statement been imported already\".",
            json!({}),
        ),
        tool(
            "get_monthly_totals",
            "Monthly totals",
            "In and out for each of the last months, oldest first, by the same rule as the \
             overview. Months start on the ledger's accounting day (a salary day, 1 by default).",
            json!({ "months": { "type": "integer", "minimum": 1, "maximum": 36, "default": 12 } }),
        ),
        tool(
            "list_regular_payments",
            "Regular payments",
            "Payments that repeat monthly or weekly — rent, subscriptions, bills — found in the \
             history, with the usual amount and when the next one is expected. Found, not \
             entered: nothing in the ledger marks them.",
            json!({}),
        ),
        tool(
            "get_budgets",
            "Budgets",
            "Every monthly budget against what was spent in its category this accounting month.",
            json!({}),
        ),
        tool(
            "get_reconciliation_status",
            "Reconciliation with the bank",
            "For each account, the last balance the person read off the bank, the ledger's \
             balance on that day, and the difference. An account never reconciled says so.",
            json!({}),
        ),
        tool(
            "list_duplicates",
            "Look-alike operations",
            "Groups of operations on one account, one day, one amount and one counterparty — \
             duplicate candidates the person has not dismissed. Candidates, not verdicts.",
            json!({}),
        ),
        tool(
            "list_changes",
            "Changes made by hand",
            "The journal of edits — removed, restored, linked, fixed, imported — newest first.",
            json!({ "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 } }),
        ),
        tool(
            "list_categories",
            "Categories",
            "Categories with how many counterparties and operations carry each, and what was \
             spent in each this accounting month.",
            json!({}),
        ),
        tool(
            "verify",
            "Verify the ledger",
            "Check the ledger's invariants: transfer legs that cancel, debts that do not go \
             below zero, reversals that point at what they reverse. Reports findings, changes \
             nothing.",
            json!({}),
        ),
    ]
}

type CallResult = std::result::Result<Value, (i64, String)>;

fn call(conn: &Connection, params: &Value, today: NaiveDate) -> CallResult {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let empty = Map::new();
    let args = params.get("arguments").and_then(Value::as_object).unwrap_or(&empty);

    // An unknown tool is a protocol error; an error inside a known one is a
    // result with `isError`, so that an agent can read it and correct itself.
    let outcome = match name {
        "get_financial_overview" => overview(conn, args, today),
        "list_accounts" => accounts(conn),
        "search_operations" => search(conn, args),
        "list_debts" => debts(conn),
        "list_counterparties" => counterparties(conn, args),
        "list_imports" => imports(conn),
        "verify" => check(conn),
        "get_monthly_totals" => monthly(conn, args, today),
        "list_regular_payments" => regular(conn, today),
        "get_budgets" => budgets(conn, today),
        "get_reconciliation_status" => reconciliation(conn),
        "list_duplicates" => duplicates(conn),
        "list_changes" => changes(conn, args),
        "list_categories" => categories(conn, today),
        _ => return Err((-32602, format!("unknown tool: {name}"))),
    };

    Ok(match outcome {
        Ok(value) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
        }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": format!("{error:#}") }],
            "isError": true,
        }),
    })
}

// ── Parsing the arguments ───────────────────────────────────────────────────

fn text_arg(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => anyhow::bail!("`{key}` must be a string"),
    }
}

fn bool_arg(args: &Map<String, Value>, key: &str) -> Result<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => anyhow::bail!("`{key}` must be true or false"),
    }
}

fn limit_arg(args: &Map<String, Value>, default: usize) -> Result<usize> {
    match args.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(value) => match value.as_u64() {
            Some(n @ 1..=500) => Ok(n as usize),
            _ => anyhow::bail!("`limit` must be an integer from 1 to 500"),
        },
    }
}

/// A date is checked before the query, because SQLite does not complain about a
/// crooked one: `date('yesterday')` is NULL, a comparison with NULL is false,
/// and the search would quietly return no rows. An agent would read «none».
fn date_arg(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    let Some(text) = text_arg(args, key)? else { return Ok(None) };
    NaiveDate::parse_from_str(&text, "%Y-%m-%d")
        .map_err(|_| anyhow::anyhow!("`{key}` must be a date as YYYY-MM-DD, got {text:?}"))?;
    Ok(Some(text))
}

// ── Money going out ─────────────────────────────────────────────────────────

/// A sum for an agent: an exact string, whole cents and a currency. No `f64` —
/// a floating-point number reads as `4873.4`, and adding such numbers up is
/// exactly the mistake `Money` is built the way it is to prevent.
fn money(amount: &Money) -> Value {
    json!({
        "amount": amount.decimal(),
        "minor": amount.minor(),
        "currency": amount.currency().as_str(),
    })
}

fn bag(totals: &MoneyBag) -> Value {
    Value::Array(totals.in_display_order(None).iter().map(money).collect())
}

// ── The tools themselves ────────────────────────────────────────────────────

fn overview(conn: &Connection, args: &Map<String, Value>, today: NaiveDate) -> Result<Value> {
    // The month the screens show: from the ledger's month rule, not the 1st.
    // On the 1st a salary paid on the 30th fell out, and the agent read a
    // month of spending with almost nothing earned.
    let (default_from, default_to) = Period::ThisMonth.bounds_on(today, &potok_core::period::MonthRule::load(conn)?);
    let from = date_arg(args, "from")?.unwrap_or(default_from);
    let to = date_arg(args, "to")?.unwrap_or(default_to);
    let flows = query::flows(conn, &from, &to)?;
    let (out, inn) = query::flow_totals(&flows);
    Ok(json!({
        "period": { "from": from, "to": to },
        "accounts": accounts(conn)?,
        "money_out": bag(&out),
        "money_in": bag(&inn),
        "flows": flows.iter().map(|flow| json!({
            "label": flow.label,
            "out": bag(&flow.out),
            "in": bag(&flow.inn),
            "operations": flow.count,
        })).collect::<Vec<_>>(),
        "open_debts": debts(conn)?,
    }))
}

fn accounts(conn: &Connection) -> Result<Value> {
    Ok(Value::Array(
        query::balances(conn)?
            .iter()
            .map(|account| json!({ "name": account.name, "balance": money(&account.balance) }))
            .collect(),
    ))
}

fn search(conn: &Connection, args: &Map<String, Value>) -> Result<Value> {
    let filter = OperationFilter {
        text: text_arg(args, "text")?,
        account: text_arg(args, "account")?,
        category: text_arg(args, "category")?,
        uncategorized: bool_arg(args, "uncategorized")?,
        from: date_arg(args, "from")?,
        to: date_arg(args, "to")?,
        counterparty: None,
        import_file: None,
        limit: limit_arg(args, 50)?,
    };
    let found = query::search_operations(conn, &filter)?;
    Ok(json!({
        "operations": found.operations.iter().map(|op| json!({
            "date": op.occurred_on,
            "account": op.account,
            "description": op.description,
            "counterparty": op.counterparty,
            "category": op.category,
            "amount": money(&op.amount),
        })).collect::<Vec<_>>(),
        "returned": found.operations.len(),
        "truncated": found.truncated,
    }))
}

fn debts(conn: &Connection) -> Result<Value> {
    Ok(Value::Array(
        query::debts(conn, false)?
            .iter()
            .map(|debt| json!({
                "counterparty": debt.counterparty,
                "direction": if debt.direction == "OWED_TO_ME" { "owed_to_me" } else { "i_owe" },
                "remaining": money(&debt.remaining),
                "issued": money(&debt.issued),
            }))
            .collect(),
    ))
}

fn counterparties(conn: &Connection, args: &Map<String, Value>) -> Result<Value> {
    let only_unmarked = bool_arg(args, "uncategorized_only")?;
    let limit = limit_arg(args, 100)?;
    let all = query::counterparties(conn)?;
    let matching: Vec<_> =
        all.iter().filter(|line| !only_unmarked || line.category.is_none()).collect();
    Ok(json!({
        "counterparties": matching.iter().take(limit).map(|line| json!({
            "name": line.name,
            "category": line.category,
            "note": line.note,
            "operations": line.operations,
            "spent": bag(&line.spent),
        })).collect::<Vec<_>>(),
        "returned": matching.len().min(limit),
        "truncated": matching.len() > limit,
    }))
}

fn imports(conn: &Connection) -> Result<Value> {
    Ok(Value::Array(
        query::import_journal(conn)?
            .iter()
            .map(|file| json!({
                "bank": file.bank,
                "filename": file.filename,
                "imported_at": file.imported_at,
                "rows": file.rows,
                "operations": file.operations,
            }))
            .collect(),
    ))
}

fn this_month(conn: &Connection, today: NaiveDate) -> Result<(String, String)> {
    let start = potok_core::period::MonthRule::load(conn)?.cycle(today, 0).start;
    Ok((start.to_string(), today.to_string()))
}

fn monthly(conn: &Connection, args: &Map<String, Value>, today: NaiveDate) -> Result<Value> {
    let months = match args.get("months") {
        None | Some(Value::Null) => 12,
        Some(v) => match v.as_u64() {
            Some(n @ 1..=36) => n as u32,
            _ => anyhow::bail!("`months` must be an integer from 1 to 36"),
        },
    };
    Ok(Value::Array(
        potok_core::analytics::months(conn, today, months)?
            .iter()
            .map(|m| json!({
                "month_start": m.start.to_string(),
                "month_end": m.end.to_string(),
                "start_corrected": m.corrected,
                "in": bag(&m.inn),
                "out": bag(&m.out),
                "in_base": m.inn_base.as_ref().map(money),
                "out_base": m.out_base.as_ref().map(money),
            }))
            .collect(),
    ))
}

fn regular(conn: &Connection, today: NaiveDate) -> Result<Value> {
    Ok(Value::Array(
        potok_core::analytics::regular(conn, today)?
            .iter()
            .map(|r| json!({
                "counterparty": r.who,
                "typical": money(&r.typical),
                "every_days": r.every_days,
                "last": r.last.to_string(),
                "next_expected": r.next.to_string(),
                "payments": r.count,
            }))
            .collect(),
    ))
}

fn budgets(conn: &Connection, today: NaiveDate) -> Result<Value> {
    let (from, to) = this_month(conn, today)?;
    Ok(json!({
        "from": from,
        "to": to,
        "budgets": potok_core::budget::status(conn, &from, &to)?.iter().map(|b| json!({
            "category": b.category,
            "limit": money(&b.limit),
            "spent": money(&b.spent),
            "left": money(&b.left),
        })).collect::<Vec<_>>(),
    }))
}

fn reconciliation(conn: &Connection) -> Result<Value> {
    let mut out = Vec::new();
    for account in query::balances(conn)? {
        out.push(match potok_core::reconcile::last(conn, account.id)? {
            Some(r) => json!({
                "account": account.name,
                "on": r.on_date,
                "bank": money(&r.bank),
                "ledger": money(&r.ledger),
                "difference": money(&r.difference),
                "matches": r.matches(),
            }),
            None => json!({ "account": account.name, "reconciled": false }),
        });
    }
    Ok(Value::Array(out))
}

fn duplicates(conn: &Connection) -> Result<Value> {
    Ok(Value::Array(
        potok_core::reconcile::duplicates(conn, None)?
            .iter()
            .map(|g| json!({
                "operations": g.operations.iter().map(|o| json!({
                    "id": o.id,
                    "date": o.occurred_on,
                    "account": o.account,
                    "amount": money(&o.amount),
                    "description": o.description,
                    "role": o.role,
                })).collect::<Vec<_>>(),
            }))
            .collect(),
    ))
}

fn changes(conn: &Connection, args: &Map<String, Value>) -> Result<Value> {
    let limit = limit_arg(args, 50)?;
    Ok(Value::Array(
        potok_core::journal::entries(conn, limit)?
            .iter()
            .map(|e| json!({ "id": e.id, "at": e.at, "what": e.what, "undone_at": e.undone_at, "can_undo": e.can_undo }))
            .collect(),
    ))
}

fn categories(conn: &Connection, today: NaiveDate) -> Result<Value> {
    let (from, to) = this_month(conn, today)?;
    Ok(Value::Array(
        potok_core::category::list(conn, &from, &to)?
            .iter()
            .map(|c| json!({
                "name": c.name,
                "counterparties": c.counterparties,
                "operations": c.operations,
                "spent_this_month": bag(&c.spent),
            }))
            .collect(),
    ))
}

fn check(conn: &Connection) -> Result<Value> {
    let report = verify::run(conn)?;
    Ok(json!({
        "ok": report.ok(),
        "checked": report.checked,
        "findings": report.findings.iter().map(|f| json!({
            "invariant": f.invariant,
            "what": f.what,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every new tool answers on a real, read-only connection — `query_only`
    /// would turn any write into an error here.
    #[test]
    fn the_new_tools_answer_read_only() {
        let path = std::env::temp_dir().join(format!("potok-mcp-new-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = potok_core::db::open(&path).unwrap();
            conn.execute_batch(SAMPLE).unwrap();
        }
        let conn = potok_core::db::open_read_only(&path).unwrap();
        for name in [
            "get_monthly_totals",
            "list_regular_payments",
            "get_budgets",
            "get_reconciliation_status",
            "list_duplicates",
            "list_changes",
            "list_categories",
        ] {
            let result = call(&conn, &json!({ "name": name, "arguments": {} }), today());
            let value = result.unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert!(value.get("isError").is_none_or(|e| e == false), "{name}: {value}");
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()
    }

    const SAMPLE: &str = "
        insert into account (name,currency,kind,opening_minor) values ('Revolut','EUR','BANK',40212);
        insert into account (name,currency,kind,opening_minor) values ('Till','BGN','CASH',0);
        insert into category (name) values ('Groceries');
        insert into counterparty (name, category_id) values ('Kaufland', 1);
        insert into counterparty (name) values ('Еконт');
        insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description,counterparty_id)
            values (1,'expense','2026-09-01T00:00:00Z',624,'EUR',-624,'Kaufland',1),
                   (1,'expense','2026-09-05T00:00:00Z',350,'EUR',-350,'Еконт',2),
                   (2,'expense','2026-09-06T00:00:00Z',1200,'BGN',-1200,'Market',null);
        insert into debt (counterparty_id,direction,currency,opened_on)
            values (2,'OWED_TO_ME','EUR','2026-09-03');
        insert into debt_movement (debt_id,kind,amount_minor,occurred_on)
            values (1,'ISSUE',80000,'2026-09-03');";

    fn ledger() -> Connection {
        let conn = potok_core::db::open_memory().unwrap();
        conn.execute_batch(SAMPLE).unwrap();
        conn
    }

    fn request(conn: &Connection, method: &str, params: Value) -> Value {
        let line = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
        handle(conn, &line.to_string(), today()).expect("a request needs an answer")
    }

    /// A tool's result is JSON inside a text block.
    fn call_tool(conn: &Connection, name: &str, arguments: Value) -> (Value, bool) {
        let response = request(conn, "tools/call", json!({ "name": name, "arguments": arguments }));
        let result = &response["result"];
        let text = result["content"][0]["text"].as_str().expect("a text block").to_string();
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (value, is_error)
    }

    #[test]
    fn initialize_answers_with_the_version_the_client_asked_for() {
        let conn = ledger();
        for version in PROTOCOL_VERSIONS {
            let response = request(&conn, "initialize", json!({ "protocolVersion": version }));
            assert_eq!(response["result"]["protocolVersion"], version);
            assert!(response["result"]["capabilities"]["tools"].is_object());
            assert_eq!(response["result"]["serverInfo"]["name"], "potok");
        }
    }

    /// An unknown version is answered with our latest, as the specification
    /// says, rather than echoing what we do not know.
    #[test]
    fn an_unknown_version_gets_ours_rather_than_an_echo() {
        let conn = ledger();
        let response = request(&conn, "initialize", json!({ "protocolVersion": "2099-01-01" }));
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    /// A notification must not be answered: the client waits for no answer and
    /// will take it for rubbish in the stream.
    #[test]
    fn a_notification_gets_no_answer() {
        let conn = ledger();
        let line = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(&conn, &line.to_string(), today()).is_none());
    }

    #[test]
    fn broken_json_is_a_parse_error_with_no_id() {
        let conn = ledger();
        let response = handle(&conn, "{not json", today()).unwrap();
        assert_eq!(response["error"]["code"], -32700);
        assert!(response["id"].is_null());
    }

    #[test]
    fn an_unknown_method_is_said_to_be_unknown() {
        let conn = ledger();
        let response = request(&conn, "resources/list", json!({}));
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(response["id"], 7, "an error answers the same id");
    }

    #[test]
    fn ping_is_answered() {
        let conn = ledger();
        assert_eq!(request(&conn, "ping", json!({}))["result"], json!({}));
    }

    /// Every tool is described in full and honestly marked read-only.
    #[test]
    fn every_tool_is_described_and_marked_read_only() {
        let conn = ledger();
        let response = request(&conn, "tools/list", json!({}));
        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 14);
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert!(!tool["description"].as_str().unwrap().is_empty(), "{name} has no description");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
            assert_eq!(tool["annotations"]["destructiveHint"], false, "{name}");
        }
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error() {
        let conn = ledger();
        let response =
            request(&conn, "tools/call", json!({ "name": "create_operation", "arguments": {} }));
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn balances_arrive_exact() {
        let conn = ledger();
        let (accounts, is_error) = call_tool(&conn, "list_accounts", json!({}));
        assert!(!is_error);
        let revolut = accounts.as_array().unwrap().iter().find(|a| a["name"] == "Revolut").unwrap();
        assert_eq!(revolut["balance"]["amount"], "392.38", "402.12 − 6.24 − 3.50");
        assert_eq!(revolut["balance"]["minor"], 39238);
        assert_eq!(revolut["balance"]["currency"], "EUR");
    }

    /// Not one fractional number in any answer: sums are strings and whole
    /// cents. An agent handed `4873.4` will add it to another `f64`, and that
    /// will be the first mistake in the figures.
    #[test]
    fn no_answer_carries_a_fractional_number() {
        fn walk(value: &Value, path: &str) {
            match value {
                Value::Number(n) => assert!(n.is_i64() || n.is_u64(), "a fractional number in {path}: {n}"),
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        walk(item, &format!("{path}[{i}]"));
                    }
                }
                Value::Object(map) => {
                    for (key, item) in map {
                        walk(item, &format!("{path}.{key}"));
                    }
                }
                _ => {}
            }
        }
        let conn = ledger();
        for tool in tools() {
            let name = tool["name"].as_str().unwrap();
            let arguments = if name == "get_financial_overview" {
                json!({ "from": "2026-09-01", "to": "2026-09-30" })
            } else {
                json!({})
            };
            let (value, is_error) = call_tool(&conn, name, arguments);
            assert!(!is_error, "{name}: {value}");
            walk(&value, name);
        }
    }

    /// Different currencies do not add up in the overview either: a total per currency.
    #[test]
    fn the_overview_keeps_currencies_apart() {
        let conn = ledger();
        let (overview, _) =
            call_tool(&conn, "get_financial_overview", json!({ "from": "2026-09-01", "to": "2026-09-30" }));
        let out = overview["money_out"].as_array().unwrap();
        assert_eq!(out.len(), 2, "EUR and BGN apart: {out:?}");
        let eur = out.iter().find(|m| m["currency"] == "EUR").unwrap();
        assert_eq!(eur["amount"], "9.74");
        assert_eq!(overview["open_debts"].as_array().unwrap().len(), 1);
    }

    /// The overview defaults to this month, and «this» comes from the date passed in.
    #[test]
    fn the_overview_defaults_to_this_month() {
        let conn = ledger();
        let (overview, _) = call_tool(&conn, "get_financial_overview", json!({}));
        assert_eq!(overview["period"]["from"], "2026-09-01");
        assert_eq!(overview["period"]["to"], "2026-09-20");
        // A month from the last working day: the same start the screens use.
        potok_core::settings::set(&conn, potok_core::settings::MONTH_MODE, "last_workday").unwrap();
        let (overview, _) = call_tool(&conn, "get_financial_overview", json!({}));
        assert_eq!(overview["period"]["from"], "2026-08-31");
    }

    /// A crooked date is an error rather than an empty answer: otherwise an
    /// agent reads it as «there are no operations».
    #[test]
    fn a_malformed_date_is_refused_rather_than_answered_with_nothing() {
        let conn = ledger();
        let (message, is_error) = call_tool(&conn, "search_operations", json!({ "from": "yesterday" }));
        assert!(is_error, "an error was due, and this came: {message}");
        assert!(message.as_str().unwrap().contains("YYYY-MM-DD"));
    }

    #[test]
    fn a_wrongly_typed_argument_names_itself() {
        let conn = ledger();
        let (message, is_error) = call_tool(&conn, "search_operations", json!({ "limit": "a lot" }));
        assert!(is_error);
        assert!(message.as_str().unwrap().contains("limit"));
        let (message, is_error) = call_tool(&conn, "search_operations", json!({ "limit": 501 }));
        assert!(is_error, "{message}");
    }

    #[test]
    fn a_search_finds_across_alphabets_and_says_when_it_is_cut() {
        let conn = ledger();
        let (found, _) = call_tool(&conn, "search_operations", json!({ "text": "ЕКОНТ" }));
        assert_eq!(found["returned"], 1);
        assert_eq!(found["operations"][0]["amount"]["amount"], "-3.50");
        let (found, _) = call_tool(&conn, "search_operations", json!({ "limit": 1 }));
        assert_eq!(found["returned"], 1);
        assert_eq!(found["truncated"], true);
    }

    #[test]
    fn counterparties_can_be_narrowed_to_the_unmarked() {
        let conn = ledger();
        let (found, _) = call_tool(&conn, "list_counterparties", json!({ "uncategorized_only": true }));
        let names: Vec<_> = found["counterparties"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["Еконт"], "Kaufland is marked");
    }

    /// Not one tool writes to the database quietly. Checked not by reading the
    /// code but by running each on a connection where SQLite refuses every write.
    #[test]
    fn every_tool_works_on_a_ledger_that_cannot_be_written() {
        let path = std::env::temp_dir().join(format!("potok-mcp-{}.db", std::process::id()));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        potok_core::db::open(&path).unwrap().execute_batch(SAMPLE).unwrap();
        let conn = potok_core::db::open_read_only(&path).unwrap();
        for tool in tools() {
            let name = tool["name"].as_str().unwrap();
            let (value, is_error) = call_tool(&conn, name, json!({}));
            assert!(!is_error, "{name} failed on a read-only database: {value}");
        }
        drop(conn);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
