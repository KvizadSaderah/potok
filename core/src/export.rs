//! CSV out: for a spreadsheet, an accountant, or another program.
//!
//! Amounts are decimal strings with a dot, never floats, and never rounded —
//! the minor units are exact. RFC 4180 quoting: a field with a comma, a quote
//! or a line break is quoted, quotes doubled.
use crate::query::OperationLine;

fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

pub fn csv(operations: &[OperationLine]) -> String {
    let mut out = String::from("id,date,account,amount,currency,role,counterparty,category,description,removed\n");
    for op in operations {
        let row = [
            op.id.to_string(),
            op.occurred_on.clone(),
            field(&op.account),
            op.amount.decimal(),
            op.amount.currency().to_string(),
            op.role.clone(),
            field(op.counterparty.as_deref().unwrap_or("")),
            field(op.category.as_deref().unwrap_or("")),
            field(&op.description),
            (op.removed as u8).to_string(),
        ];
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_quoted_and_amounts_exact() {
        let conn = crate::db::open_memory().unwrap();
        conn.execute_batch(
            "insert into account (id,name,currency,kind) values (1,'Cash','EUR','CASH');
             insert into operation (account_id,role,occurred_at,amount_minor,currency,effect_minor,description)
             values (1,'expense','2026-10-01',1005,'EUR',-1005,'Coffee, \"large\"');",
        )
        .unwrap();
        let ops = crate::query::operations(&conn, None, true).unwrap();
        let text = csv(&ops);
        assert!(text.contains(",-10.05,EUR,expense,"), "{text}");
        assert!(text.contains("\"Coffee, \"\"large\"\"\""), "{text}");
        assert_eq!(text.lines().count(), 2);
    }
}
