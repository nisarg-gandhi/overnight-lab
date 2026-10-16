//! The catalog: which tables and indexes exist and what they look like.
//!
//! The catalog is *self-hosted*: it is stored in an ordinary table called
//! `toydb_schema`, using the same B+tree and row encoding as user tables.
//! Its root page is recorded in the database header. Each row holds the
//! canonical `CREATE TABLE` / `CREATE INDEX` text of one object, which is
//! parsed again when the catalog is loaded — so the schema format is just
//! SQL.
//!
//! ```text
//! header page: catalog_root = 1
//!                     |
//!                     v
//! toydb_schema (B+tree keyed by name)
//! +-------+------------+----------+----------+-------------------------------------+
//! | type  | name       | tbl_name | rootpage | sql                                 |
//! +-------+------------+----------+----------+-------------------------------------+
//! | index | by_email   | users    | 9        | CREATE UNIQUE INDEX by_email ON ... |
//! | table | orders     | orders   | 7        | CREATE TABLE orders (...)           |
//! | table | users      | users    | 2        | CREATE TABLE users (...)            |
//! +-------+------------+----------+----------+-------------------------------------+
//! ```
//!
//! You can query it like any table: `SELECT name, sql FROM toydb_schema`.
//!
//! ## Secondary indexes
//!
//! An index is another B+tree. For a row with primary-key bytes `pk`, an
//! index on columns `(a, b)` stores the entry
//!
//! ```text
//!   key   = encode(a) ++ encode(b) ++ pk      value = pk
//! ```
//!
//! Appending `pk` makes every key unique even when many rows share the same
//! `(a, b)`, and because the key encoding is prefix-free, all entries for a
//! given value form one contiguous key range: `[encode(v), successor)`. A
//! lookup scans that range and fetches each row from the table by `pk`.

use std::collections::BTreeMap;
use std::ops::Bound;

use crate::btree::BTree;
use crate::error::{corrupt, query_err, Error, Result};
use crate::sql::ast::{CreateIndex, CreateTable, Statement};
use crate::sql::parser::parse;
use crate::storage::{PageId, PageStore, Storage};
use crate::types::{decode_row, encode_key, encode_key_into, encode_row, DataType, Row, Value};

/// Name of the built-in schema table.
pub const SCHEMA_TABLE: &str = "toydb_schema";

/// Prefix for indexes created automatically for `UNIQUE` columns.
const AUTOINDEX_PREFIX: &str = "toydb_autoindex_";

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: DataType,
    pub not_null: bool,
}

/// A secondary index on one or more columns of a table.
#[derive(Clone, Debug, PartialEq)]
pub struct Index {
    pub name: String,
    pub table: String,
    /// Indexed column positions in the table, in index-key order.
    pub columns: Vec<usize>,
    pub unique: bool,
    pub root: PageId,
}

/// Everything the planner and executor need to know about a table.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    /// Index of the PRIMARY KEY column. Tables without one get a hidden,
    /// auto-incrementing integer row id as their B+tree key.
    pub pk: Option<usize>,
    /// Root page of the table's B+tree.
    pub root: PageId,
    /// Secondary indexes, in name order.
    pub indexes: Vec<Index>,
}

impl Table {
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c.name == name)
    }

    fn from_create(ct: &CreateTable, root: PageId) -> Table {
        Table {
            name: ct.name.clone(),
            columns: ct
                .columns
                .iter()
                .map(|c| Column {
                    name: c.name.clone(),
                    ty: c.ty,
                    not_null: c.not_null,
                })
                .collect(),
            pk: ct.columns.iter().position(|c| c.primary_key),
            root,
            indexes: vec![],
        }
    }

    /// The schema table's own definition.
    fn schema_table(root: PageId) -> Table {
        let col = |name: &str, ty| Column {
            name: name.into(),
            ty,
            not_null: true,
        };
        Table {
            name: SCHEMA_TABLE.into(),
            columns: vec![
                col("type", DataType::Text),
                col("name", DataType::Text),
                col("tbl_name", DataType::Text),
                col("rootpage", DataType::Integer),
                col("sql", DataType::Text),
            ],
            pk: Some(1),
            root,
            indexes: vec![],
        }
    }

    /// Add every index entry for a newly stored row.
    pub fn index_insert(&self, s: &mut dyn PageStore, row: &Row, pk: &[u8]) -> Result<()> {
        for ix in &self.indexes {
            ix.insert(s, self, row, pk)?;
        }
        Ok(())
    }

    /// Remove every index entry of a row that is going away.
    pub fn index_delete(&self, s: &mut dyn PageStore, row: &Row, pk: &[u8]) -> Result<()> {
        for ix in &self.indexes {
            BTree::open(ix.root).delete(s, &ix.entry_key(row, pk))?;
        }
        Ok(())
    }
}

/// The smallest byte string greater than every string starting with
/// `prefix`, or `None` if there is none (all 0xFF bytes).
pub fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut v = prefix.to_vec();
    while let Some(last) = v.pop() {
        if last != 0xFF {
            v.push(last + 1);
            return Some(v);
        }
    }
    None
}

impl Index {
    /// `encode(col1) ++ encode(col2) ++ ...` for `row`.
    pub fn value_prefix(&self, row: &Row) -> Vec<u8> {
        let mut k = Vec::new();
        for &c in &self.columns {
            encode_key_into(&row[c], &mut k);
        }
        k
    }

    /// The full index key of a row: indexed values followed by its pk bytes.
    pub fn entry_key(&self, row: &Row, pk: &[u8]) -> Vec<u8> {
        let mut k = self.value_prefix(row);
        k.extend_from_slice(pk);
        k
    }

    /// Insert `row`'s entry, enforcing uniqueness. As in standard SQL, rows
    /// with a NULL in an indexed column never conflict.
    pub fn insert(&self, s: &mut dyn PageStore, table: &Table, row: &Row, pk: &[u8]) -> Result<()> {
        let tree = BTree::open(self.root);
        if self.unique && self.columns.iter().all(|&c| !row[c].is_null()) {
            let prefix = self.value_prefix(row);
            let end = prefix_successor(&prefix);
            let hi = end.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
            let mut cur = tree.range(s, Bound::Included(&prefix), hi)?;
            if cur.next(s)?.is_some() {
                let vals: Vec<String> = self.columns.iter().map(|&c| row[c].to_sql()).collect();
                let cols: Vec<&str> = self.columns.iter().map(|&c| table.columns[c].name.as_str()).collect();
                return Err(Error::Constraint(format!(
                    "duplicate value ({}) for UNIQUE ({}) in table {}",
                    vals.join(", "),
                    cols.join(", "),
                    table.name
                )));
            }
        }
        tree.insert(s, &self.entry_key(row, pk), pk)?;
        Ok(())
    }

    fn from_create(ci: &CreateIndex, table: &Table, root: PageId) -> Result<Index> {
        let mut columns = vec![];
        for c in &ci.columns {
            let i = table
                .column_index(c)
                .ok_or_else(|| query_err!("no such column: {c} in table {}", table.name))?;
            if columns.contains(&i) {
                return Err(query_err!("column {c} appears twice in index {}", ci.name));
            }
            columns.push(i);
        }
        Ok(Index {
            name: ci.name.clone(),
            table: table.name.clone(),
            columns,
            unique: ci.unique,
            root,
        })
    }
}

/// In-memory copy of the catalog, loaded from `toydb_schema`.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    tables: BTreeMap<String, Table>,
}

fn schema_row(kind: &str, name: &str, table: &str, root: PageId, sql: String) -> Row {
    vec![
        Value::Text(kind.into()),
        Value::Text(name.into()),
        Value::Text(table.into()),
        Value::Integer(root as i64),
        Value::Text(sql),
    ]
}

impl Catalog {
    /// Create the schema table if this is a brand-new database.
    pub fn bootstrap(s: &mut Storage) -> Result<()> {
        if s.catalog_root()? == 0 {
            let tree = BTree::create(s)?;
            s.set_catalog_root(tree.root())?;
            s.commit()?;
        }
        Ok(())
    }

    /// Read every table and index definition from the schema table.
    pub fn load(s: &mut Storage) -> Result<Catalog> {
        let root = s.catalog_root()?;
        let mut tables = BTreeMap::new();
        tables.insert(SCHEMA_TABLE.to_string(), Table::schema_table(root));
        let mut indexes = vec![];
        let mut cur = BTree::open(root).scan(s)?;
        while let Some((_, v)) = cur.next(s)? {
            let row = decode_row(&v)?;
            let (Some(Value::Integer(rootpage)), Some(Value::Text(sql))) = (row.get(3), row.get(4))
            else {
                return Err(corrupt!("malformed toydb_schema row"));
            };
            match parse(sql)? {
                Statement::CreateTable(ct) => {
                    tables.insert(ct.name.clone(), Table::from_create(&ct, *rootpage as PageId));
                }
                Statement::CreateIndex(ci) => indexes.push((ci, *rootpage as PageId)),
                _ => return Err(corrupt!("unexpected statement in toydb_schema: {sql}")),
            }
        }
        // Attach indexes once all tables are known.
        for (ci, root) in indexes {
            let table = tables
                .get_mut(&ci.table)
                .ok_or_else(|| corrupt!("index {} on missing table {}", ci.name, ci.table))?;
            let ix = Index::from_create(&ci, table, root)?;
            table.indexes.push(ix);
        }
        Ok(Catalog { tables })
    }

    pub fn get(&self, name: &str) -> Result<&Table> {
        self.tables
            .get(name)
            .ok_or_else(|| query_err!("no such table: {name}"))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.tables.contains_key(name)
    }

    /// The table owning index `name`, if any.
    fn index_owner(&self, name: &str) -> Option<&Table> {
        self.tables
            .values()
            .find(|t| t.indexes.iter().any(|i| i.name == name))
    }

    /// User tables, in name order.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values().filter(|t| t.name != SCHEMA_TABLE)
    }

    fn insert_schema_row(s: &mut Storage, row: &Row) -> Result<()> {
        BTree::open(s.catalog_root()?).insert(s, &encode_key(&row[1]), &encode_row(row))?;
        Ok(())
    }

    /// Create a table: validate, allocate its B+tree, record it in the
    /// schema table, and create indexes for UNIQUE columns. Returns false if
    /// it existed and `IF NOT EXISTS` was given.
    pub fn create_table(&mut self, s: &mut Storage, ct: &CreateTable) -> Result<bool> {
        if self.exists(&ct.name) {
            if ct.if_not_exists {
                return Ok(false);
            }
            return Err(query_err!("table {} already exists", ct.name));
        }
        if self.index_owner(&ct.name).is_some() {
            return Err(query_err!("there is already an index named {}", ct.name));
        }
        if ct.name.starts_with("toydb_") {
            return Err(query_err!("table names starting with toydb_ are reserved"));
        }
        if ct.columns.is_empty() {
            return Err(query_err!("a table needs at least one column"));
        }
        for (i, c) in ct.columns.iter().enumerate() {
            if ct.columns[..i].iter().any(|d| d.name == c.name) {
                return Err(query_err!("duplicate column name {}", c.name));
            }
        }
        if ct.columns.iter().filter(|c| c.primary_key).count() > 1 {
            return Err(Error::Query(
                "only one PRIMARY KEY column is supported (composite keys are not)".into(),
            ));
        }
        let tree = BTree::create(s)?;
        Self::insert_schema_row(s, &schema_row("table", &ct.name, &ct.name, tree.root(), ct.to_string()))?;
        self.tables
            .insert(ct.name.clone(), Table::from_create(ct, tree.root()));
        // UNIQUE column constraints are enforced by automatic indexes.
        for (n, c) in ct.columns.iter().filter(|c| c.unique && !c.primary_key).enumerate() {
            let ci = CreateIndex {
                name: format!("{AUTOINDEX_PREFIX}{}_{}", ct.name, n + 1),
                table: ct.name.clone(),
                columns: vec![c.name.clone()],
                unique: true,
                if_not_exists: false,
            };
            self.create_index_unchecked(s, &ci)?;
        }
        Ok(true)
    }

    /// `CREATE INDEX`: build the index from the table's existing rows.
    pub fn create_index(&mut self, s: &mut Storage, ci: &CreateIndex) -> Result<bool> {
        if self.index_owner(&ci.name).is_some() || self.exists(&ci.name) {
            if ci.if_not_exists && self.index_owner(&ci.name).is_some() {
                return Ok(false);
            }
            return Err(query_err!("an index or table named {} already exists", ci.name));
        }
        if ci.name.starts_with("toydb_") {
            return Err(query_err!("index names starting with toydb_ are reserved"));
        }
        if ci.table == SCHEMA_TABLE {
            return Err(query_err!("cannot index {SCHEMA_TABLE}"));
        }
        self.create_index_unchecked(s, ci)?;
        Ok(true)
    }

    fn create_index_unchecked(&mut self, s: &mut Storage, ci: &CreateIndex) -> Result<()> {
        let table = self.get(&ci.table)?.clone();
        let tree = BTree::create(s)?;
        let index = Index::from_create(ci, &table, tree.root())?;
        // Populate from existing rows (collect first: never modify a tree
        // under an open cursor — here the trees differ, but stay consistent).
        let mut rows = vec![];
        let mut cur = BTree::open(table.root).scan(s)?;
        while let Some(entry) = cur.next_with(s, |k, v| Ok((k.to_vec(), decode_row(v)?)))? {
            rows.push(entry);
        }
        for (pk, row) in &rows {
            index.insert(s, &table, row, pk)?;
        }
        Self::insert_schema_row(s, &schema_row("index", &ci.name, &ci.table, tree.root(), ci.to_string()))?;
        let t = self.tables.get_mut(&ci.table).expect("checked above");
        t.indexes.push(index);
        t.indexes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(())
    }

    /// `DROP INDEX`.
    pub fn drop_index(&mut self, s: &mut Storage, name: &str, if_exists: bool) -> Result<bool> {
        let Some(owner) = self.index_owner(name).map(|t| t.name.clone()) else {
            if if_exists {
                return Ok(false);
            }
            return Err(query_err!("no such index: {name}"));
        };
        if name.starts_with(AUTOINDEX_PREFIX) {
            return Err(query_err!("index {name} enforces a UNIQUE constraint and cannot be dropped"));
        }
        let t = self.tables.get_mut(&owner).unwrap();
        let pos = t.indexes.iter().position(|i| i.name == name).unwrap();
        let ix = t.indexes.remove(pos);
        BTree::open(ix.root).destroy(s)?;
        BTree::open(s.catalog_root()?).delete(s, &encode_key(&Value::Text(name.into())))?;
        Ok(true)
    }

    /// Drop a table (and its indexes) and free all their pages. Returns
    /// false if it didn't exist and `IF EXISTS` was given.
    pub fn drop_table(&mut self, s: &mut Storage, name: &str, if_exists: bool) -> Result<bool> {
        if name == SCHEMA_TABLE {
            return Err(query_err!("cannot drop {SCHEMA_TABLE}"));
        }
        let Some(table) = self.tables.remove(name) else {
            if if_exists {
                return Ok(false);
            }
            return Err(query_err!("no such table: {name}"));
        };
        let schema = BTree::open(s.catalog_root()?);
        for ix in table.indexes {
            BTree::open(ix.root).destroy(s)?;
            schema.delete(s, &encode_key(&Value::Text(ix.name)))?;
        }
        BTree::open(table.root).destroy(s)?;
        schema.delete(s, &encode_key(&Value::Text(name.into())))?;
        Ok(true)
    }

    /// Verify every table and index B+tree, and that each index holds
    /// exactly one correct entry per row. Returns a list of problems (empty
    /// if the database is consistent).
    pub fn check_integrity(&self, s: &mut Storage) -> Result<Vec<String>> {
        let mut problems = vec![];
        for t in self.tables.values() {
            if let Err(e) = BTree::open(t.root).check(s) {
                problems.push(format!("table {}: {e}", t.name));
                continue;
            }
            let mut rows = vec![];
            let mut cur = BTree::open(t.root).scan(s)?;
            while let Some(e) = cur.next_with(s, |k, v| Ok((k.to_vec(), decode_row(v)?)))? {
                rows.push(e);
            }
            for (pk, row) in &rows {
                if row.len() != t.columns.len() {
                    problems.push(format!("table {}: row with {} values", t.name, row.len()));
                }
                if let Some(p) = t.pk {
                    if encode_key(&row[p]) != *pk {
                        problems.push(format!("table {}: key does not match primary key column", t.name));
                    }
                }
            }
            for ix in &t.indexes {
                let tree = BTree::open(ix.root);
                match tree.check(s) {
                    Err(e) => problems.push(format!("index {}: {e}", ix.name)),
                    Ok(st) if st.entries != rows.len() => problems.push(format!(
                        "index {}: {} entries for {} rows",
                        ix.name,
                        st.entries,
                        rows.len()
                    )),
                    Ok(_) => {}
                }
                for (pk, row) in &rows {
                    if tree.get(s, &ix.entry_key(row, pk))?.as_deref() != Some(pk.as_slice()) {
                        problems.push(format!("index {}: missing entry for a row", ix.name));
                        break;
                    }
                }
            }
        }
        Ok(problems)
    }
}
