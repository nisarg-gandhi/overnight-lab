//! A hand-written recursive-descent parser.
//!
//! Each grammar rule is one method. Expressions use *precedence climbing*:
//! one method per precedence level, each calling the next-tighter level for
//! its operands.
//!
//! ```text
//! statement   := select | insert | update | delete | create | drop
//!              | BEGIN [TRANSACTION] | COMMIT | ROLLBACK | EXPLAIN statement
//! select      := SELECT [DISTINCT] item {, item} [FROM ident]
//!                [WHERE expr] [GROUP BY expr {, expr}] [HAVING expr]
//!                [ORDER BY expr [ASC|DESC] {, ...}] [LIMIT expr [OFFSET expr]]
//! insert      := INSERT INTO ident [( ident {, ident} )]
//!                VALUES ( expr {, expr} ) {, ( ... )}
//! update      := UPDATE ident SET ident = expr {, ident = expr} [WHERE expr]
//! delete      := DELETE FROM ident [WHERE expr]
//! create      := CREATE TABLE [IF NOT EXISTS] ident ( coldef {, coldef} )
//!              | CREATE [UNIQUE] INDEX [IF NOT EXISTS] ident ON ident ( ident {, ident} )
//! coldef      := ident type {PRIMARY KEY | NOT NULL | NULL | UNIQUE}
//! drop        := DROP TABLE [IF EXISTS] ident | DROP INDEX [IF EXISTS] ident
//!
//! expr        := or
//! or          := and {OR and}                                  (loosest)
//! and         := not {AND not}
//! not         := NOT not | predicate
//! predicate   := concat [ comparison-op concat
//!                       | IS [NOT] NULL | [NOT] LIKE concat
//!                       | [NOT] IN ( expr {, expr} )
//!                       | [NOT] BETWEEN concat AND concat ]
//! concat      := additive {|| additive}
//! additive    := term {(+|-) term}
//! term        := unary {(*|/|%) unary}
//! unary       := (-|+) unary | primary
//! primary     := literal | ident [. ident] | ident ( args ) | ( expr ) (tightest)
//! ```

use super::ast::*;
use super::lexer::{describe_pos, tokenize, Keyword, Spanned, Token};
use crate::error::{Error, Result};
use crate::types::{DataType, Value};

/// Parse a single statement (a trailing `;` is allowed).
pub fn parse(sql: &str) -> Result<Statement> {
    let mut stmts = parse_script(sql)?;
    match stmts.len() {
        1 => Ok(stmts.remove(0)),
        0 => Err(Error::Parse("empty statement".into())),
        _ => Err(Error::Parse("expected a single statement".into())),
    }
}

/// Parse a `;`-separated sequence of statements.
pub fn parse_script(sql: &str) -> Result<Vec<Statement>> {
    let mut p = Parser {
        src: sql,
        tokens: tokenize(sql)?,
        pos: 0,
    };
    let mut out = vec![];
    loop {
        while p.eat(&Token::Semicolon) {}
        if p.peek().is_none() {
            return Ok(out);
        }
        out.push(p.statement()?);
        if p.peek().is_some() && !p.eat(&Token::Semicolon) {
            return Err(p.unexpected("';' or end of input"));
        }
    }
}

/// True if `s` lexes as a single unquoted identifier equal to itself, i.e.
/// it's not a keyword and needs no quoting.
pub fn is_plain_identifier(s: &str) -> bool {
    matches!(tokenize(s).as_deref(), Ok([Spanned { token: Token::Ident(i), .. }]) if i == s)
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Spanned>,
    pos: usize,
}

impl Parser<'_> {
    // --- token helpers ------------------------------------------------------

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|t| &t.token)
    }

    fn peek_at(&self, n: usize) -> Option<&Token> {
        self.tokens.get(self.pos + n).map(|t| &t.token)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).map(|t| t.token.clone());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    /// Consume `t` if it is next.
    fn eat(&mut self, t: &Token) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: Keyword) -> bool {
        self.eat(&Token::Keyword(k))
    }

    fn expect(&mut self, t: &Token) -> Result<()> {
        if self.eat(t) {
            Ok(())
        } else {
            Err(self.unexpected(&t.to_string()))
        }
    }

    fn expect_kw(&mut self, k: Keyword) -> Result<()> {
        self.expect(&Token::Keyword(k))
    }

    /// Build a "expected X, found Y at line L, column C" error.
    fn unexpected(&self, expected: &str) -> Error {
        match self.tokens.get(self.pos) {
            Some(t) => Error::Parse(format!(
                "expected {expected}, found {} at {}",
                t.token,
                describe_pos(self.src, t.pos)
            )),
            None => Error::Parse(format!("expected {expected}, found end of input")),
        }
    }

    /// An identifier. Type names and a few other non-reserved keywords are
    /// accepted too, so columns can be called `text` or `key`.
    fn ident(&mut self) -> Result<String> {
        let name = match self.peek() {
            Some(Token::Ident(s)) => s.clone(),
            Some(Token::Keyword(
                k @ (Keyword::Key
                | Keyword::Text
                | Keyword::Integer
                | Keyword::Real
                | Keyword::Boolean
                | Keyword::Transaction),
            )) => format!("{k:?}").to_lowercase(),
            _ => return Err(self.unexpected("an identifier")),
        };
        self.pos += 1;
        Ok(name)
    }

    // --- statements -----------------------------------------------------------

    fn statement(&mut self) -> Result<Statement> {
        let Some(Token::Keyword(k)) = self.peek() else {
            return Err(self.unexpected("a statement"));
        };
        match k {
            Keyword::Select => Ok(Statement::Select(Box::new(self.select()?))),
            Keyword::Insert => self.insert(),
            Keyword::Update => self.update(),
            Keyword::Delete => self.delete(),
            Keyword::Create => self.create(),
            Keyword::Drop => self.drop_table(),
            Keyword::Begin => {
                self.next();
                self.eat_kw(Keyword::Transaction);
                Ok(Statement::Begin)
            }
            Keyword::Commit => {
                self.next();
                self.eat_kw(Keyword::Transaction);
                Ok(Statement::Commit)
            }
            Keyword::Rollback => {
                self.next();
                self.eat_kw(Keyword::Transaction);
                Ok(Statement::Rollback)
            }
            Keyword::Explain => {
                self.next();
                Ok(Statement::Explain(Box::new(self.statement()?)))
            }
            _ => Err(self.unexpected("a statement")),
        }
    }

    fn select(&mut self) -> Result<Select> {
        self.expect_kw(Keyword::Select)?;
        let distinct = self.eat_kw(Keyword::Distinct);
        if !distinct {
            self.eat_kw(Keyword::All);
        }
        let mut items = vec![];
        loop {
            if self.eat(&Token::Star) {
                items.push(SelectItem::Wildcard);
            } else {
                let e = self.expr()?;
                let alias = if self.eat_kw(Keyword::As) {
                    Some(self.ident()?)
                } else if let Some(Token::Ident(_)) = self.peek() {
                    Some(self.ident()?) // `SELECT a b` aliases a as b
                } else {
                    None
                };
                items.push(SelectItem::Expr(e, alias));
            }
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        let from = if self.eat_kw(Keyword::From) {
            Some(self.ident()?)
        } else {
            None
        };
        let filter = if self.eat_kw(Keyword::Where) {
            Some(self.expr()?)
        } else {
            None
        };
        let mut group_by = vec![];
        if self.eat_kw(Keyword::Group) {
            self.expect_kw(Keyword::By)?;
            group_by = self.expr_list()?;
        }
        let having = if self.eat_kw(Keyword::Having) {
            Some(self.expr()?)
        } else {
            None
        };
        let mut order_by = vec![];
        if self.eat_kw(Keyword::Order) {
            self.expect_kw(Keyword::By)?;
            loop {
                let expr = self.expr()?;
                let asc = if self.eat_kw(Keyword::Desc) {
                    false
                } else {
                    self.eat_kw(Keyword::Asc);
                    true
                };
                order_by.push(OrderBy { expr, asc });
                if !self.eat(&Token::Comma) {
                    break;
                }
            }
        }
        let (mut limit, mut offset) = (None, None);
        if self.eat_kw(Keyword::Limit) {
            limit = Some(self.expr()?);
            if self.eat_kw(Keyword::Offset) {
                offset = Some(self.expr()?);
            }
        }
        Ok(Select {
            distinct,
            items,
            from,
            filter,
            group_by,
            having,
            order_by,
            limit,
            offset,
        })
    }

    fn insert(&mut self) -> Result<Statement> {
        self.expect_kw(Keyword::Insert)?;
        self.expect_kw(Keyword::Into)?;
        let table = self.ident()?;
        let columns = if self.eat(&Token::LParen) {
            let mut cols = vec![self.ident()?];
            while self.eat(&Token::Comma) {
                cols.push(self.ident()?);
            }
            self.expect(&Token::RParen)?;
            Some(cols)
        } else {
            None
        };
        self.expect_kw(Keyword::Values)?;
        let mut rows = vec![];
        loop {
            self.expect(&Token::LParen)?;
            rows.push(self.expr_list()?);
            self.expect(&Token::RParen)?;
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(Statement::Insert {
            table,
            columns,
            rows,
        })
    }

    fn update(&mut self) -> Result<Statement> {
        self.expect_kw(Keyword::Update)?;
        let table = self.ident()?;
        self.expect_kw(Keyword::Set)?;
        let mut set = vec![];
        loop {
            let col = self.ident()?;
            self.expect(&Token::Eq)?;
            set.push((col, self.expr()?));
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        let filter = if self.eat_kw(Keyword::Where) {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Statement::Update { table, set, filter })
    }

    fn delete(&mut self) -> Result<Statement> {
        self.expect_kw(Keyword::Delete)?;
        self.expect_kw(Keyword::From)?;
        let table = self.ident()?;
        let filter = if self.eat_kw(Keyword::Where) {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(Statement::Delete { table, filter })
    }

    fn if_not_exists(&mut self) -> Result<bool> {
        if self.eat_kw(Keyword::If) {
            self.expect_kw(Keyword::Not)?;
            self.expect_kw(Keyword::Exists)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn if_exists(&mut self) -> Result<bool> {
        if self.eat_kw(Keyword::If) {
            self.expect_kw(Keyword::Exists)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn create(&mut self) -> Result<Statement> {
        self.expect_kw(Keyword::Create)?;
        let unique = self.eat_kw(Keyword::Unique);
        if unique || self.peek() == Some(&Token::Keyword(Keyword::Index)) {
            self.expect_kw(Keyword::Index)?;
            let if_not_exists = self.if_not_exists()?;
            let name = self.ident()?;
            self.expect_kw(Keyword::On)?;
            let table = self.ident()?;
            self.expect(&Token::LParen)?;
            let mut columns = vec![self.ident()?];
            while self.eat(&Token::Comma) {
                columns.push(self.ident()?);
            }
            self.expect(&Token::RParen)?;
            return Ok(Statement::CreateIndex(CreateIndex {
                name,
                table,
                columns,
                unique,
                if_not_exists,
            }));
        }
        self.expect_kw(Keyword::Table)?;
        let if_not_exists = self.if_not_exists()?;
        let name = self.ident()?;
        self.expect(&Token::LParen)?;
        let mut columns = vec![];
        loop {
            let cname = self.ident()?;
            let ty = match self.next() {
                Some(Token::Keyword(Keyword::Integer)) => DataType::Integer,
                Some(Token::Keyword(Keyword::Real)) => DataType::Real,
                Some(Token::Keyword(Keyword::Text)) => DataType::Text,
                Some(Token::Keyword(Keyword::Boolean)) => DataType::Boolean,
                _ => {
                    self.pos -= 1;
                    return Err(self.unexpected("a column type (INTEGER, REAL, TEXT, BOOLEAN)"));
                }
            };
            // VARCHAR(255) and friends: accept and ignore a length.
            if self.eat(&Token::LParen) {
                match self.next() {
                    Some(Token::Number(_)) => {}
                    _ => return Err(self.unexpected("a length")),
                }
                self.expect(&Token::RParen)?;
            }
            let mut col = ColumnDef {
                name: cname,
                ty,
                primary_key: false,
                not_null: false,
                unique: false,
            };
            loop {
                if self.eat_kw(Keyword::Primary) {
                    self.expect_kw(Keyword::Key)?;
                    col.primary_key = true;
                    col.not_null = true;
                } else if self.eat_kw(Keyword::Not) {
                    self.expect_kw(Keyword::Null)?;
                    col.not_null = true;
                } else if self.eat_kw(Keyword::Null) {
                    // explicit nullable: the default
                } else if self.eat_kw(Keyword::Unique) {
                    col.unique = true;
                } else {
                    break;
                }
            }
            columns.push(col);
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        self.expect(&Token::RParen)?;
        Ok(Statement::CreateTable(CreateTable {
            name,
            columns,
            if_not_exists,
        }))
    }

    fn drop_table(&mut self) -> Result<Statement> {
        self.expect_kw(Keyword::Drop)?;
        if self.eat_kw(Keyword::Index) {
            let if_exists = self.if_exists()?;
            return Ok(Statement::DropIndex {
                name: self.ident()?,
                if_exists,
            });
        }
        self.expect_kw(Keyword::Table)?;
        let if_exists = self.if_exists()?;
        Ok(Statement::DropTable {
            name: self.ident()?,
            if_exists,
        })
    }

    // --- expressions ----------------------------------------------------------

    fn expr_list(&mut self) -> Result<Vec<Expr>> {
        let mut v = vec![self.expr()?];
        while self.eat(&Token::Comma) {
            v.push(self.expr()?);
        }
        Ok(v)
    }

    pub fn expr(&mut self) -> Result<Expr> {
        self.or()
    }

    fn or(&mut self) -> Result<Expr> {
        let mut e = self.and()?;
        while self.eat_kw(Keyword::Or) {
            e = Expr::Binary(BinOp::Or, Box::new(e), Box::new(self.and()?));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr> {
        let mut e = self.not()?;
        while self.eat_kw(Keyword::And) {
            e = Expr::Binary(BinOp::And, Box::new(e), Box::new(self.not()?));
        }
        Ok(e)
    }

    fn not(&mut self) -> Result<Expr> {
        if self.eat_kw(Keyword::Not) {
            Ok(Expr::Unary(UnOp::Not, Box::new(self.not()?)))
        } else {
            self.predicate()
        }
    }

    fn predicate(&mut self) -> Result<Expr> {
        let left = self.concat()?;
        let op = match self.peek() {
            Some(Token::Eq) => Some(BinOp::Eq),
            Some(Token::NotEq) => Some(BinOp::NotEq),
            Some(Token::Lt) => Some(BinOp::Lt),
            Some(Token::LtEq) => Some(BinOp::LtEq),
            Some(Token::Gt) => Some(BinOp::Gt),
            Some(Token::GtEq) => Some(BinOp::GtEq),
            _ => None,
        };
        if let Some(op) = op {
            self.next();
            return Ok(Expr::Binary(op, Box::new(left), Box::new(self.concat()?)));
        }
        if self.eat_kw(Keyword::Is) {
            let neg = self.eat_kw(Keyword::Not);
            self.expect_kw(Keyword::Null)?;
            return Ok(Expr::IsNull(Box::new(left), neg));
        }
        // NOT LIKE / NOT IN / NOT BETWEEN
        let neg = matches!(self.peek(), Some(Token::Keyword(Keyword::Not)))
            && matches!(
                self.peek_at(1),
                Some(Token::Keyword(Keyword::Like | Keyword::In | Keyword::Between))
            );
        if neg {
            self.next();
        }
        if self.eat_kw(Keyword::Like) {
            return Ok(Expr::Like(Box::new(left), Box::new(self.concat()?), neg));
        }
        if self.eat_kw(Keyword::In) {
            self.expect(&Token::LParen)?;
            let list = self.expr_list()?;
            self.expect(&Token::RParen)?;
            return Ok(Expr::InList(Box::new(left), list, neg));
        }
        if self.eat_kw(Keyword::Between) {
            let lo = self.concat()?;
            self.expect_kw(Keyword::And)?;
            let hi = self.concat()?;
            return Ok(Expr::Between(Box::new(left), Box::new(lo), Box::new(hi), neg));
        }
        Ok(left)
    }

    fn concat(&mut self) -> Result<Expr> {
        let mut e = self.additive()?;
        while self.eat(&Token::Concat) {
            e = Expr::Binary(BinOp::Concat, Box::new(e), Box::new(self.additive()?));
        }
        Ok(e)
    }

    fn additive(&mut self) -> Result<Expr> {
        let mut e = self.term()?;
        loop {
            let op = match self.peek() {
                Some(Token::Plus) => BinOp::Add,
                Some(Token::Minus) => BinOp::Sub,
                _ => return Ok(e),
            };
            self.next();
            e = Expr::Binary(op, Box::new(e), Box::new(self.term()?));
        }
    }

    fn term(&mut self) -> Result<Expr> {
        let mut e = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Token::Star) => BinOp::Mul,
                Some(Token::Slash) => BinOp::Div,
                Some(Token::Percent) => BinOp::Mod,
                _ => return Ok(e),
            };
            self.next();
            e = Expr::Binary(op, Box::new(e), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat(&Token::Minus) {
            // Fold `-<number>` into a literal so that i64::MIN parses.
            if let Some(Token::Number(n)) = self.peek().cloned() {
                self.next();
                return number(&format!("-{n}")).map(Expr::Literal);
            }
            return Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)));
        }
        if self.eat(&Token::Plus) {
            return Ok(Expr::Unary(UnOp::Plus, Box::new(self.unary()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr> {
        match self.peek().cloned() {
            Some(Token::Number(n)) => {
                self.next();
                Ok(Expr::Literal(number(&n)?))
            }
            Some(Token::String(s)) => {
                self.next();
                Ok(Expr::Literal(Value::Text(s)))
            }
            Some(Token::Keyword(Keyword::Null)) => {
                self.next();
                Ok(Expr::Literal(Value::Null))
            }
            Some(Token::Keyword(Keyword::True)) => {
                self.next();
                Ok(Expr::Literal(Value::Boolean(true)))
            }
            Some(Token::Keyword(Keyword::False)) => {
                self.next();
                Ok(Expr::Literal(Value::Boolean(false)))
            }
            Some(Token::LParen) => {
                self.next();
                let e = self.expr()?;
                self.expect(&Token::RParen)?;
                Ok(e)
            }
            Some(Token::Ident(_) | Token::Keyword(_)) => {
                let name = self.ident()?;
                if self.eat(&Token::LParen) {
                    return self.function(name);
                }
                if self.eat(&Token::Dot) {
                    let col = self.ident()?;
                    return Ok(Expr::Column(Some(name), col));
                }
                Ok(Expr::Column(None, name))
            }
            _ => Err(self.unexpected("an expression")),
        }
    }

    /// Function call arguments; the name and `(` are already consumed.
    fn function(&mut self, name: String) -> Result<Expr> {
        if self.eat(&Token::Star) {
            self.expect(&Token::RParen)?;
            return Ok(Expr::Function {
                name,
                args: vec![],
                distinct: false,
                star: true,
            });
        }
        let distinct = self.eat_kw(Keyword::Distinct);
        let args = if self.peek() == Some(&Token::RParen) {
            vec![]
        } else {
            self.expr_list()?
        };
        self.expect(&Token::RParen)?;
        Ok(Expr::Function {
            name,
            args,
            distinct,
            star: false,
        })
    }
}

/// Parse a numeric literal: INTEGER if it has no `.`/exponent and fits in
/// an i64, otherwise REAL.
fn number(n: &str) -> Result<Value> {
    if !n.contains(['.', 'e', 'E']) {
        if let Ok(i) = n.parse::<i64>() {
            return Ok(Value::Integer(i));
        }
    }
    n.parse::<f64>()
        .map(Value::Real)
        .map_err(|_| Error::Parse(format!("invalid number {n}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(s: &str) -> Expr {
        match parse(&format!("SELECT {s}")).unwrap() {
            Statement::Select(sel) => match &sel.items[0] {
                SelectItem::Expr(e, _) => e.clone(),
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn precedence() {
        assert_eq!(expr("1 + 2 * 3").to_string(), "1 + (2 * 3)");
        assert_eq!(expr("(1 + 2) * 3").to_string(), "(1 + 2) * 3");
        assert_eq!(expr("a OR b AND NOT c = 1").to_string(), "a OR (b AND (NOT (c = 1)))");
        assert_eq!(expr("-x - -3").to_string(), "(-x) - -3");
        assert_eq!(expr("a || b = 'xy'").to_string(), "(a || b) = 'xy'");
        assert_eq!(expr("x NOT BETWEEN 1 AND 2 + 3").to_string(), "x NOT BETWEEN 1 AND (2 + 3)");
        assert_eq!(expr("x IS NOT NULL AND y NOT IN (1, 2)").to_string(), "(x IS NOT NULL) AND (y NOT IN (1, 2))");
        assert_eq!(expr("-9223372036854775808"), Expr::Literal(Value::Integer(i64::MIN)));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "a + b * c - d / e % f",
            "NOT (a OR b)",
            "-(a + b)",
            "t.x LIKE 'a%' AND y NOT LIKE '_b'",
            "COUNT(*) + SUM(DISTINCT x)",
            "\"Weird Name\" = 'it''s'",
        ] {
            let e = expr(s);
            assert_eq!(expr(&e.to_string()), e, "{s} -> {e}");
        }
    }

    #[test]
    fn full_select() {
        let s = parse(
            "SELECT DISTINCT a, b AS bee, COUNT(*) FROM t WHERE a > 1 \
             GROUP BY a, b HAVING COUNT(*) > 2 ORDER BY bee DESC, 1 LIMIT 10 OFFSET 5;",
        )
        .unwrap();
        let Statement::Select(sel) = s else { panic!() };
        assert!(sel.distinct);
        assert_eq!(sel.items.len(), 3);
        assert_eq!(sel.from.as_deref(), Some("t"));
        assert_eq!(sel.group_by.len(), 2);
        assert!(sel.having.is_some());
        assert_eq!(sel.order_by.len(), 2);
        assert!(!sel.order_by[0].asc);
        assert!(sel.limit.is_some() && sel.offset.is_some());
    }

    #[test]
    fn create_table_and_canonical_text() {
        let s = parse("create table Users (id int primary key, name varchar(20) not null, score real, \"key\" bool)")
            .unwrap();
        let Statement::CreateTable(ct) = s else { panic!() };
        assert_eq!(
            ct.to_string(),
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL, \"key\" BOOLEAN)"
        );
        assert_eq!(parse(&ct.to_string()).unwrap(), Statement::CreateTable(ct));
    }

    #[test]
    fn dml_and_txn_statements() {
        assert!(matches!(
            parse("INSERT INTO t (a, b) VALUES (1, 'x'), (2, NULL)").unwrap(),
            Statement::Insert { rows, .. } if rows.len() == 2
        ));
        assert!(matches!(
            parse("UPDATE t SET a = a + 1, b = 'y' WHERE a < 3").unwrap(),
            Statement::Update { set, filter: Some(_), .. } if set.len() == 2
        ));
        assert!(matches!(parse("DELETE FROM t").unwrap(), Statement::Delete { filter: None, .. }));
        assert_eq!(parse("BEGIN TRANSACTION").unwrap(), Statement::Begin);
        assert_eq!(parse_script("begin; commit; rollback;").unwrap().len(), 3);
        assert!(matches!(parse("EXPLAIN SELECT 1").unwrap(), Statement::Explain(_)));
        let Statement::CreateIndex(ci) = parse("create unique index if not exists i on t (a, \"B\")").unwrap() else {
            panic!()
        };
        assert!(ci.unique && ci.if_not_exists);
        assert_eq!(ci.to_string(), "CREATE UNIQUE INDEX i ON t (a, \"B\")");
        assert_eq!(parse(&ci.to_string()).unwrap(), Statement::CreateIndex(CreateIndex { if_not_exists: false, ..ci }));
        assert!(matches!(parse("DROP INDEX IF EXISTS i").unwrap(), Statement::DropIndex { if_exists: true, .. }));
        let Statement::CreateTable(ct) = parse("CREATE TABLE t (a INTEGER UNIQUE NOT NULL)").unwrap() else { panic!() };
        assert_eq!(ct.to_string(), "CREATE TABLE t (a INTEGER NOT NULL UNIQUE)");
    }

    #[test]
    fn helpful_errors() {
        let e = parse("SELECT a FROM").unwrap_err().to_string();
        assert!(e.contains("expected an identifier, found end of input"), "{e}");
        let e = parse("SELECT a,\nFROM t").unwrap_err().to_string();
        assert!(e.contains("line 2, column 1"), "{e}");
        let e = parse("CREATE TABLE t (a BLOB)").unwrap_err().to_string();
        assert!(e.contains("column type"), "{e}");
        assert!(parse("SELECT 1 SELECT 2").is_err());
        assert!(parse("").is_err());
    }
}
