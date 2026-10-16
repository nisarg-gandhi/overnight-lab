//! The catalog: which tables exist and what they look like.
//!
//! The catalog is *self-hosted*: it is stored in an ordinary table called
//! `toydb_schema`, using the same B+tree and row encoding as user tables.
//! Its root page is recorded in the database header. Each row holds the
//! canonical `CREATE TABLE` text of one table, which is parsed again when
//! the catalog is loaded — so the schema format is just SQL.
//!
//! ```text
//! header page: catalog_root = 1
//!                     |
//!                     v
//! toydb_schema (B+tree keyed by name)
//! +-------+---------+----------+----------+------------------------------+
//! | type  | name    | tbl_name | rootpage | sql                          |
//! +-------+---------+----------+----------+------------------------------+
//! | table | orders  | orders   | 7        | CREATE TABLE orders (...)    |
//! | table | users   | users    | 2        | CREATE TABLE users (...)     |
//! +-------+---------+----------+----------+------------------------------+
//! ```
//!
//! You can query it like any table: `SELECT name, sql FROM toydb_schema`.

use std::collections::BTreeMap;

use crate::btree::BTree;
use crate::error::{corrupt, query_err, Error, Result};
use crate::sql::ast::{CreateTable, Statement};
use crate::sql::parser::parse;
use crate::storage::{PageId, Storage};
use crate::types::{decode_row, encode_key, encode_row, DataType, Value};

/// Name of the built-in schema table.
pub const SCHEMA_TABLE: &str = "toydb_schema";

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    pub ty: DataType,
    pub not_null: bool,
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
        }
    }
}

/// In-memory copy of the catalog, loaded from `toydb_schema`.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    tables: BTreeMap<String, Table>,
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

    /// Read every table definition from the schema table.
    pub fn load(s: &mut Storage) -> Result<Catalog> {
        let root = s.catalog_root()?;
        let mut tables = BTreeMap::new();
        tables.insert(SCHEMA_TABLE.to_string(), Table::schema_table(root));
        let mut cur = BTree::open(root).scan(s)?;
        while let Some((_, v)) = cur.next(s)? {
            let row = decode_row(&v)?;
            let (Some(Value::Integer(rootpage)), Some(Value::Text(sql))) = (row.get(3), row.get(4))
            else {
                return Err(corrupt!("malformed toydb_schema row"));
            };
            let Statement::CreateTable(ct) = parse(sql)? else {
                return Err(corrupt!("toydb_schema contains non-CREATE TABLE sql"));
            };
            tables.insert(ct.name.clone(), Table::from_create(&ct, *rootpage as PageId));
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

    /// User tables, in name order.
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values().filter(|t| t.name != SCHEMA_TABLE)
    }

    /// Create a table: validate, allocate its B+tree, record it in the
    /// schema table. Returns false if it existed and `IF NOT EXISTS` was given.
    pub fn create_table(&mut self, s: &mut Storage, ct: &CreateTable) -> Result<bool> {
        if self.exists(&ct.name) {
            if ct.if_not_exists {
                return Ok(false);
            }
            return Err(query_err!("table {} already exists", ct.name));
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
        let sql = ct.to_string();
        let row = vec![
            Value::Text("table".into()),
            Value::Text(ct.name.clone()),
            Value::Text(ct.name.clone()),
            Value::Integer(tree.root() as i64),
            Value::Text(sql),
        ];
        BTree::open(s.catalog_root()?).insert(s, &encode_key(&row[1]), &encode_row(&row))?;
        self.tables
            .insert(ct.name.clone(), Table::from_create(ct, tree.root()));
        Ok(true)
    }

    /// Drop a table and free all its pages. Returns false if it didn't exist
    /// and `IF EXISTS` was given.
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
        BTree::open(table.root).destroy(s)?;
        BTree::open(s.catalog_root()?).delete(s, &encode_key(&Value::Text(name.into())))?;
        Ok(true)
    }
}
