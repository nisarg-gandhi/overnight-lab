//! Bound expressions and their evaluation.
//!
//! The parser produces [`ast::Expr`](super::ast::Expr) trees that refer to
//! columns by *name*. The planner *binds* them into [`BExpr`] trees that
//! refer to columns by *position* in the input row, so evaluation is a
//! simple recursive walk with no lookups.
//!
//! NULL handling follows SQL's three-valued logic: most operators return
//! NULL if any input is NULL, `AND`/`OR` use the truth tables below, and a
//! WHERE clause keeps a row only if its predicate is TRUE (not NULL).
//!
//! ```text
//!   AND   | T  F  N        OR    | T  F  N
//!   ------+---------       ------+---------
//!   T     | T  F  N        T     | T  T  T
//!   F     | F  F  F        F     | T  F  N
//!   N     | N  F  N        N     | T  N  N
//! ```

use std::cmp::Ordering;
use std::fmt;

use super::ast::{BinOp, UnOp};
use crate::error::{query_err, Error, Result};
use crate::types::Value;

/// Built-in scalar functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarFn {
    Abs,
    Lower,
    Upper,
    Length,
    Coalesce,
    Typeof,
    Round,
    Substr,
}

impl ScalarFn {
    pub fn lookup(name: &str) -> Option<(ScalarFn, std::ops::RangeInclusive<usize>)> {
        Some(match name {
            "abs" => (ScalarFn::Abs, 1..=1),
            "lower" => (ScalarFn::Lower, 1..=1),
            "upper" => (ScalarFn::Upper, 1..=1),
            "length" => (ScalarFn::Length, 1..=1),
            "coalesce" => (ScalarFn::Coalesce, 1..=usize::MAX),
            "typeof" => (ScalarFn::Typeof, 1..=1),
            "round" => (ScalarFn::Round, 1..=2),
            "substr" | "substring" => (ScalarFn::Substr, 2..=3),
            _ => return None,
        })
    }
}

/// An expression whose column references have been resolved to indexes.
#[derive(Clone, Debug, PartialEq)]
pub enum BExpr {
    Const(Value),
    Col(usize),
    Unary(UnOp, Box<BExpr>),
    Binary(BinOp, Box<BExpr>, Box<BExpr>),
    IsNull(Box<BExpr>, bool),
    InList(Box<BExpr>, Vec<BExpr>, bool),
    Like(Box<BExpr>, Box<BExpr>, bool),
    Call(ScalarFn, Vec<BExpr>),
}

fn truth(v: &Value) -> Result<Option<bool>> {
    match v {
        Value::Null => Ok(None),
        Value::Boolean(b) => Ok(Some(*b)),
        v => Err(query_err!("expected a boolean, got {} {v}", v.type_name())),
    }
}

fn from_truth(t: Option<bool>) -> Value {
    t.map_or(Value::Null, Value::Boolean)
}

impl BExpr {
    /// Evaluate against one input row.
    pub fn eval(&self, row: &[Value]) -> Result<Value> {
        Ok(match self {
            BExpr::Const(v) => v.clone(),
            BExpr::Col(i) => row[*i].clone(),
            BExpr::Unary(op, e) => {
                let v = e.eval(row)?;
                match (op, v) {
                    (_, Value::Null) => Value::Null,
                    (UnOp::Not, v) => from_truth(truth(&v)?.map(|b| !b)),
                    (UnOp::Neg, Value::Integer(i)) => Value::Integer(
                        i.checked_neg().ok_or_else(|| query_err!("integer overflow"))?,
                    ),
                    (UnOp::Neg, Value::Real(r)) => Value::Real(-r),
                    (UnOp::Plus, v @ (Value::Integer(_) | Value::Real(_))) => v,
                    (_, v) => return Err(query_err!("cannot negate {}", v.type_name())),
                }
            }
            BExpr::Binary(BinOp::And, l, r) => {
                // Short-circuit: FALSE AND anything is FALSE.
                let a = truth(&l.eval(row)?)?;
                if a == Some(false) {
                    return Ok(Value::Boolean(false));
                }
                let b = truth(&r.eval(row)?)?;
                from_truth(match (a, b) {
                    (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                })
            }
            BExpr::Binary(BinOp::Or, l, r) => {
                let a = truth(&l.eval(row)?)?;
                if a == Some(true) {
                    return Ok(Value::Boolean(true));
                }
                let b = truth(&r.eval(row)?)?;
                from_truth(match (a, b) {
                    (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                })
            }
            BExpr::Binary(op, l, r) => binary(*op, l.eval(row)?, r.eval(row)?)?,
            BExpr::IsNull(e, negated) => Value::Boolean(e.eval(row)?.is_null() != *negated),
            BExpr::InList(e, list, negated) => {
                // x IN (a, b) == x = a OR x = b, with the same NULL rules.
                let v = e.eval(row)?;
                let mut result = Some(false);
                for item in list {
                    match v.sql_cmp(&item.eval(row)?)? {
                        Some(Ordering::Equal) => {
                            result = Some(true);
                            break;
                        }
                        None => result = None,
                        Some(_) => {}
                    }
                }
                from_truth(result.map(|b| b != *negated))
            }
            BExpr::Like(e, p, negated) => match (e.eval(row)?, p.eval(row)?) {
                (Value::Null, _) | (_, Value::Null) => Value::Null,
                (Value::Text(s), Value::Text(p)) => Value::Boolean(like(&s, &p) != *negated),
                (a, b) => {
                    return Err(query_err!(
                        "LIKE needs TEXT operands, got {} and {}",
                        a.type_name(),
                        b.type_name()
                    ))
                }
            },
            BExpr::Call(f, args) => {
                let vals = args.iter().map(|a| a.eval(row)).collect::<Result<Vec<_>>>()?;
                call(*f, vals)?
            }
        })
    }

    /// Evaluate as a WHERE/HAVING predicate: only TRUE passes.
    pub fn matches(&self, row: &[Value]) -> Result<bool> {
        Ok(truth(&self.eval(row)?)? == Some(true))
    }
}

fn binary(op: BinOp, a: Value, b: Value) -> Result<Value> {
    use Value::*;
    if a.is_null() || b.is_null() {
        return Ok(Null);
    }
    let cmp = |want: fn(Ordering) -> bool| -> Result<Value> {
        Ok(from_truth(a.sql_cmp(&b)?.map(want)))
    };
    match op {
        BinOp::Eq => cmp(|o| o == Ordering::Equal),
        BinOp::NotEq => cmp(|o| o != Ordering::Equal),
        BinOp::Lt => cmp(|o| o == Ordering::Less),
        BinOp::LtEq => cmp(|o| o != Ordering::Greater),
        BinOp::Gt => cmp(|o| o == Ordering::Greater),
        BinOp::GtEq => cmp(|o| o != Ordering::Less),
        BinOp::Concat => Ok(Text(format!("{a}{b}"))),
        BinOp::And | BinOp::Or => unreachable!("handled in eval"),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => arith(op, a, b),
    }
}

fn arith(op: BinOp, a: Value, b: Value) -> Result<Value> {
    use Value::*;
    let overflow = || query_err!("integer overflow");
    match (a, b) {
        (Integer(x), Integer(y)) => Ok(Integer(match op {
            BinOp::Add => x.checked_add(y).ok_or_else(overflow)?,
            BinOp::Sub => x.checked_sub(y).ok_or_else(overflow)?,
            BinOp::Mul => x.checked_mul(y).ok_or_else(overflow)?,
            BinOp::Div if y == 0 => return Err(query_err!("division by zero")),
            BinOp::Div => x.checked_div(y).ok_or_else(overflow)?,
            BinOp::Mod if y == 0 => return Err(query_err!("division by zero")),
            BinOp::Mod => x.checked_rem(y).ok_or_else(overflow)?,
            _ => unreachable!(),
        })),
        (x @ (Integer(_) | Real(_)), y @ (Integer(_) | Real(_))) => {
            let f = |v: Value| match v {
                Integer(i) => i as f64,
                Real(r) => r,
                _ => unreachable!(),
            };
            let (x, y) = (f(x), f(y));
            if matches!(op, BinOp::Div | BinOp::Mod) && y == 0.0 {
                return Err(query_err!("division by zero"));
            }
            Ok(Real(match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => x / y,
                BinOp::Mod => x % y,
                _ => unreachable!(),
            }))
        }
        (a, b) => Err(query_err!(
            "cannot apply arithmetic to {} and {}",
            a.type_name(),
            b.type_name()
        )),
    }
}

/// SQL LIKE: `%` matches any run of characters, `_` exactly one. Matching is
/// case-sensitive. Classic two-pointer algorithm with backtracking to the
/// last `%`, O(n*m) worst case.
pub fn like(s: &str, pattern: &str) -> bool {
    let s: Vec<char> = s.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut i, mut j) = (0, 0);
    let mut star: Option<(usize, usize)> = None; // (pattern pos after %, string pos)
    while i < s.len() {
        if j < p.len() && (p[j] == '_' || (p[j] != '%' && p[j] == s[i])) {
            i += 1;
            j += 1;
        } else if j < p.len() && p[j] == '%' {
            star = Some((j + 1, i));
            j += 1;
        } else if let Some((pj, si)) = star {
            // Let the last % swallow one more character and retry.
            j = pj;
            i = si + 1;
            star = Some((pj, si + 1));
        } else {
            return false;
        }
    }
    p[j..].iter().all(|&c| c == '%')
}

fn call(f: ScalarFn, mut args: Vec<Value>) -> Result<Value> {
    use Value::*;
    if f == ScalarFn::Coalesce {
        return Ok(args.into_iter().find(|v| !v.is_null()).unwrap_or(Null));
    }
    if f == ScalarFn::Typeof {
        return Ok(Text(args[0].type_name().to_lowercase()));
    }
    if args.iter().any(Value::is_null) {
        return Ok(Null);
    }
    let bad = |args: &[Value]| -> Error {
        let types: Vec<&str> = args.iter().map(Value::type_name).collect();
        query_err!("invalid argument types for {f:?}: {}", types.join(", "))
    };
    Ok(match (f, args.as_slice()) {
        (ScalarFn::Abs, [Integer(i)]) => {
            Integer(i.checked_abs().ok_or_else(|| query_err!("integer overflow"))?)
        }
        (ScalarFn::Abs, [Real(r)]) => Real(r.abs()),
        (ScalarFn::Lower, [Text(s)]) => Text(s.to_lowercase()),
        (ScalarFn::Upper, [Text(s)]) => Text(s.to_uppercase()),
        (ScalarFn::Length, [Text(s)]) => Integer(s.chars().count() as i64),
        (ScalarFn::Round, [Integer(i)]) => Integer(*i),
        (ScalarFn::Round, [Real(r)]) => Real(r.round()),
        (ScalarFn::Round, [Integer(i), Integer(_)]) => Integer(*i),
        (ScalarFn::Round, [Real(r), Integer(d)]) => {
            let m = 10f64.powi((*d).clamp(-30, 30) as i32);
            Real((r * m).round() / m)
        }
        (ScalarFn::Substr, [Text(_), Integer(_)] | [Text(_), Integer(_), Integer(_)]) => {
            // SQL strings are 1-indexed.
            let len = if args.len() == 3 {
                match args.pop() {
                    Some(Integer(l)) => Some(l.max(0) as usize),
                    _ => unreachable!(),
                }
            } else {
                None
            };
            let (Some(Integer(start)), Some(Text(s))) = (args.pop(), args.pop()) else {
                unreachable!()
            };
            let skip = (start.max(1) - 1) as usize;
            let it = s.chars().skip(skip);
            Text(match len {
                Some(l) => it.take(l).collect(),
                None => it.collect(),
            })
        }
        _ => return Err(bad(&args)),
    })
}

// --- aggregates ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggFn {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

impl AggFn {
    pub fn lookup(name: &str) -> Option<AggFn> {
        Some(match name {
            "count" => AggFn::Count,
            "sum" => AggFn::Sum,
            "avg" => AggFn::Avg,
            "min" => AggFn::Min,
            "max" => AggFn::Max,
            _ => return None,
        })
    }
}

impl fmt::Display for AggFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format!("{self:?}").to_uppercase())
    }
}

/// Running state of one aggregate over one group.
#[derive(Clone, Debug)]
pub enum Accumulator {
    Count(i64),
    /// Integer sum until a REAL shows up, then floating point.
    Sum(Option<Value>),
    Avg { sum: f64, n: i64 },
    Min(Option<Value>),
    Max(Option<Value>),
}

impl Accumulator {
    pub fn new(f: AggFn) -> Self {
        match f {
            AggFn::Count => Accumulator::Count(0),
            AggFn::Sum => Accumulator::Sum(None),
            AggFn::Avg => Accumulator::Avg { sum: 0.0, n: 0 },
            AggFn::Min => Accumulator::Min(None),
            AggFn::Max => Accumulator::Max(None),
        }
    }

    /// Feed one input value. `COUNT(*)` feeds a non-NULL dummy per row; all
    /// aggregates ignore NULLs.
    pub fn add(&mut self, v: Value) -> Result<()> {
        if v.is_null() {
            return Ok(());
        }
        match self {
            Accumulator::Count(n) => *n += 1,
            Accumulator::Sum(acc) => {
                if !matches!(v, Value::Integer(_) | Value::Real(_)) {
                    return Err(query_err!("SUM of non-numeric value {}", v.type_name()));
                }
                *acc = Some(match acc.take() {
                    None => v,
                    Some(a) => arith(BinOp::Add, a, v)?,
                });
            }
            Accumulator::Avg { sum, n } => {
                *sum += match v {
                    Value::Integer(i) => i as f64,
                    Value::Real(r) => r,
                    v => return Err(query_err!("AVG of non-numeric value {}", v.type_name())),
                };
                *n += 1;
            }
            Accumulator::Min(m) => {
                if m.as_ref().is_none_or(|cur| v.total_cmp(cur) == Ordering::Less) {
                    *m = Some(v);
                }
            }
            Accumulator::Max(m) => {
                if m.as_ref().is_none_or(|cur| v.total_cmp(cur) == Ordering::Greater) {
                    *m = Some(v);
                }
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Value {
        match self {
            Accumulator::Count(n) => Value::Integer(n),
            Accumulator::Sum(v) | Accumulator::Min(v) | Accumulator::Max(v) => v.unwrap_or(Value::Null),
            Accumulator::Avg { n: 0, .. } => Value::Null,
            Accumulator::Avg { sum, n } => Value::Real(sum / n as f64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns() {
        assert!(like("hello", "h%"));
        assert!(like("hello", "%llo"));
        assert!(like("hello", "h_l%o"));
        assert!(like("", "%"));
        assert!(!like("hello", "h_"));
        assert!(like("abcabc", "%bc%bc"));
        assert!(!like("abc", "abcd"));
        assert!(like("a%b", "a%b"));
    }

    #[test]
    fn three_valued_logic() {
        let t = BExpr::Const(Value::Boolean(true));
        let f = BExpr::Const(Value::Boolean(false));
        let n = BExpr::Const(Value::Null);
        let and = |a: &BExpr, b: &BExpr| BExpr::Binary(BinOp::And, Box::new(a.clone()), Box::new(b.clone())).eval(&[]).unwrap();
        let or = |a: &BExpr, b: &BExpr| BExpr::Binary(BinOp::Or, Box::new(a.clone()), Box::new(b.clone())).eval(&[]).unwrap();
        assert!(and(&n, &f) == Value::Boolean(false));
        assert!(and(&n, &t).is_null());
        assert!(or(&n, &t) == Value::Boolean(true));
        assert!(or(&n, &f).is_null());
        assert!(!BExpr::IsNull(Box::new(n.clone()), true).matches(&[]).unwrap());
    }

    #[test]
    fn arithmetic() {
        assert_eq!(arith(BinOp::Div, Value::Integer(7), Value::Integer(2)).unwrap(), Value::Integer(3));
        assert_eq!(arith(BinOp::Div, Value::Integer(7), Value::Real(2.0)).unwrap(), Value::Real(3.5));
        assert!(arith(BinOp::Div, Value::Integer(1), Value::Integer(0)).is_err());
        assert!(arith(BinOp::Add, Value::Integer(i64::MAX), Value::Integer(1)).is_err());
    }

    #[test]
    fn aggregates_ignore_nulls() {
        let mut s = Accumulator::new(AggFn::Sum);
        for v in [Value::Integer(1), Value::Null, Value::Real(2.5)] {
            s.add(v).unwrap();
        }
        assert_eq!(s.finish(), Value::Real(3.5));
        assert!(Accumulator::new(AggFn::Avg).finish().is_null());
        assert_eq!(Accumulator::new(AggFn::Count).finish(), Value::Integer(0));
    }
}
