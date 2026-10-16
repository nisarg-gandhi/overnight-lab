//! The abstract syntax tree produced by the parser.
//!
//! `Display` implementations turn AST nodes back into SQL. That is used to
//! name result columns (`SELECT a+1` has a column called `a + 1`) and to
//! store a canonical `CREATE TABLE` statement in the catalog, which is
//! re-parsed when the database is opened (the same trick SQLite uses for
//! its `sqlite_schema` table).

use std::fmt;

use crate::types::{DataType, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    CreateTable(CreateTable),
    DropTable { name: String, if_exists: bool },
    CreateIndex(CreateIndex),
    DropIndex { name: String, if_exists: bool },
    Insert {
        table: String,
        columns: Option<Vec<String>>,
        rows: Vec<Vec<Expr>>,
    },
    Select(Box<Select>),
    Update {
        table: String,
        set: Vec<(String, Expr)>,
        filter: Option<Expr>,
    },
    Delete { table: String, filter: Option<Expr> },
    Begin,
    Commit,
    Rollback,
    Explain(Box<Statement>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateTable {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub if_not_exists: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnDef {
    pub name: String,
    pub ty: DataType,
    pub primary_key: bool,
    pub not_null: bool,
    /// `UNIQUE` column constraint: backed by an automatically created index.
    pub unique: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CreateIndex {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub if_not_exists: bool,
}

/// A table in a FROM clause, optionally renamed: `users AS u`.
#[derive(Clone, Debug, PartialEq)]
pub struct FromTable {
    pub name: String,
    pub alias: Option<String>,
}

impl FromTable {
    /// The name columns are qualified with: the alias if any.
    pub fn reference(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinKind {
    /// `a, b` or `a CROSS JOIN b`: every pair of rows.
    Cross,
    /// `a [INNER] JOIN b ON ...`: pairs satisfying the condition.
    Inner,
    /// `a LEFT [OUTER] JOIN b ON ...`: like INNER, plus left rows with no
    /// match, padded with NULLs.
    Left,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Join {
    pub table: FromTable,
    pub kind: JoinKind,
    pub on: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Select {
    pub distinct: bool,
    pub items: Vec<SelectItem>,
    pub from: Option<FromTable>,
    /// Further tables joined to `from`, left to right.
    pub joins: Vec<Join>,
    pub filter: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub having: Option<Expr>,
    pub order_by: Vec<OrderBy>,
    pub limit: Option<Expr>,
    pub offset: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SelectItem {
    /// `*`
    Wildcard,
    /// `t.*`
    TableWildcard(String),
    /// `expr [AS alias]`
    Expr(Expr, Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct OrderBy {
    pub expr: Expr,
    pub asc: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Plus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Literal(Value),
    /// Possibly table-qualified column reference.
    Column(Option<String>, String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `expr IS [NOT] NULL`
    IsNull(Box<Expr>, bool),
    /// `expr [NOT] IN (list)`
    InList(Box<Expr>, Vec<Expr>, bool),
    /// `expr [NOT] LIKE pattern`
    Like(Box<Expr>, Box<Expr>, bool),
    /// `expr [NOT] BETWEEN lo AND hi`
    Between(Box<Expr>, Box<Expr>, Box<Expr>, bool),
    /// Function call; `name` is lower-case. `COUNT(*)` has `star` set.
    Function {
        name: String,
        args: Vec<Expr>,
        distinct: bool,
        star: bool,
    },
}

impl BinOp {
    fn symbol(self) -> &'static str {
        match self {
            BinOp::Or => "OR",
            BinOp::And => "AND",
            BinOp::Eq => "=",
            BinOp::NotEq => "!=",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Concat => "||",
        }
    }
}

/// Quote an identifier if it wouldn't survive the lexer unquoted.
pub fn quote_ident(s: &str) -> String {
    let plain = s
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && crate::sql::parser::is_plain_identifier(s);
    if plain {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

/// Display an operand, parenthesised if it is itself an operator
/// expression, so that the text re-parses with the same meaning.
struct Operand<'a>(&'a Expr);

impl fmt::Display for Operand<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            e @ (Expr::Literal(_) | Expr::Column(..) | Expr::Function { .. }) => write!(f, "{e}"),
            e => write!(f, "({e})"),
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let not = |neg: &bool| if *neg { "NOT " } else { "" };
        match self {
            Expr::Literal(v) => f.write_str(&v.to_sql()),
            Expr::Column(Some(t), c) => write!(f, "{}.{}", quote_ident(t), quote_ident(c)),
            Expr::Column(None, c) => f.write_str(&quote_ident(c)),
            Expr::Unary(UnOp::Not, e) => write!(f, "NOT {}", Operand(e)),
            Expr::Unary(UnOp::Neg, e) => write!(f, "-{}", Operand(e)),
            Expr::Unary(UnOp::Plus, e) => write!(f, "+{}", Operand(e)),
            Expr::Binary(op, l, r) => write!(f, "{} {} {}", Operand(l), op.symbol(), Operand(r)),
            Expr::IsNull(e, neg) => write!(f, "{} IS {}NULL", Operand(e), not(neg)),
            Expr::InList(e, list, neg) => {
                let items: Vec<String> = list.iter().map(|x| x.to_string()).collect();
                write!(f, "{} {}IN ({})", Operand(e), not(neg), items.join(", "))
            }
            Expr::Like(e, p, neg) => write!(f, "{} {}LIKE {}", Operand(e), not(neg), Operand(p)),
            Expr::Between(e, lo, hi, neg) => write!(
                f,
                "{} {}BETWEEN {} AND {}",
                Operand(e),
                not(neg),
                Operand(lo),
                Operand(hi)
            ),
            Expr::Function { name, star: true, .. } => write!(f, "{}(*)", name.to_uppercase()),
            Expr::Function { name, args, distinct, .. } => {
                let a: Vec<String> = args.iter().map(|x| x.to_string()).collect();
                write!(
                    f,
                    "{}({}{})",
                    name.to_uppercase(),
                    if *distinct { "DISTINCT " } else { "" },
                    a.join(", ")
                )
            }
        }
    }
}

impl fmt::Display for CreateTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CREATE TABLE {} (", quote_ident(&self.name))?;
        for (i, c) in self.columns.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{} {}", quote_ident(&c.name), c.ty)?;
            if c.primary_key {
                f.write_str(" PRIMARY KEY")?;
            } else if c.not_null {
                f.write_str(" NOT NULL")?;
            }
            if c.unique && !c.primary_key {
                f.write_str(" UNIQUE")?;
            }
        }
        f.write_str(")")
    }
}

impl fmt::Display for CreateIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cols: Vec<String> = self.columns.iter().map(|c| quote_ident(c)).collect();
        write!(
            f,
            "CREATE {}INDEX {} ON {} ({})",
            if self.unique { "UNIQUE " } else { "" },
            quote_ident(&self.name),
            quote_ident(&self.table),
            cols.join(", ")
        )
    }
}

impl Expr {
    /// Rebuild the expression bottom-up, replacing every sub-expression for
    /// which `f` returns `Some`.
    pub fn transform(&self, f: &impl Fn(&Expr) -> Option<Expr>) -> Expr {
        if let Some(e) = f(self) {
            return e;
        }
        let t = |e: &Expr| Box::new(e.transform(f));
        match self {
            Expr::Literal(_) | Expr::Column(..) => self.clone(),
            Expr::Unary(op, e) => Expr::Unary(*op, t(e)),
            Expr::Binary(op, l, r) => Expr::Binary(*op, t(l), t(r)),
            Expr::IsNull(e, n) => Expr::IsNull(t(e), *n),
            Expr::InList(e, list, n) => {
                Expr::InList(t(e), list.iter().map(|x| x.transform(f)).collect(), *n)
            }
            Expr::Like(e, p, n) => Expr::Like(t(e), t(p), *n),
            Expr::Between(e, lo, hi, n) => Expr::Between(t(e), t(lo), t(hi), *n),
            Expr::Function { name, args, distinct, star } => Expr::Function {
                name: name.clone(),
                args: args.iter().map(|x| x.transform(f)).collect(),
                distinct: *distinct,
                star: *star,
            },
        }
    }

    /// Visit this expression and all sub-expressions, pre-order.
    pub fn walk(&self, f: &mut impl FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Literal(_) | Expr::Column(..) => {}
            Expr::Unary(_, e) | Expr::IsNull(e, _) => e.walk(f),
            Expr::Binary(_, l, r) | Expr::Like(l, r, _) => {
                l.walk(f);
                r.walk(f);
            }
            Expr::InList(e, list, _) => {
                e.walk(f);
                list.iter().for_each(|x| x.walk(f));
            }
            Expr::Between(e, lo, hi, _) => {
                e.walk(f);
                lo.walk(f);
                hi.walk(f);
            }
            Expr::Function { args, .. } => args.iter().for_each(|x| x.walk(f)),
        }
    }
}
