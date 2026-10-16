//! Concurrent transactions: several threads move money between accounts.
//!
//! Each transfer reads two balances and writes them back inside a
//! transaction. Without proper isolation, concurrent transfers would lose
//! updates and the total would drift. toydb's strict two-phase locking makes
//! the schedule serializable, so the total is always preserved. When two
//! transactions collide, one gets a `Busy` error and simply retries.
//!
//! Run with: cargo run --release --example bank

use std::thread;
use std::time::{Duration, Instant};

use toydb::storage::file::MemVfs;
use toydb::util::Rng;
use toydb::{Database, Error, Options, QueryResult, Session, Value};

const ACCOUNTS: u64 = 20;
const THREADS: u64 = 4;
const TRANSFERS: u64 = 300;

fn balance(s: &mut Session, id: u64) -> toydb::Result<i64> {
    match s.execute(&format!("SELECT balance FROM accounts WHERE id = {id}"))? {
        QueryResult::Rows { rows, .. } => match rows[0][0] {
            Value::Integer(b) => Ok(b),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    }
}

fn transfer(s: &mut Session, from: u64, to: u64, amount: i64) -> toydb::Result<bool> {
    s.execute("BEGIN")?;
    let from_balance = balance(s, from)?;
    if from_balance < amount {
        s.execute("ROLLBACK")?;
        return Ok(false); // insufficient funds
    }
    let to_balance = balance(s, to)?;
    s.execute(&format!("UPDATE accounts SET balance = {} WHERE id = {from}", from_balance - amount))?;
    s.execute(&format!("UPDATE accounts SET balance = {} WHERE id = {to}", to_balance + amount))?;
    s.execute("COMMIT")?;
    Ok(true)
}

fn main() -> toydb::Result<()> {
    // An in-memory file system keeps the example self-contained.
    let db = Database::open_vfs(&MemVfs::new(), "bank.db", Options::default())?;
    let mut s = db.session();
    s.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, balance INTEGER NOT NULL)")?;
    for id in 0..ACCOUNTS {
        s.execute(&format!("INSERT INTO accounts VALUES ({id}, 1000)"))?;
    }

    let start = Instant::now();
    let workers: Vec<_> = (0..THREADS)
        .map(|t| {
            let db = db.clone();
            thread::spawn(move || {
                let mut s = db.session();
                let mut rng = Rng::new(t);
                let (mut done, mut retries) = (0, 0);
                while done < TRANSFERS {
                    let (from, to) = (rng.below(ACCOUNTS), rng.below(ACCOUNTS));
                    if from == to {
                        continue; // reading then writing one row twice would double-count
                    }
                    match transfer(&mut s, from, to, rng.below(200) as i64) {
                        Ok(_) => done += 1,
                        Err(Error::Busy(_)) => {
                            // Lost a lock conflict: roll back and try again.
                            if s.in_transaction() {
                                s.execute("ROLLBACK").unwrap();
                            }
                            retries += 1;
                            thread::sleep(Duration::from_micros(50 * (t + 1)));
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
                retries
            })
        })
        .collect();
    let retries: u64 = workers.into_iter().map(|w| w.join().unwrap()).sum();

    let total = match s.execute("SELECT SUM(balance), MIN(balance), MAX(balance) FROM accounts")? {
        QueryResult::Rows { rows, .. } => rows[0].clone(),
        _ => unreachable!(),
    };
    println!(
        "{} transfers by {THREADS} threads in {:?} ({retries} retries after lock conflicts)",
        THREADS * TRANSFERS,
        start.elapsed()
    );
    println!("total = {}, min = {}, max = {}", total[0], total[1], total[2]);
    assert_eq!(total[0], Value::Integer(ACCOUNTS as i64 * 1000), "money was created or destroyed!");
    println!("invariant holds: no money created or destroyed");
    Ok(())
}
