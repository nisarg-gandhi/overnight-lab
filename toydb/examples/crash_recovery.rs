//! Watch crash recovery work.
//!
//! We use `MemVfs`, an in-memory file system that can simulate a power cut:
//! every write that wasn't fsynced may be lost or torn. Two scenarios:
//!
//! 1. The power fails *during* a COMMIT, before the WAL reached disk. The
//!    commit returns an error and the transaction is gone after reboot.
//! 2. The power fails right *after* a COMMIT returned successfully. The
//!    database file may never have received the new pages, but the WAL was
//!    fsynced, so recovery replays it and the row is there after reboot.
//!
//! Run with: cargo run --example crash_recovery

use toydb::storage::file::MemVfs;
use toydb::{format, Database, Options};

fn main() -> toydb::Result<()> {
    let vfs = MemVfs::new();
    {
        let db = Database::open_vfs(&vfs, "crash.db", Options::default())?;
        let mut s = db.session();
        s.execute("CREATE TABLE events (id INTEGER PRIMARY KEY, what TEXT)")?;
        s.execute("INSERT INTO events VALUES (1, 'committed before the crash')")?;

        s.execute("BEGIN")?;
        s.execute("INSERT INTO events VALUES (2, 'in flight when the power failed')")?;
        vfs.fail_after(0); // the very next write fails: power is out
        match s.execute("COMMIT") {
            Ok(_) => println!("commit unexpectedly succeeded"),
            Err(e) => println!("scenario 1: COMMIT failed with: {e}"),
        }
        // The database object is now unusable ("poisoned"); dropping it is
        // all we can do, exactly like a process that died.
    }
    vfs.crash(1); // reboot: unsynced writes are lost or torn at random
    {
        let db = Database::open_vfs(&vfs, "crash.db", Options::default())?;
        println!("after reboot (row 2 must be missing):");
        print!("{}", format::result(&db.execute("SELECT * FROM events")?));

        db.execute("INSERT INTO events VALUES (3, 'acknowledged just before the crash')")?;
        println!("scenario 2: COMMIT of row 3 returned OK; cutting the power now");
        vfs.fail_after(0); // no clean shutdown, no checkpoint
    }
    vfs.crash(2);
    let db = Database::open_vfs(&vfs, "crash.db", Options::default())?;
    println!("after reboot (row 3 must be present, replayed from the WAL):");
    print!("{}", format::result(&db.execute("SELECT * FROM events")?));
    Ok(())
}
