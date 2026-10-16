//! Pretty-printing query results as ASCII tables (used by the REPL and the
//! examples).
//!
//! ```text
//! +----+-------+-------+
//! | id | name  | score |
//! +----+-------+-------+
//! |  1 | Ada   |   9.5 |
//! |  2 | Grace |  NULL |
//! +----+-------+-------+
//! (2 rows)
//! ```

use crate::sql::exec::QueryResult;
use crate::types::Value;

/// Render a result set as an ASCII table. Numbers are right-aligned.
pub fn table(columns: &[String], rows: &[Vec<Value>]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|v| v.to_string().replace('\n', "\\n")).collect())
        .collect();
    let width = |s: &str| s.chars().count();
    let mut widths: Vec<usize> = columns.iter().map(|c| width(c)).collect();
    for row in &cells {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(width(c));
        }
    }
    let rule: String = {
        let mut s = String::from("+");
        for w in &widths {
            s.push_str(&"-".repeat(w + 2));
            s.push('+');
        }
        s.push('\n');
        s
    };
    let mut out = rule.clone();
    out.push('|');
    for (c, w) in columns.iter().zip(&widths) {
        out.push_str(&format!(" {c}{} |", " ".repeat(w - width(c))));
    }
    out.push('\n');
    out.push_str(&rule);
    for (row, vals) in cells.iter().zip(rows) {
        out.push('|');
        for ((c, w), v) in row.iter().zip(&widths).zip(vals) {
            let pad = " ".repeat(w - width(c));
            if matches!(v, Value::Integer(_) | Value::Real(_)) {
                out.push_str(&format!(" {pad}{c} |"));
            } else {
                out.push_str(&format!(" {c}{pad} |"));
            }
        }
        out.push('\n');
    }
    if !rows.is_empty() {
        out.push_str(&rule);
    }
    out.push_str(&match rows.len() {
        1 => "(1 row)\n".to_string(),
        n => format!("({n} rows)\n"),
    });
    out
}

/// Render any statement result for display.
pub fn result(r: &QueryResult) -> String {
    match r {
        QueryResult::Rows { columns, rows } => table(columns, rows),
        QueryResult::Modified { verb, count } => {
            format!("{verb} {count} row{}\n", if *count == 1 { "" } else { "s" })
        }
        QueryResult::Done(msg) => format!("{msg}\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_table() {
        let t = table(
            &["id".into(), "name".into()],
            &[
                vec![Value::Integer(1), Value::Text("Ada".into())],
                vec![Value::Integer(10), Value::Null],
            ],
        );
        assert_eq!(
            t,
            "+----+------+\n| id | name |\n+----+------+\n|  1 | Ada  |\n| 10 | NULL |\n+----+------+\n(2 rows)\n"
        );
    }
}
