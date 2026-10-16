//! The planner: turns a parsed SELECT into a tree of physical operators.
//!
//! ```text
//! SELECT dept, COUNT(*) AS n FROM emp WHERE id > 10
//! GROUP BY dept HAVING COUNT(*) > 1 ORDER BY n DESC LIMIT 3
//!
//!   Limit 3
//!     Sort #1 DESC                       -- sorts on output column 1 (n)
//!       Project dept, n
//!         Filter #1 > 1                  -- HAVING, over Aggregate's output
//!           Aggregate group=[dept] aggs=[COUNT(*)]
//!             Filter id > 10             -- WHERE
//!               Scan emp range id > 10   -- primary-key range, not full scan
//! ```
//!
//! Steps, in the order SQL semantics require:
//!
//! 1. **Access path.** If WHERE constrains the primary key with constants
//!    (`pk = 5`, `pk > 3 AND pk <= 9`, `pk BETWEEN ...`), scan only that key
//!    range of the B+tree. The full WHERE is still applied afterwards, so
//!    this is purely an optimization.
//! 2. **Filter** by WHERE.
//! 3. **Aggregate** if there is a GROUP BY or any aggregate function. Its
//!    output rows are `[group values..., aggregate results...]`; every later
//!    expression is re-bound against that shape, which is how `COUNT(*) > 1`
//!    in HAVING becomes "column #1 > 1".
//! 4. **Project** the SELECT list, plus hidden extra columns for ORDER BY
//!    terms that aren't in the SELECT list.
//! 5. **Distinct**, **Sort**, **Limit**, then drop hidden columns.

use std::fmt;
use std::ops::Bound;

use super::ast::{BinOp, Expr, Select, SelectItem, UnOp};
use super::expr::{AggFn, BExpr, ScalarFn};
use crate::catalog::{Catalog, Index, Table};
use crate::error::{query_err, Result};
use crate::types::Value;

/// How to read a table.
#[derive(Clone, Debug, PartialEq)]
pub enum Access {
    /// Every row, in primary-key order.
    Full,
    /// The single row with this primary key.
    Key(Value),
    /// Rows whose primary key is within the bounds.
    Range { lo: Bound<Value>, hi: Bound<Value> },
    /// Rows found through a secondary index, using bounds on its first
    /// column (`eq` if it is constrained to a single value), in index order.
    Index {
        index: Index,
        eq: Option<Value>,
        lo: Bound<Value>,
        hi: Bound<Value>,
    },
}

/// One aggregate function call, e.g. `SUM(price * qty)`.
#[derive(Clone, Debug, PartialEq)]
pub struct AggCall {
    pub func: AggFn,
    /// `None` for `COUNT(*)`.
    pub arg: Option<BExpr>,
    pub distinct: bool,
}

/// Physical operator tree. Each node becomes one executor (see `exec.rs`).
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// Produces one empty row: the input of `SELECT 1 + 1` (no FROM).
    SingleRow,
    Scan { table: Table, access: Access },
    Filter { input: Box<Plan>, pred: BExpr, desc: String },
    Aggregate { input: Box<Plan>, groups: Vec<BExpr>, aggs: Vec<AggCall>, desc: String },
    Project { input: Box<Plan>, exprs: Vec<BExpr>, desc: String },
    Distinct { input: Box<Plan> },
    Sort { input: Box<Plan>, keys: Vec<(usize, bool)>, desc: String },
    Limit { input: Box<Plan>, limit: Option<u64>, offset: u64 },
}

/// A planned query: the operator tree and its output column names.
#[derive(Clone, Debug)]
pub struct QueryPlan {
    pub plan: Plan,
    pub columns: Vec<String>,
}

// --- binding -----------------------------------------------------------------------

/// Resolves column names to positions. In aggregate mode, expressions are
/// resolved against the Aggregate operator's output instead of the table.
pub struct Binder<'a> {
    table: Option<&'a Table>,
    agg: Option<(&'a [Expr], &'a [Expr])>,
    /// Where we are, for error messages ("WHERE", "SET", ...).
    context: &'static str,
}

pub fn is_aggregate(e: &Expr) -> bool {
    matches!(e, Expr::Function { name, .. } if AggFn::lookup(name).is_some())
}

impl<'a> Binder<'a> {
    pub fn new(table: Option<&'a Table>, context: &'static str) -> Self {
        Binder {
            table,
            agg: None,
            context,
        }
    }

    pub fn bind(&self, e: &Expr) -> Result<BExpr> {
        if let Some((groups, aggs)) = self.agg {
            if let Some(i) = groups.iter().position(|g| g == e) {
                return Ok(BExpr::Col(i));
            }
            if is_aggregate(e) {
                let j = aggs.iter().position(|a| a == e).expect("aggregates were collected");
                return Ok(BExpr::Col(groups.len() + j));
            }
            if let Expr::Column(..) = e {
                return Err(query_err!(
                    "column {e} must appear in the GROUP BY clause or be used in an aggregate function"
                ));
            }
        } else if is_aggregate(e) {
            return Err(query_err!("aggregate functions are not allowed in {}", self.context));
        }
        let b = |x: &Expr| self.bind(x).map(Box::new);
        Ok(match e {
            Expr::Literal(v) => BExpr::Const(v.clone()),
            Expr::Column(q, name) => BExpr::Col(self.column(q.as_deref(), name)?),
            Expr::Unary(op, x) => BExpr::Unary(*op, b(x)?),
            Expr::Binary(op, l, r) => BExpr::Binary(*op, b(l)?, b(r)?),
            Expr::IsNull(x, neg) => BExpr::IsNull(b(x)?, *neg),
            Expr::InList(x, list, neg) => BExpr::InList(
                b(x)?,
                list.iter().map(|i| self.bind(i)).collect::<Result<_>>()?,
                *neg,
            ),
            Expr::Like(x, p, neg) => BExpr::Like(b(x)?, b(p)?, *neg),
            Expr::Between(x, lo, hi, neg) => {
                // x BETWEEN lo AND hi  ==>  x >= lo AND x <= hi
                let x = self.bind(x)?;
                let both = BExpr::Binary(
                    BinOp::And,
                    Box::new(BExpr::Binary(BinOp::GtEq, Box::new(x.clone()), b(lo)?)),
                    Box::new(BExpr::Binary(BinOp::LtEq, Box::new(x), b(hi)?)),
                );
                if *neg {
                    BExpr::Unary(UnOp::Not, Box::new(both))
                } else {
                    both
                }
            }
            Expr::Function { name, args, star, distinct } => {
                let Some((f, arity)) = ScalarFn::lookup(name) else {
                    return Err(query_err!("unknown function {}", name.to_uppercase()));
                };
                if *star || *distinct {
                    return Err(query_err!("{}() does not take * or DISTINCT", name.to_uppercase()));
                }
                if !arity.contains(&args.len()) {
                    return Err(query_err!(
                        "wrong number of arguments to {}()",
                        name.to_uppercase()
                    ));
                }
                BExpr::Call(f, args.iter().map(|a| self.bind(a)).collect::<Result<_>>()?)
            }
        })
    }

    fn column(&self, qualifier: Option<&str>, name: &str) -> Result<usize> {
        let Some(t) = self.table else {
            return Err(query_err!("no such column: {name} (there is no FROM clause)"));
        };
        if let Some(q) = qualifier {
            if q != t.name {
                return Err(query_err!("no such table in this query: {q}"));
            }
        }
        t.column_index(name)
            .ok_or_else(|| query_err!("no such column: {name} in table {}", t.name))
    }
}

/// Evaluate a constant expression (VALUES rows, LIMIT, ...).
pub fn eval_const(e: &Expr, context: &'static str) -> Result<Value> {
    Binder::new(None, context).bind(e)?.eval(&[])
}

// --- access path selection -----------------------------------------------------------

/// Split `a AND b AND c` into `[a, b, c]`.
fn conjuncts<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match e {
        Expr::Binary(BinOp::And, l, r) => {
            conjuncts(l, out);
            conjuncts(r, out);
        }
        e => out.push(e),
    }
}

fn tighter_lo(cur: &Bound<Value>, new: &Bound<Value>) -> bool {
    match (cur, new) {
        (Bound::Unbounded, _) => true,
        (_, Bound::Unbounded) => false,
        (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b)) => b > a,
        (Bound::Included(a), Bound::Excluded(b)) => b >= a,
        (Bound::Excluded(a), Bound::Excluded(b)) => b > a,
    }
}

fn tighter_hi(cur: &Bound<Value>, new: &Bound<Value>) -> bool {
    match (cur, new) {
        (Bound::Unbounded, _) => true,
        (_, Bound::Unbounded) => false,
        (Bound::Included(a) | Bound::Excluded(a), Bound::Included(b)) => b < a,
        (Bound::Included(a), Bound::Excluded(b)) => b <= a,
        (Bound::Excluded(a), Bound::Excluded(b)) => b < a,
    }
}

/// What the WHERE clause says about one column, as constant bounds.
struct ColumnBounds {
    eq: Option<Value>,
    lo: Bound<Value>,
    hi: Bound<Value>,
}

impl ColumnBounds {
    fn is_range(&self) -> bool {
        !matches!((&self.lo, &self.hi), (Bound::Unbounded, Bound::Unbounded))
    }
}

/// Extract `col = c`, `col < c` (either side), `col BETWEEN c1 AND c2`
/// constraints on column `col` from the AND-ed parts of the filter.
fn column_bounds(table: &Table, parts: &[&Expr], col: usize) -> ColumnBounds {
    let column = &table.columns[col];
    let is_col = |e: &Expr| {
        matches!(e, Expr::Column(q, n)
            if n == &column.name && q.as_ref().is_none_or(|q| q == &table.name))
    };
    // Only use constants of exactly the column's type: the key encoding
    // orders values within one type, so `id < 2.5` on an INTEGER column
    // can't become a byte range. Such predicates still work, via a scan.
    let usable = |e: &Expr| match e {
        Expr::Literal(v) if v.data_type() == Some(column.ty) => Some(v.clone()),
        _ => None,
    };
    let mut b = ColumnBounds { eq: None, lo: Bound::Unbounded, hi: Bound::Unbounded };
    let offer = |b: &mut ColumnBounds, lo_new: Option<Bound<Value>>, hi_new: Option<Bound<Value>>| {
        if let Some(n) = lo_new {
            if tighter_lo(&b.lo, &n) {
                b.lo = n;
            }
        }
        if let Some(n) = hi_new {
            if tighter_hi(&b.hi, &n) {
                b.hi = n;
            }
        }
    };
    for part in parts {
        match part {
            Expr::Binary(op, l, r) => {
                let (op, v) = if let (true, Some(v)) = (is_col(l), usable(r)) {
                    (*op, v)
                } else if let (Some(v), true) = (usable(l), is_col(r)) {
                    // `5 < id` is `id > 5`
                    let flipped = match op {
                        BinOp::Lt => BinOp::Gt,
                        BinOp::LtEq => BinOp::GtEq,
                        BinOp::Gt => BinOp::Lt,
                        BinOp::GtEq => BinOp::LtEq,
                        o => *o,
                    };
                    (flipped, v)
                } else {
                    continue;
                };
                match op {
                    BinOp::Eq => {
                        if b.eq.is_none() {
                            b.eq = Some(v);
                        }
                    }
                    BinOp::Lt => offer(&mut b, None, Some(Bound::Excluded(v))),
                    BinOp::LtEq => offer(&mut b, None, Some(Bound::Included(v))),
                    BinOp::Gt => offer(&mut b, Some(Bound::Excluded(v)), None),
                    BinOp::GtEq => offer(&mut b, Some(Bound::Included(v)), None),
                    _ => {}
                }
            }
            Expr::Between(x, lo, hi, false) if is_col(x) => {
                if let (Some(lo), Some(hi)) = (usable(lo), usable(hi)) {
                    offer(&mut b, Some(Bound::Included(lo)), Some(Bound::Included(hi)));
                }
            }
            _ => {}
        }
    }
    b
}

/// Pick a way to read `table` given a WHERE clause. A simple rule-based
/// choice, best first:
///
/// 1. primary key equality (one B+tree lookup)
/// 2. equality on the first column of a secondary index (unique preferred)
/// 3. primary key range
/// 4. range on the first column of a secondary index
/// 5. full scan
///
/// The whole WHERE clause is always re-checked on the rows produced, so the
/// access path only has to return a superset of the matching rows.
pub fn choose_access(table: &Table, filter: Option<&Expr>) -> Access {
    let Some(filter) = filter else {
        return Access::Full;
    };
    let mut parts = vec![];
    conjuncts(filter, &mut parts);
    let pk = table.pk.map(|pk| column_bounds(table, &parts, pk));
    if let Some(ColumnBounds { eq: Some(v), .. }) = pk {
        return Access::Key(v);
    }
    let mut indexed: Vec<(&Index, ColumnBounds)> = table
        .indexes
        .iter()
        .map(|ix| (ix, column_bounds(table, &parts, ix.columns[0])))
        .collect();
    // Unique indexes first: an equality on them matches at most one row.
    indexed.sort_by_key(|(ix, _)| !ix.unique);
    let index_access = |ix: &Index, b: ColumnBounds| Access::Index {
        index: ix.clone(),
        eq: b.eq,
        lo: b.lo,
        hi: b.hi,
    };
    if let Some(pos) = indexed.iter().position(|(_, b)| b.eq.is_some()) {
        let (ix, b) = indexed.swap_remove(pos);
        return index_access(ix, b);
    }
    if let Some(b) = pk {
        if b.is_range() {
            return Access::Range { lo: b.lo, hi: b.hi };
        }
    }
    if let Some(pos) = indexed.iter().position(|(_, b)| b.is_range()) {
        let (ix, b) = indexed.swap_remove(pos);
        return index_access(ix, b);
    }
    Access::Full
}

// --- SELECT planning ---------------------------------------------------------------------

/// Output column name for a select item.
fn item_name(e: &Expr, alias: &Option<String>) -> String {
    match (alias, e) {
        (Some(a), _) => a.clone(),
        (None, Expr::Column(_, c)) => c.clone(),
        (None, e) => e.to_string(),
    }
}

/// Non-negative integer constant for LIMIT/OFFSET.
fn count_const(e: &Expr, what: &'static str) -> Result<u64> {
    match eval_const(e, what)? {
        Value::Integer(n) if n >= 0 => Ok(n as u64),
        v => Err(query_err!("{what} must be a non-negative integer, got {v}")),
    }
}

pub fn plan_select(catalog: &Catalog, sel: &Select) -> Result<QueryPlan> {
    let table = match &sel.from {
        Some(name) => Some(catalog.get(name)?),
        None => None,
    };
    let mut plan = match table {
        Some(t) => Plan::Scan {
            table: t.clone(),
            access: choose_access(t, sel.filter.as_ref()),
        },
        None => Plan::SingleRow,
    };
    let row_binder = Binder::new(table, "WHERE");
    if let Some(f) = &sel.filter {
        plan = Plan::Filter {
            input: Box::new(plan),
            pred: row_binder.bind(f)?,
            desc: f.to_string(),
        };
    }

    // Expand `*` into the table's columns.
    let mut items: Vec<(Expr, String)> = vec![];
    for item in &sel.items {
        match item {
            SelectItem::Wildcard => {
                let t = table.ok_or_else(|| query_err!("SELECT * requires a FROM clause"))?;
                for c in &t.columns {
                    items.push((Expr::Column(None, c.name.clone()), c.name.clone()));
                }
            }
            SelectItem::Expr(e, alias) => items.push((e.clone(), item_name(e, alias))),
        }
    }

    // SELECT-list aliases may be used in GROUP BY and HAVING (as in SQLite;
    // PostgreSQL allows them in GROUP BY). A real column of the same name
    // takes precedence. GROUP BY may also use positions (`GROUP BY 1`).
    let aliases: Vec<(String, Expr)> = sel
        .items
        .iter()
        .filter_map(|i| match i {
            SelectItem::Expr(e, Some(a)) if table.is_none_or(|t| t.column_index(a).is_none()) => {
                Some((a.clone(), e.clone()))
            }
            _ => None,
        })
        .collect();
    let expand_aliases = |e: &Expr| {
        e.transform(&|x| match x {
            Expr::Column(None, name) => aliases.iter().find(|(a, _)| a == name).map(|(_, e)| e.clone()),
            _ => None,
        })
    };
    let group_by: Vec<Expr> = sel
        .group_by
        .iter()
        .map(|g| match g {
            Expr::Literal(Value::Integer(k)) => {
                if *k < 1 || *k as usize > items.len() {
                    Err(query_err!("GROUP BY position {k} is out of range"))
                } else {
                    Ok(items[*k as usize - 1].0.clone())
                }
            }
            g => Ok(expand_aliases(g)),
        })
        .collect::<Result<_>>()?;
    let having = sel.having.as_ref().map(expand_aliases);

    // Aggregation.
    let mut agg_exprs: Vec<Expr> = vec![];
    {
        let mut visit = |e: &Expr| -> Result<()> {
            let mut err = None;
            e.walk(&mut |x| {
                if is_aggregate(x) {
                    if let Expr::Function { args, .. } = x {
                        if args.iter().any(|a| {
                            let mut nested = false;
                            a.walk(&mut |y| nested |= is_aggregate(y));
                            nested
                        }) {
                            err = Some(query_err!("aggregate functions cannot be nested"));
                        }
                    }
                    if !agg_exprs.contains(x) {
                        agg_exprs.push(x.clone());
                    }
                }
            });
            err.map_or(Ok(()), Err)
        };
        for (e, _) in &items {
            visit(e)?;
        }
        if let Some(h) = &having {
            visit(h)?;
        }
        for o in &sel.order_by {
            visit(&o.expr)?;
        }
    }
    let aggregating = !group_by.is_empty() || !agg_exprs.is_empty();
    let mut binder = Binder::new(table, "this context");
    if aggregating {
        let group_binder = Binder::new(table, "GROUP BY");
        let groups = group_by
            .iter()
            .map(|g| group_binder.bind(g))
            .collect::<Result<Vec<_>>>()?;
        let arg_binder = Binder::new(table, "aggregate arguments");
        let mut aggs = vec![];
        for a in &agg_exprs {
            let Expr::Function { name, args, distinct, star } = a else { unreachable!() };
            let func = AggFn::lookup(name).unwrap();
            let arg = if *star {
                if func != AggFn::Count {
                    return Err(query_err!("only COUNT accepts *"));
                }
                None
            } else if args.len() == 1 {
                Some(arg_binder.bind(&args[0])?)
            } else {
                return Err(query_err!("{} takes exactly one argument", func));
            };
            aggs.push(AggCall { func, arg, distinct: *distinct });
        }
        let desc = format!(
            "group by [{}] aggregates [{}]",
            group_by.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(", "),
            agg_exprs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", ")
        );
        plan = Plan::Aggregate {
            input: Box::new(plan),
            groups,
            aggs,
            desc,
        };
        binder.agg = Some((&group_by, &agg_exprs));
    } else if having.is_some() {
        return Err(query_err!("HAVING requires GROUP BY or an aggregate function"));
    }
    if let Some(h) = &having {
        binder.context = "HAVING";
        plan = Plan::Filter {
            input: Box::new(plan),
            pred: binder.bind(h)?,
            desc: h.to_string(),
        };
    }

    // Projection, with hidden columns for ORDER BY terms not in the list.
    binder.context = "the SELECT list";
    let mut exprs = items
        .iter()
        .map(|(e, _)| binder.bind(e))
        .collect::<Result<Vec<_>>>()?;
    let mut names: Vec<String> = items.iter().map(|(_, n)| n.clone()).collect();
    let visible = exprs.len();
    let mut sort_keys = vec![];
    binder.context = "ORDER BY";
    for o in &sel.order_by {
        let idx = match &o.expr {
            Expr::Literal(Value::Integer(k)) => {
                if *k < 1 || *k as usize > visible {
                    return Err(query_err!("ORDER BY position {k} is out of range"));
                }
                *k as usize - 1
            }
            Expr::Column(None, name)
                if sel.items.iter().any(|i| matches!(i, SelectItem::Expr(_, Some(a)) if a == name)) =>
            {
                names.iter().position(|n| n == name).unwrap()
            }
            e => match items.iter().position(|(ie, _)| ie == e) {
                Some(i) => i,
                None => {
                    if sel.distinct {
                        return Err(query_err!(
                            "with SELECT DISTINCT, ORDER BY terms must appear in the select list"
                        ));
                    }
                    exprs.push(binder.bind(e)?);
                    names.push(e.to_string());
                    exprs.len() - 1
                }
            },
        };
        sort_keys.push((idx, o.asc));
    }
    plan = Plan::Project {
        input: Box::new(plan),
        exprs,
        desc: names.join(", "),
    };
    if sel.distinct {
        plan = Plan::Distinct { input: Box::new(plan) };
    }
    if !sort_keys.is_empty() {
        let desc = sort_keys
            .iter()
            .map(|(i, asc)| format!("{}{}", names[*i], if *asc { "" } else { " DESC" }))
            .collect::<Vec<_>>()
            .join(", ");
        plan = Plan::Sort {
            input: Box::new(plan),
            keys: sort_keys,
            desc,
        };
    }
    if sel.limit.is_some() || sel.offset.is_some() {
        plan = Plan::Limit {
            input: Box::new(plan),
            limit: sel.limit.as_ref().map(|l| count_const(l, "LIMIT")).transpose()?,
            offset: sel.offset.as_ref().map(|o| count_const(o, "OFFSET")).transpose()?.unwrap_or(0),
        };
    }
    if names.len() > visible {
        names.truncate(visible);
        plan = Plan::Project {
            input: Box::new(plan),
            exprs: (0..visible).map(BExpr::Col).collect(),
            desc: names.join(", "),
        };
    }
    Ok(QueryPlan {
        plan,
        columns: names,
    })
}

// --- EXPLAIN output -----------------------------------------------------------------------

fn fmt_bound(col: &str, b: &Bound<Value>, lower: bool) -> Option<String> {
    match (b, lower) {
        (Bound::Unbounded, _) => None,
        (Bound::Included(v), true) => Some(format!("{col} >= {}", v.to_sql())),
        (Bound::Excluded(v), true) => Some(format!("{col} > {}", v.to_sql())),
        (Bound::Included(v), false) => Some(format!("{col} <= {}", v.to_sql())),
        (Bound::Excluded(v), false) => Some(format!("{col} < {}", v.to_sql())),
    }
}

impl Plan {
    fn fmt_indented(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        let pad = "  ".repeat(depth);
        let child = match self {
            Plan::SingleRow => {
                writeln!(f, "{pad}SingleRow")?;
                None
            }
            Plan::Scan { table, access } => {
                let pk = table.pk.map(|i| table.columns[i].name.as_str()).unwrap_or("rowid");
                match access {
                    Access::Full => writeln!(f, "{pad}Scan {} (full)", table.name)?,
                    Access::Key(v) => {
                        writeln!(f, "{pad}Scan {} (point lookup {pk} = {})", table.name, v.to_sql())?
                    }
                    Access::Range { lo, hi } => {
                        let parts: Vec<String> = [fmt_bound(pk, lo, true), fmt_bound(pk, hi, false)]
                            .into_iter()
                            .flatten()
                            .collect();
                        writeln!(f, "{pad}Scan {} (range {})", table.name, parts.join(" AND "))?
                    }
                    Access::Index { index, eq, lo, hi } => {
                        let col = table.columns[index.columns[0]].name.as_str();
                        let cond = match eq {
                            Some(v) => format!("{col} = {}", v.to_sql()),
                            None => [fmt_bound(col, lo, true), fmt_bound(col, hi, false)]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" AND "),
                        };
                        writeln!(f, "{pad}IndexScan {} using {} ({cond})", table.name, index.name)?
                    }
                }
                None
            }
            Plan::Filter { input, desc, .. } => {
                writeln!(f, "{pad}Filter {desc}")?;
                Some(input)
            }
            Plan::Aggregate { input, desc, .. } => {
                writeln!(f, "{pad}Aggregate {desc}")?;
                Some(input)
            }
            Plan::Project { input, desc, .. } => {
                writeln!(f, "{pad}Project {desc}")?;
                Some(input)
            }
            Plan::Distinct { input } => {
                writeln!(f, "{pad}Distinct")?;
                Some(input)
            }
            Plan::Sort { input, desc, .. } => {
                writeln!(f, "{pad}Sort {desc}")?;
                Some(input)
            }
            Plan::Limit { input, limit, offset } => {
                match limit {
                    Some(l) => write!(f, "{pad}Limit {l}")?,
                    None => write!(f, "{pad}Limit none")?,
                }
                if *offset > 0 {
                    write!(f, " offset {offset}")?;
                }
                writeln!(f)?;
                Some(input)
            }
        };
        match child {
            Some(c) => c.fmt_indented(f, depth + 1),
            None => Ok(()),
        }
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_indented(f, 0)
    }
}
