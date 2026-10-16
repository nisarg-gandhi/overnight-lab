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
    choose_access, eval_const, plan_select, Access, AggCall, Binder, JoinLookup, Plan, QueryPlan,
};
use crate::btree::{BTree, Cursor};
use crate::catalog::{prefix_successor, Catalog, Table, SCHEMA_TABLE};
use crate::error::{corrupt, query_err, Error, Result};
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
        Plan::Join { left, right, lookup, on, left_join, .. } => Box::new(JoinExec {
            left: build(*left),
            right,
            lookup,
            on,
            left_join,
            materialized: None,
            current: None,
            candidates: vec![],
            pos: 0,
            matched: false,
        }),
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

/// Encode an access path into key bounds on the B+tree it scans (the table
/// for primary-key access, the index for index access).
fn key_bounds(access: &Access) -> (Bound<Vec<u8>>, Bound<Vec<u8>>) {
    let enc = |b: &Bound<Value>| match b {
        Bound::Included(v) => Bound::Included(encode_key(v)),
        Bound::Excluded(v) => Bound::Excluded(encode_key(v)),
        Bound::Unbounded => Bound::Unbounded,
    };
    // Index keys are `encode(value) ++ pk`, so "all entries for value v" is
    // the prefix range [encode(v), successor(encode(v))).
    let after = |v: &Value| match prefix_successor(&encode_key(v)) {
        Some(k) => Bound::Excluded(k),
        None => Bound::Unbounded,
    };
    match access {
        Access::Full => (Bound::Unbounded, Bound::Unbounded),
        Access::Key(v) => (Bound::Included(encode_key(v)), Bound::Included(encode_key(v))),
        Access::Range { lo, hi } => (enc(lo), enc(hi)),
        Access::Index { eq: Some(v), .. } => (Bound::Included(encode_key(v)), after(v)),
        Access::Index { eq: None, lo, hi, .. } => {
            let lo = match lo {
                Bound::Included(v) => Bound::Included(encode_key(v)),
                Bound::Excluded(v) => match after(v) {
                    Bound::Excluded(k) => Bound::Included(k),
                    _ => Bound::Unbounded, // can't happen; the filter re-checks anyway
                },
                Bound::Unbounded => Bound::Unbounded,
            };
            let hi = match hi {
                Bound::Included(v) => after(v),
                Bound::Excluded(v) => Bound::Excluded(encode_key(v)),
                Bound::Unbounded => Bound::Unbounded,
            };
            (lo, hi)
        }
    }
}

/// Reads rows from a table, either directly from its B+tree (optionally
/// restricted to a primary-key range) or through a secondary index.
pub struct ScanExec {
    /// The tree the cursor walks: the table, or the index.
    tree: BTree,
    /// For index scans: the table to fetch rows from by primary key.
    table: Option<BTree>,
    bounds: (Bound<Vec<u8>>, Bound<Vec<u8>>),
    cursor: Option<Cursor>,
}

impl ScanExec {
    pub fn new(table: &Table, access: Access) -> Self {
        let bounds = key_bounds(&access);
        match access {
            Access::Index { index, .. } => ScanExec {
                tree: BTree::open(index.root),
                table: Some(BTree::open(table.root)),
                bounds,
                cursor: None,
            },
            _ => ScanExec {
                tree: BTree::open(table.root),
                table: None,
                bounds,
                cursor: None,
            },
        }
    }

    /// Position the cursor on first use.
    fn open(&mut self, s: &mut dyn PageStore) -> Result<()> {
        if self.cursor.is_none() {
            let (lo, hi) = &self.bounds;
            self.cursor = Some(self.tree.range(
                s,
                lo.as_ref().map(|k| k.as_slice()),
                hi.as_ref().map(|k| k.as_slice()),
            )?);
        }
        Ok(())
    }

    /// Next `(primary key, row)` pair; used directly by UPDATE and DELETE.
    pub fn next_entry(&mut self, s: &mut dyn PageStore) -> Result<Option<(Vec<u8>, Row)>> {
        self.open(s)?;
        let cursor = self.cursor.as_mut().unwrap();
        match self.table {
            None => cursor.next_with(s, |k, v| Ok((k.to_vec(), decode_row(v)?))),
            Some(table) => {
                // The index entry's value is the row's primary key.
                let Some(pk) = cursor.next_with(s, |_, v| Ok(v.to_vec()))? else {
                    return Ok(None);
                };
                let row = table
                    .get(s, &pk)?
                    .ok_or_else(|| corrupt!("index entry points at a missing row"))?;
                Ok(Some((pk, decode_row(&row)?)))
            }
        }
    }
}

impl Executor for ScanExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        if self.table.is_some() {
            return Ok(self.next_entry(s)?.map(|(_, row)| row));
        }
        self.open(s)?;
        // Decode straight from the cursor's page copy: no key/value copies.
        self.cursor.as_mut().unwrap().next_with(s, |_, v| decode_row(v))
    }
}

/// Nested-loop join. For each left row, the candidate right rows come from
/// a primary-key/index lookup when the planner found one, or else from a
/// copy of the whole right table read once on first use. Every candidate
/// pair is checked against the full ON condition.
struct JoinExec {
    left: Box<dyn Executor>,
    right: Table,
    lookup: Option<JoinLookup>,
    on: Option<BExpr>,
    left_join: bool,
    /// All rows of the right table (only without a lookup).
    materialized: Option<Vec<Row>>,
    /// The left row being joined, and its candidate right rows.
    current: Option<Row>,
    candidates: Vec<Row>,
    pos: usize,
    /// Did the current left row match anything? (for LEFT JOIN padding)
    matched: bool,
}

impl JoinExec {
    /// Right rows that might match `left`.
    fn lookup_rows(&self, s: &mut dyn PageStore, lookup: &JoinLookup, left: &Row) -> Result<Vec<Row>> {
        let col = match &lookup.index {
            None => self.right.pk.expect("pk lookup on a table with a primary key"),
            Some(ix) => ix.columns[0],
        };
        // Key encodings only match within a type, so convert like a stored
        // value would be; NULL or an incomparable type can't match.
        let Ok(key) = lookup.key.eval(left)?.coerce_to(self.right.columns[col].ty) else {
            return Ok(vec![]);
        };
        if key.is_null() {
            return Ok(vec![]);
        }
        let access = match &lookup.index {
            None => Access::Key(key),
            Some(ix) => Access::Index { index: ix.clone(), eq: Some(key), lo: Bound::Unbounded, hi: Bound::Unbounded },
        };
        let mut scan = ScanExec::new(&self.right, access);
        let mut rows = vec![];
        while let Some(r) = scan.next(s)? {
            rows.push(r);
        }
        Ok(rows)
    }
}

impl Executor for JoinExec {
    fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<Row>> {
        loop {
            let Some(left) = &self.current else {
                // Advance to the next left row.
                let Some(left) = self.left.next(s)? else { return Ok(None) };
                if let Some(lookup) = &self.lookup {
                    self.candidates = self.lookup_rows(s, lookup, &left)?;
                } else if self.materialized.is_none() {
                    let mut scan = ScanExec::new(&self.right, Access::Full);
                    let mut rows = vec![];
                    while let Some(r) = scan.next(s)? {
                        rows.push(r);
                    }
                    self.materialized = Some(rows);
                }
                self.current = Some(left);
                self.pos = 0;
                self.matched = false;
                continue;
            };
            let right_rows = match &self.materialized {
                Some(rows) if self.lookup.is_none() => rows,
                _ => &self.candidates,
            };
            if let Some(right) = right_rows.get(self.pos) {
                self.pos += 1;
                let mut row = Vec::with_capacity(left.len() + right.len());
                row.extend_from_slice(left);
                row.extend_from_slice(right);
                if self.on.as_ref().map_or(Ok(true), |on| on.matches(&row))? {
                    self.matched = true;
                    return Ok(Some(row));
                }
                continue;
            }
            // Candidates exhausted.
            let left = self.current.take().unwrap();
            if self.left_join && !self.matched {
                let mut row = left;
                row.extend(std::iter::repeat_n(Value::Null, self.right.columns.len()));
                return Ok(Some(row));
            }
        }
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
        let finish = |mut key: Vec<Value>, (accs, _): GroupState| {
            key.extend(accs.into_iter().map(Accumulator::finish));
            key
        };
        let feed = |aggs: &[AggCall], (accs, seen): &mut GroupState, row: &Row| -> Result<()> {
            for (i, agg) in aggs.iter().enumerate() {
                let v = match &agg.arg {
                    None => Value::Boolean(true), // COUNT(*): count every row
                    Some(e) => e.eval(row)?,
                };
                if agg.distinct && !v.is_null() && !seen[i].insert(v.clone()) {
                    continue;
                }
                accs[i].add(v)?;
            }
            Ok(())
        };
        if self.groups.is_empty() {
            // No GROUP BY: a single group, which exists even for no input
            // (so `SELECT COUNT(*) FROM empty` returns 0).
            let mut state = fresh(&self.aggs);
            while let Some(row) = self.input.next(s)? {
                feed(&self.aggs, &mut state, &row)?;
            }
            return Ok(vec![finish(vec![], state)]);
        }
        let mut groups: BTreeMap<Vec<Value>, GroupState> = BTreeMap::new();
        while let Some(row) = self.input.next(s)? {
            let key = self.groups.iter().map(|g| g.eval(&row)).collect::<Result<Vec<_>>>()?;
            let state = groups.entry(key).or_insert_with(|| fresh(&self.aggs));
            feed(&self.aggs, state, &row)?;
        }
        Ok(groups.into_iter().map(|(k, st)| finish(k, st)).collect())
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
            Statement::CreateIndex(ci) => {
                self.with_catalog(|cat, s| cat.create_index(s, ci))?;
                Ok(QueryResult::Done("CREATE INDEX".into()))
            }
            Statement::DropIndex { name, if_exists } => {
                self.with_catalog(|cat, s| cat.drop_index(s, name, *if_exists))?;
                Ok(QueryResult::Done("DROP INDEX".into()))
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
                    access: choose_access(&t, &t.name, filter.as_ref()),
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

    /// Verify all B+trees and that every index matches its table. Returns
    /// a list of problems; empty means consistent.
    pub fn check_integrity(&mut self) -> Result<Vec<String>> {
        self.with_catalog(|cat, s| cat.check_integrity(s))
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
            table.index_insert(&mut self.storage, &row, &key)?;
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
        let mut scan = ScanExec::new(table, choose_access(table, &table.name, filter));
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
            updates.push((key, new_key, new, old));
        }
        let tree = BTree::open(table.root);
        // Rows whose key changes are deleted first and re-inserted after, so
        // `SET id = id + 1` doesn't collide with rows not yet updated. The
        // same goes for index entries: all old ones go before any new one is
        // added, so swapping two UNIQUE values in one statement works.
        for (key, new_key, _, old) in &updates {
            table.index_delete(&mut self.storage, old, key)?;
            if key != new_key {
                tree.delete(&mut self.storage, key)?;
            }
        }
        for (key, new_key, row, _) in &updates {
            if key != new_key && tree.get(&mut self.storage, new_key)?.is_some() {
                let pk = table.pk.unwrap();
                return Err(Error::Constraint(format!(
                    "duplicate primary key {} in table {}",
                    row[pk].to_sql(),
                    table.name
                )));
            }
            tree.insert(&mut self.storage, new_key, &encode_row(row))?;
            table.index_insert(&mut self.storage, row, new_key)?;
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
        for (key, row) in &matches {
            table.index_delete(&mut self.storage, row, key)?;
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
