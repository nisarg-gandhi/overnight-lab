//! The executor: Volcano-style iterators plus DDL/DML statement execution.
//!
//! In the *Volcano* (or *iterator*) model every operator implements one
//! method, `next()`, which returns the operator's next output row. A parent
//! pulls rows from its child one at a time:
//!
//! ```text
//!   caller ── next() ──> Limit ── next() ──> Filter ── next() ──> Scan ── B+tree cursor
//!          <── row ────        <── row ────         <── row ────
//! ```
//!
//! Most operators stream (Scan, Filter, Project, Limit hold one row at a
//! time). Sort and Aggregate are *pipeline breakers*: they must consume
//! their whole input before producing the first row.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;

use super::ast::{CreateTable, Expr, Statement};
use super::expr::{Accumulator, BExpr};
use super::planner::{
    choose_access, eval_const, plan_select, Access, AggCall, Binder, Plan, QueryPlan,
};
use crate::btree::{BTree, Cursor};
use crate::catalog::{Catalog, Table, SCHEMA_TABLE};
use crate::error::{query_err, Error, Result};
use crate::storage::{PageStore, Storage};
use crate::types::{decode_int_key, decode_row, encode_key, encode_row, Row, Value};

/// The result of executing one statement.
#[derive(Clone, Debug, PartialEq)]
pub enum QueryResult {
    /// A query's result set.
    Rows { columns: Vec<String>, rows: Vec<Row> },
    /// INSERT/UPDATE/DELETE: how many rows were affected.
    Modified { verb: &'static str, count: u64 },
    /// Anything else (CREATE TABLE, BEGIN, ...): a status message.
    Done(String),
}

/// A Volcano operator.
pub trait Executor {
    /// Produce the next row, or `None` when exhausted.
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>>;
}

/// Build the executor tree for a plan.
pub fn build(plan: Plan) -> Box<dyn Executor> {
    match plan {
        Plan::SingleRow => Box::new(SingleRowExec { done: false }),
        Plan::Scan { table, access } => Box::new(ScanExec::new(&table, access)),
        Plan::Filter { input, pred, .. } => Box::new(FilterExec { input: build(*input), pred }),
        Plan::Project { input, exprs, .. } => Box::new(ProjectExec { input: build(*input), exprs }),
        Plan::Aggregate { input, groups, aggs, .. } => Box::new(AggregateExec {
            input: build(*input),
            groups,
            aggs,
            output: None,
        }),
        Plan::Distinct { input } => Box::new(DistinctExec {
            input: build(*input),
            seen: BTreeSet::new(),
        }),
        Plan::Sort { input, keys, .. } => Box::new(SortExec {
            input: build(*input),
            keys,
            output: None,
        }),
        Plan::Limit { input, limit, offset } => Box::new(LimitExec {
            input: build(*input),
            limit,
            offset,
            produced: 0,
        }),
    }
}

// --- operators ---------------------------------------------------------------------------

struct SingleRowExec {
    done: bool,
}

impl Executor for SingleRowExec {
    fn next(&mut self, _: &mut dyn PageStore) -> Result<Option<Row>> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        Ok(Some(vec![]))
    }
}

/// Encode an access path into B+tree key bounds.
fn key_bounds(access: &Access) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
    let enc = |b: &Bound<Value>| match b {
        Bound::Included(v) => Bound::Included(encode_key(v)),
        Bound::Excluded(v) => Bound::Excluded(encode_key(v)),
        Bound::Unbounded => Bound::Unbounded,
    };
    match access {
        Access::Full => (Bound::Unbounded, Bound::Unbounded),
        Access::Key(v) => (Bound::Included(encode_key(v)), Bound::Included(encode_key(v))),
        Access::Range { lo, hi } => (enc(lo), enc(hi)),
    }
}

/// Reads rows from a table's B+tree, optionally restricted to a key range.
pub struct ScanExec {
    tree: BTree,
    bounds: (Bound<Vec<u8>>, Bound<Vec<u8>>),
    cursor: Option<Cursor>,
}

impl ScanExec {
    pub fn new(table: &Table, access: Access) -> Self {
        ScanExec {
            tree: BTree::open(table.root),
            bounds: key_bounds(&access),
            cursor: None,
        }
    }

    /// Next `(key, row)` pair; used directly by UPDATE and DELETE.
    pub fn next_entry(&mut self, s: &mut dyn PageStore) -> Result<Option<(Vec<u8>, Row)>> {
        if self.cursor.is_none() {
            let as_ref = |b: &Bound<Vec<u8>>| match b {
                Bound::Included(k) => Bound::Included(k.clone()),
                Bound::Excluded(k) => Bound::Excluded(k.clone()),
                Bound::Unbounded => Bound::Unbounded,
            };
            let (lo, hi) = (as_ref(&self.bounds.0), as_ref(&self.bounds.1));
            self.cursor = Some(self.tree.range(
                s,
                lo.as_ref().map(|k| k.as_slice()),
                hi.as_ref().map(|k| k.as_slice()),
            )?);
        }
        match self.cursor.as_mut().unwrap().next(s)? {
            Some((k, v)) => Ok(Some((k, decode_row(&v)?))),
            None => Ok(None),
        }
    }
}

impl Executor for ScanExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        Ok(self.next_entry(s)?.map(|(_, row)| row))
    }
}

struct FilterExec {
    input: Box<dyn Executor>,
    pred: BExpr,
}

impl Executor for FilterExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        while let Some(row) = self.input.next(s)? {
            if self.pred.matches(&row)? {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
}

struct ProjectExec {
    input: Box<dyn Executor>,
    exprs: Vec<BExpr>,
}

impl Executor for ProjectExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        match self.input.next(s)? {
            Some(row) => Ok(Some(
                self.exprs.iter().map(|e| e.eval(&row)).collect::<Result<_>>()?,
            )),
            None => Ok(None),
        }
    }
}

/// Groups rows by the group-by values (in a BTreeMap, so output comes out
/// sorted by group) and folds each group through the aggregate functions.
struct AggregateExec {
    input: Box<dyn Executor>,
    groups: Vec<BExpr>,
    aggs: Vec<AggCall>,
    output: Option<std::vec::IntoIter<Row>>,
}

impl AggregateExec {
    fn compute(&mut self, s: &mut dyn PageStore) -> Result<Vec<Row>> {
        // Per group: one accumulator per aggregate, plus a set of values
        // already seen for DISTINCT aggregates.
        type GroupState = (Vec<Accumulator>, Vec<BTreeSet<Value>>);
        let fresh = |aggs: &[AggCall]| -> GroupState {
            (
                aggs.iter().map(|a| Accumulator::new(a.func)).collect(),
                aggs.iter().map(|_| BTreeSet::new()).collect(),
            )
        };
        let mut groups: BTreeMap<Vec<Value>, GroupState> = BTreeMap::new();
        if self.groups.is_empty() {
            // An aggregate without GROUP BY yields one row even for no input.
            groups.insert(vec![], fresh(&self.aggs));
        }
        while let Some(row) = self.input.next(s)? {
            let key = self.groups.iter().map(|g| g.eval(&row)).collect::<Result<Vec<_>>>()?;
            let (accs, seen) = groups.entry(key).or_insert_with(|| fresh(&self.aggs));
            for (i, agg) in self.aggs.iter().enumerate() {
                let v = match &agg.arg {
                    None => Value::Boolean(true), // COUNT(*): count every row
                    Some(e) => e.eval(&row)?,
                };
                if agg.distinct && !v.is_null() && !seen[i].insert(v.clone()) {
                    continue;
                }
                accs[i].add(v)?;
            }
        }
        Ok(groups
            .into_iter()
            .map(|(mut key, (accs, _))| {
                key.extend(accs.into_iter().map(Accumulator::finish));
                key
            })
            .collect())
    }
}

impl Executor for AggregateExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        if self.output.is_none() {
            self.output = Some(self.compute(s)?.into_iter());
        }
        Ok(self.output.as_mut().unwrap().next())
    }
}

struct DistinctExec {
    input: Box<dyn Executor>,
    seen: BTreeSet<Row>,
}

impl Executor for DistinctExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        while let Some(row) = self.input.next(s)? {
            if self.seen.insert(row.clone()) {
                return Ok(Some(row));
            }
        }
        Ok(None)
    }
}

/// In-memory sort (stable, so ties keep input order). An external merge
/// sort would be needed for inputs larger than memory.
struct SortExec {
    input: Box<dyn Executor>,
    keys: Vec<(usize, bool)>,
    output: Option<std::vec::IntoIter<Row>>,
}

impl Executor for SortExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        if self.output.is_none() {
            let mut rows = vec![];
            while let Some(r) = self.input.next(s)? {
                rows.push(r);
            }
            rows.sort_by(|a, b| {
                for &(i, asc) in &self.keys {
                    let o = a[i].total_cmp(&b[i]);
                    if o.is_ne() {
                        return if asc { o } else { o.reverse() };
                    }
                }
                std::cmp::Ordering::Equal
            });
            self.output = Some(rows.into_iter());
        }
        Ok(self.output.as_mut().unwrap().next())
    }
}

struct LimitExec {
    input: Box<dyn Executor>,
    limit: Option<u64>,
    offset: u64,
    produced: u64,
}

impl Executor for LimitExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        while self.offset > 0 {
            if self.input.next(s)?.is_none() {
                return Ok(None);
            }
            self.offset -= 1;
        }
        if self.limit.is_some_and(|l| self.produced >= l) {
            return Ok(None); // stop pulling: the rest of the input is never read
        }
        self.produced += 1;
        self.input.next(s)
    }
}

// --- statements ---------------------------------------------------------------------------

/// Storage plus the cached catalog: everything a statement executes against.
pub struct Engine {
    pub storage: Storage,
    catalog: Option<Catalog>,
}

impl Engine {
    pub fn new(mut storage: Storage) -> Result<Self> {
        Catalog::bootstrap(&mut storage)?;
        Ok(Engine {
            storage,
            catalog: None,
        })
    }

    /// The catalog, loading it from the schema table if needed.
    pub fn catalog(&mut self) -> Result<&mut Catalog> {
        if self.catalog.is_none() {
            self.catalog = Some(Catalog::load(&mut self.storage)?);
        }
        Ok(self.catalog.as_mut().unwrap())
    }

    /// Forget the cached catalog (after a rollback it may be stale).
    pub fn invalidate_catalog(&mut self) {
        self.catalog = None;
    }

    /// Execute one statement. Transaction control is handled by the session
    /// layer (`db.rs`), not here.
    pub fn execute(&mut self, stmt: &Statement) -> Result<QueryResult> {
        match stmt {
            Statement::Select(sel) => {
                let QueryPlan { plan, columns } = plan_select(self.catalog()?, sel)?;
                let mut exec = build(plan);
                let mut rows = vec![];
                while let Some(r) = exec.next(&mut self.storage)? {
                    rows.push(r);
                }
                Ok(QueryResult::Rows { columns, rows })
            }
            Statement::Explain(inner) => self.explain(inner),
            Statement::CreateTable(ct) => self.create_table(ct),
            Statement::DropTable { name, if_exists } => {
                self.with_catalog(|cat, s| cat.drop_table(s, name, *if_exists))?;
                Ok(QueryResult::Done("DROP TABLE".into()))
            }
            Statement::Insert { table, columns, rows } => self.insert(table, columns.as_deref(), rows),
            Statement::Update { table, set, filter } => self.update(table, set, filter.as_ref()),
            Statement::Delete { table, filter } => self.delete(table, filter.as_ref()),
            Statement::Begin | Statement::Commit | Statement::Rollback => {
                Err(Error::Txn("transaction statements are handled by the session".into()))
            }
        }
    }

    fn explain(&mut self, stmt: &Statement) -> Result<QueryResult> {
        let text = match stmt {
            Statement::Select(sel) => plan_select(self.catalog()?, sel)?.plan.to_string(),
            Statement::Update { table, filter, .. } | Statement::Delete { table, filter } => {
                let t = self.catalog()?.get(table)?.clone();
                let verb = if matches!(stmt, Statement::Update { .. }) { "Update" } else { "Delete" };
                let mut plan = Plan::Scan {
                    access: choose_access(&t, filter.as_ref()),
                    table: t.clone(),
                };
                if let Some(f) = filter {
                    plan = Plan::Filter {
                        pred: Binder::new(Some(&t), "WHERE").bind(f)?,
                        input: Box::new(plan),
                        desc: f.to_string(),
                    };
                }
                format!("{verb} {}\n{}", t.name, indent(&plan.to_string()))
            }
            Statement::Insert { table, rows, .. } => format!("Insert {table} ({} rows)\n", rows.len()),
            other => return Err(query_err!("cannot EXPLAIN {other:?}")),
        };
        Ok(QueryResult::Rows {
            columns: vec!["plan".into()],
            rows: text.lines().map(|l| vec![Value::Text(l.to_string())]).collect(),
        })
    }

    fn create_table(&mut self, ct: &CreateTable) -> Result<QueryResult> {
        self.with_catalog(|cat, s| cat.create_table(s, ct))?;
        Ok(QueryResult::Done("CREATE TABLE".into()))
    }

    /// Run a catalog-modifying operation with both the catalog and storage.
    fn with_catalog<T>(&mut self, f: impl FnOnce(&mut Catalog, &mut Storage) -> Result<T>) -> Result<T> {
        self.catalog()?;
        let Engine { storage, catalog } = self;
        f(catalog.as_mut().expect("loaded above"), storage)
    }

    fn writable_table(&mut self, name: &str) -> Result<Table> {
        if name == SCHEMA_TABLE {
            return Err(query_err!("{SCHEMA_TABLE} may not be modified directly"));
        }
        Ok(self.catalog()?.get(name)?.clone())
    }

    fn insert(
        &mut self,
        name: &str,
        columns: Option<&[String]>,
        rows: &[Vec<Expr>],
    ) -> Result<QueryResult> {
        let table = self.writable_table(name)?;
        // Map each provided value to its column position.
        let targets: Vec<usize> = match columns {
            None => (0..table.columns.len()).collect(),
            Some(cols) => {
                let mut v = vec![];
                for c in cols {
                    let i = table
                        .column_index(c)
                        .ok_or_else(|| query_err!("no such column: {c} in table {}", table.name))?;
                    if v.contains(&i) {
                        return Err(query_err!("column {c} specified twice"));
                    }
                    v.push(i);
                }
                v
            }
        };
        let tree = BTree::open(table.root);
        let mut next_rowid: Option<i64> = None;
        for exprs in rows {
            if exprs.len() != targets.len() {
                return Err(query_err!(
                    "expected {} values, got {}",
                    targets.len(),
                    exprs.len()
                ));
            }
            let mut row = vec![Value::Null; table.columns.len()];
            for (e, &i) in exprs.iter().zip(&targets) {
                row[i] = eval_const(e, "VALUES")?;
            }
            let row = check_row(&table, row)?;
            let key = match table.pk {
                Some(pk) => {
                    let key = encode_key(&row[pk]);
                    if tree.get(&mut self.storage, &key)?.is_some() {
                        return Err(Error::Constraint(format!(
                            "duplicate primary key {} in table {}",
                            row[pk].to_sql(),
                            table.name
                        )));
                    }
                    key
                }
                None => {
                    // Hidden rowid: one more than the largest existing key.
                    let id = match next_rowid {
                        Some(id) => id,
                        None => match tree.last(&mut self.storage)? {
                            Some((k, _)) => decode_int_key(&k)?
                                .checked_add(1)
                                .ok_or_else(|| Error::Limit("rowid space exhausted".into()))?,
                            None => 1,
                        },
                    };
                    next_rowid = Some(id + 1);
                    encode_key(&Value::Integer(id))
                }
            };
            tree.insert(&mut self.storage, &key, &encode_row(&row))?;
        }
        Ok(QueryResult::Modified {
            verb: "INSERT",
            count: rows.len() as u64,
        })
    }

    /// Collect `(key, row)` for every row matching `filter`. DML collects
    /// first and modifies second, so it never changes a tree it is scanning
    /// (which would also risk the "Halloween problem": an UPDATE seeing rows
    /// it already moved).
    fn matching(&mut self, table: &Table, filter: Option<&Expr>) -> Result<Vec<(Vec<u8>, Row)>> {
        let pred = filter
            .map(|f| Binder::new(Some(table), "WHERE").bind(f))
            .transpose()?;
        let mut scan = ScanExec::new(table, choose_access(table, filter));
        let mut out = vec![];
        while let Some((k, row)) = scan.next_entry(&mut self.storage)? {
            if pred.as_ref().map_or(Ok(true), |p| p.matches(&row))? {
                out.push((k, row));
            }
        }
        Ok(out)
    }

    fn update(&mut self, name: &str, set: &[(String, Expr)], filter: Option<&Expr>) -> Result<QueryResult> {
        let table = self.writable_table(name)?;
        let binder = Binder::new(Some(&table), "SET");
        let mut assignments = vec![];
        for (col, e) in set {
            let i = table
                .column_index(col)
                .ok_or_else(|| query_err!("no such column: {col} in table {}", table.name))?;
            assignments.push((i, binder.bind(e)?));
        }
        let matches = self.matching(&table, filter)?;
        // Compute every new row from its *old* values first.
        let mut updates = Vec::with_capacity(matches.len());
        for (key, old) in matches {
            let mut new = old.clone();
            for (i, e) in &assignments {
                new[*i] = e.eval(&old)?;
            }
            let new = check_row(&table, new)?;
            let new_key = match table.pk {
                Some(pk) => encode_key(&new[pk]),
                None => key.clone(),
            };
            updates.push((key, new_key, new));
        }
        let tree = BTree::open(table.root);
        // Rows whose key changes are deleted first and re-inserted after, so
        // `SET id = id + 1` doesn't collide with rows not yet updated.
        for (key, new_key, _) in &updates {
            if key != new_key {
                tree.delete(&mut self.storage, key)?;
            }
        }
        for (key, new_key, row) in &updates {
            if key != new_key && tree.get(&mut self.storage, new_key)?.is_some() {
                let pk = table.pk.unwrap();
                return Err(Error::Constraint(format!(
                    "duplicate primary key {} in table {}",
                    row[pk].to_sql(),
                    table.name
                )));
            }
            tree.insert(&mut self.storage, new_key, &encode_row(row))?;
        }
        Ok(QueryResult::Modified {
            verb: "UPDATE",
            count: updates.len() as u64,
        })
    }

    fn delete(&mut self, name: &str, filter: Option<&Expr>) -> Result<QueryResult> {
        let table = self.writable_table(name)?;
        let matches = self.matching(&table, filter)?;
        let tree = BTree::open(table.root);
        for (key, _) in &matches {
            tree.delete(&mut self.storage, key)?;
        }
        Ok(QueryResult::Modified {
            verb: "DELETE",
            count: matches.len() as u64,
        })
    }
}

/// Coerce values to their column types and enforce NOT NULL.
fn check_row(table: &Table, row: Row) -> Result<Row> {
    row.into_iter()
        .zip(&table.columns)
        .map(|(v, c)| {
            if v.is_null() && c.not_null {
                return Err(Error::Constraint(format!(
                    "NULL value in NOT NULL column {}.{}",
                    table.name, c.name
                )));
            }
            v.coerce_to(c.ty)
                .map_err(|e| Error::Query(format!("column {}: {}", c.name, e.to_string().trim_start_matches("query error: "))))
        })
        .collect()
}

fn indent(s: &str) -> String {
    s.lines().map(|l| format!("  {l}\n")).collect()
}
