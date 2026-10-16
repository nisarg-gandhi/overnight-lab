//! Concurrency control: database-level shared/exclusive locks.
//!
//! toydb allows many readers *or* one writer at a time, using strict
//! two-phase locking at the granularity of the whole database:
//!
//! * A transaction takes a **shared** lock before its first read and an
//!   **exclusive** lock before its first write (upgrading if it already
//!   holds a shared lock).
//! * Locks are held until COMMIT or ROLLBACK ("strict" 2PL). This is what
//!   makes the schedule serializable, and it means readers never see a
//!   writer's uncommitted pages: the writer's exclusive lock keeps them out.
//!
//! ```text
//!                 held by others:  none     shared    exclusive
//!   request shared                 grant    grant     wait
//!   request exclusive              grant    wait      wait
//! ```
//!
//! **Upgrade deadlock.** Two transactions that both hold shared locks and
//! both want to upgrade would wait for each other forever. Like SQLite, we
//! refuse the second upgrade immediately with [`Error::Busy`]; that
//! transaction should roll back (releasing its shared lock) so the first
//! can proceed. Every other wait has a timeout, which also turns any
//! remaining deadlock into a `Busy` error instead of a hang.
//!
//! **Writer preference.** While a writer is waiting, new readers queue
//! behind it, so a steady stream of readers can't starve writers.
//!
//! Snapshot isolation (readers that don't block writers) would need
//! multiple versions of pages; see DESIGN.md for how that could be added.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// Identifies a session (connection) for locking purposes.
pub type SessionId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LockMode {
    Shared,
    Exclusive,
}

#[derive(Default, Debug)]
struct LockState {
    readers: HashSet<SessionId>,
    writer: Option<SessionId>,
    /// A session waiting for the exclusive lock (at most one may wait while
    /// holding a shared lock; see "upgrade deadlock").
    waiting_writer: Option<SessionId>,
}

/// The lock table. One per open database.
#[derive(Default, Debug)]
pub struct LockManager {
    state: Mutex<LockState>,
    changed: Condvar,
}

impl LockManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The strongest lock `sid` currently holds.
    pub fn held(&self, sid: SessionId) -> Option<LockMode> {
        let st = self.state.lock().unwrap();
        if st.writer == Some(sid) {
            Some(LockMode::Exclusive)
        } else if st.readers.contains(&sid) {
            Some(LockMode::Shared)
        } else {
            None
        }
    }

    /// Acquire (or upgrade to) `mode` for `sid`, waiting at most `timeout`.
    /// Re-acquiring a lock you already hold is a no-op.
    pub fn acquire(&self, sid: SessionId, mode: LockMode, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut st = self.state.lock().unwrap();
        loop {
            match mode {
                LockMode::Shared => {
                    if st.writer == Some(sid) || st.readers.contains(&sid) {
                        return Ok(());
                    }
                    if st.writer.is_none() && st.waiting_writer.is_none() {
                        st.readers.insert(sid);
                        return Ok(());
                    }
                }
                LockMode::Exclusive => {
                    if st.writer == Some(sid) {
                        return Ok(());
                    }
                    let others_reading = st.readers.iter().any(|&r| r != sid);
                    if st.writer.is_none() && !others_reading {
                        st.readers.remove(&sid);
                        st.writer = Some(sid);
                        if st.waiting_writer == Some(sid) {
                            st.waiting_writer = None;
                        }
                        return Ok(());
                    }
                    match st.waiting_writer {
                        None => st.waiting_writer = Some(sid),
                        Some(w) if w == sid => {}
                        Some(_) if st.readers.contains(&sid) => {
                            // Someone else is already upgrading and waits for
                            // *our* shared lock to go away: waiting would
                            // deadlock.
                            return Err(Error::Busy(
                                "another transaction is upgrading to a write lock; roll back and retry"
                                    .into(),
                            ));
                        }
                        Some(_) => {} // queue behind the other writer
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                if st.waiting_writer == Some(sid) {
                    st.waiting_writer = None;
                    self.changed.notify_all();
                }
                return Err(Error::Busy(format!(
                    "timed out after {timeout:?} waiting for {} lock",
                    if mode == LockMode::Shared { "a read" } else { "a write" }
                )));
            }
            st = self.changed.wait_timeout(st, deadline - now).unwrap().0;
        }
    }

    /// Release every lock held by `sid` (end of transaction).
    pub fn release(&self, sid: SessionId) {
        let mut st = self.state.lock().unwrap();
        st.readers.remove(&sid);
        if st.writer == Some(sid) {
            st.writer = None;
        }
        if st.waiting_writer == Some(sid) {
            st.waiting_writer = None;
        }
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    const T: Duration = Duration::from_millis(100);
    const LONG: Duration = Duration::from_secs(5);

    #[test]
    fn shared_locks_coexist() {
        let lm = LockManager::new();
        lm.acquire(1, LockMode::Shared, T).unwrap();
        lm.acquire(2, LockMode::Shared, T).unwrap();
        assert_eq!(lm.held(1), Some(LockMode::Shared));
        assert!(matches!(lm.acquire(3, LockMode::Exclusive, T), Err(Error::Busy(_))));
        lm.release(1);
        lm.release(2);
        lm.acquire(3, LockMode::Exclusive, T).unwrap();
        assert_eq!(lm.held(3), Some(LockMode::Exclusive));
    }

    #[test]
    fn exclusive_excludes_everyone() {
        let lm = LockManager::new();
        lm.acquire(1, LockMode::Exclusive, T).unwrap();
        lm.acquire(1, LockMode::Shared, T).unwrap(); // re-entrant, stays X
        assert_eq!(lm.held(1), Some(LockMode::Exclusive));
        assert!(lm.acquire(2, LockMode::Shared, T).is_err());
        assert!(lm.acquire(2, LockMode::Exclusive, T).is_err());
        lm.release(1);
        lm.acquire(2, LockMode::Shared, T).unwrap();
    }

    #[test]
    fn upgrade_when_sole_reader() {
        let lm = LockManager::new();
        lm.acquire(1, LockMode::Shared, T).unwrap();
        lm.acquire(1, LockMode::Exclusive, T).unwrap();
        assert_eq!(lm.held(1), Some(LockMode::Exclusive));
    }

    #[test]
    fn upgrade_deadlock_is_detected() {
        let lm = Arc::new(LockManager::new());
        lm.acquire(1, LockMode::Shared, T).unwrap();
        lm.acquire(2, LockMode::Shared, T).unwrap();
        // Session 1 starts upgrading in the background; it must wait for 2.
        let lm2 = lm.clone();
        let h = thread::spawn(move || lm2.acquire(1, LockMode::Exclusive, LONG));
        while lm.state.lock().unwrap().waiting_writer.is_none() {
            thread::yield_now();
        }
        // Session 2's upgrade would deadlock, so it fails immediately...
        let t0 = Instant::now();
        assert!(matches!(lm.acquire(2, LockMode::Exclusive, LONG), Err(Error::Busy(_))));
        assert!(t0.elapsed() < Duration::from_secs(1));
        // ...and once it rolls back, session 1 gets the write lock.
        lm.release(2);
        h.join().unwrap().unwrap();
        assert_eq!(lm.held(1), Some(LockMode::Exclusive));
    }

    #[test]
    fn waiting_writer_blocks_new_readers() {
        let lm = Arc::new(LockManager::new());
        lm.acquire(1, LockMode::Shared, T).unwrap();
        let lm2 = lm.clone();
        let h = thread::spawn(move || lm2.acquire(2, LockMode::Exclusive, LONG));
        while lm.state.lock().unwrap().waiting_writer.is_none() {
            thread::yield_now();
        }
        assert!(lm.acquire(3, LockMode::Shared, T).is_err());
        lm.release(1);
        h.join().unwrap().unwrap();
    }

    #[test]
    fn timed_out_writer_stops_blocking_readers() {
        let lm = LockManager::new();
        lm.acquire(1, LockMode::Shared, T).unwrap();
        assert!(lm.acquire(2, LockMode::Exclusive, T).is_err());
        lm.acquire(3, LockMode::Shared, T).unwrap();
    }

    #[test]
    fn many_threads_mutual_exclusion() {
        // Writers increment a counter non-atomically under the X lock; any
        // overlap would lose updates.
        let lm = Arc::new(LockManager::new());
        let counter = Arc::new(Mutex::new(0u64));
        let mut hs = vec![];
        for sid in 0..8u64 {
            let (lm, counter) = (lm.clone(), counter.clone());
            hs.push(thread::spawn(move || {
                for _ in 0..200 {
                    lm.acquire(sid, LockMode::Exclusive, LONG).unwrap();
                    let v = *counter.lock().unwrap();
                    thread::yield_now();
                    *counter.lock().unwrap() = v + 1;
                    lm.release(sid);
                }
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(*counter.lock().unwrap(), 1600);
    }
}
