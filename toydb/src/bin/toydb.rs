//! `toydb` — an interactive SQL shell.
//!
//! ```text
//! toydb path.db                 interactive shell on a database file
//! toydb                         interactive shell on a throwaway in-memory database
//! toydb path.db "SQL; SQL"      run statements and exit
//! toydb path.db < script.sql    run a script from stdin
//! ```
//!
//! Statements end with `;` and may span several lines. Lines starting with
//! `.` are shell commands; type `.help` to list them.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use toydb::format;
use toydb::storage::file::MemVfs;
use toydb::{Database, Options, QueryResult, Session};

const HELP: &str = "\
Statements (end each with ;):
  CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL, ok BOOLEAN)
  DROP TABLE [IF EXISTS] t
  INSERT INTO t [(cols)] VALUES (...), (...)
  SELECT [DISTINCT] exprs FROM t [WHERE ...] [GROUP BY ...] [HAVING ...]
         [ORDER BY ... [DESC]] [LIMIT n [OFFSET m]]
  UPDATE t SET col = expr, ... [WHERE ...]
  DELETE FROM t [WHERE ...]
  BEGIN / COMMIT / ROLLBACK
  EXPLAIN <statement>          show the query plan

Shell commands:
  .help                        this text
  .tables                      list tables
  .schema [table]              show CREATE TABLE statements
  .stats                       page cache and file statistics
  .timer on|off                print how long each statement takes
  .read FILE                   execute statements from a file
  .checkpoint                  flush the WAL into the database file
  .quit                        exit (also Ctrl-D)
";

struct Shell {
    db: Database,
    session: Session,
    timer: bool,
}

/// True if `sql` ends with a `;` that isn't inside a string, quoted
/// identifier or comment — i.e. the statement is ready to run.
fn statement_complete(sql: &str) -> bool {
    let mut chars = sql.chars().peekable();
    let mut quote: Option<char> = None;
    let mut last_significant = None;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    last_significant = Some(c);
                }
                '-' if chars.peek() == Some(&'-') => {
                    // comment to end of line
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                c if c.is_whitespace() => {}
                c => last_significant = Some(c),
            },
        }
    }
    quote.is_none() && last_significant == Some(';')
}

impl Shell {
    fn run_sql(&mut self, sql: &str) -> bool {
        let start = Instant::now();
        let mut ok = true;
        match toydb::sql::parser::parse_script(sql) {
            Err(e) => {
                eprintln!("Error: {e}");
                ok = false;
            }
            Ok(stmts) => {
                for stmt in stmts {
                    match self.session.execute_statement(&stmt) {
                        Ok(r) => print!("{}", format::result(&r)),
                        Err(e) => {
                            eprintln!("Error: {e}");
                            ok = false;
                            break;
                        }
                    }
                }
            }
        }
        if self.timer {
            println!("Time: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        }
        ok
    }

    /// Handle a `.command`. Returns false to quit.
    fn meta(&mut self, line: &str) -> bool {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next();
        match cmd {
            ".quit" | ".exit" | ".q" => return false,
            ".help" => print!("{HELP}"),
            ".tables" => match self.db.table_names() {
                Ok(names) => names.iter().for_each(|n| println!("{n}")),
                Err(e) => eprintln!("Error: {e}"),
            },
            ".schema" => {
                let sql = match arg {
                    Some(t) => format!("SELECT sql FROM toydb_schema WHERE name = '{}'", t.replace('\'', "''")),
                    None => "SELECT sql FROM toydb_schema".to_string(),
                };
                match self.session.execute(&sql) {
                    Ok(QueryResult::Rows { rows, .. }) => {
                        for r in rows {
                            println!("{};", r[0]);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("Error: {e}"),
                }
            }
            ".stats" => {
                let s = self.db.stats();
                println!("pages in file:   {} ({} KiB)", s.page_count, s.page_count as u64 * 4);
                println!("free pages:      {}", s.free_pages);
                println!("WAL size:        {} bytes", s.wal_bytes);
                let total = s.cache_hits + s.cache_misses;
                let rate = if total > 0 { 100.0 * s.cache_hits as f64 / total as f64 } else { 0.0 };
                println!("cache hits:      {} ({rate:.1}%)", s.cache_hits);
                println!("cache misses:    {}", s.cache_misses);
                println!("evictions:       {}", s.evictions);
            }
            ".timer" => match arg {
                Some("on") => self.timer = true,
                Some("off") => self.timer = false,
                _ => eprintln!("usage: .timer on|off"),
            },
            ".read" => match arg.map(std::fs::read_to_string) {
                Some(Ok(sql)) => {
                    self.run_sql(&sql);
                }
                Some(Err(e)) => eprintln!("Error: {e}"),
                None => eprintln!("usage: .read FILE"),
            },
            ".checkpoint" => {
                if let Err(e) = self.db.checkpoint() {
                    eprintln!("Error: {e}");
                }
            }
            _ => eprintln!("Unknown command {cmd}. Type .help for help."),
        }
        true
    }

    fn repl(&mut self) {
        let interactive = io::stdin().is_terminal();
        if interactive {
            println!("toydb {} — type .help for help, .quit to exit", env!("CARGO_PKG_VERSION"));
        }
        let mut buf = String::new();
        let stdin = io::stdin();
        let mut lines = stdin.lock().lines();
        loop {
            if interactive {
                let prompt = match (buf.is_empty(), self.session.in_transaction()) {
                    (true, false) => "toydb> ",
                    (true, true) => "toydb*> ",
                    (false, _) => "   ...> ",
                };
                print!("{prompt}");
                let _ = io::stdout().flush();
            }
            let Some(Ok(line)) = lines.next() else { break };
            if buf.is_empty() && line.trim_start().starts_with('.') {
                if !self.meta(line.trim()) {
                    return;
                }
                continue;
            }
            buf.push_str(&line);
            buf.push('\n');
            if statement_complete(&buf) {
                let sql = std::mem::take(&mut buf);
                self.run_sql(&sql);
            }
        }
        if !buf.trim().is_empty() {
            // Input ended mid-statement: run what we have.
            self.run_sql(&buf);
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("usage: toydb [DATABASE] [SQL]\n\n{HELP}");
        return ExitCode::SUCCESS;
    }
    let db = match args.first() {
        Some(path) => Database::open(Path::new(path)),
        None => {
            eprintln!("(no database file given: using a temporary in-memory database)");
            Database::open_vfs(&MemVfs::new(), "memory.db", Options::default())
        }
    };
    let db = match db {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Error: cannot open database: {e}");
            return ExitCode::FAILURE;
        }
    };
    let session = db.session();
    let mut shell = Shell {
        db,
        session,
        timer: false,
    };
    if let Some(sql) = args.get(1) {
        return if shell.run_sql(sql) { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    shell.repl();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::statement_complete;

    #[test]
    fn detects_statement_end() {
        assert!(statement_complete("SELECT 1;"));
        assert!(statement_complete("SELECT 1; -- trailing comment\n"));
        assert!(!statement_complete("SELECT 'a;b'"));
        assert!(!statement_complete("SELECT 'a;"));
        assert!(statement_complete("SELECT 'a;'\n;"));
        assert!(!statement_complete("SELECT 1 -- not yet;\n"));
        assert!(!statement_complete("SELECT \"x;"));
    }
}
