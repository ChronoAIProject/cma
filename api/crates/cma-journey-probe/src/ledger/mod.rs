mod misses;
mod observe;
mod reserve;

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use thiserror::Error;

use crate::{
    config::Config,
    model::{now, Job, Provenance},
};

#[derive(Debug, Error)]
pub enum Error {
    #[error("database operation failed")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid state or witness")]
    Fence,
    #[error("evidence serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("state or evidence file operation failed")]
    Io(#[from] std::io::Error),
    #[error("evidence limit reached")]
    Capacity,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Ledger {
    state: PathBuf,
    witness: PathBuf,
    domain: String,
    provenance: Provenance,
}

const META: &str = "CREATE TABLE fence (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    domain TEXT NOT NULL,
    generation TEXT NOT NULL,
    serial INTEGER NOT NULL
);";

const SCHEMA: &str = "
CREATE TABLE jobs (
    job TEXT PRIMARY KEY,
    offset INTEGER NOT NULL,
    enabled_since INTEGER,
    last_slot INTEGER,
    last_attempt INTEGER,
    last_completed INTEGER,
    last_success INTEGER,
    last_receipt INTEGER,
    last_terminal INTEGER
);
CREATE TABLE slots (
    job TEXT NOT NULL,
    slot INTEGER NOT NULL,
    attempted_at INTEGER,
    intent TEXT,
    target TEXT,
    funded_cost INTEGER NOT NULL DEFAULT 0,
    first_result TEXT,
    settled INTEGER NOT NULL DEFAULT 0,
    recovery_count INTEGER NOT NULL DEFAULT 0,
    next_recovery INTEGER NOT NULL,
    PRIMARY KEY (job, slot)
);
CREATE INDEX slot_time ON slots (slot);
CREATE INDEX in_flight ON slots (job,attempted_at) WHERE first_result IS NULL;
CREATE INDEX unresolved ON slots (next_recovery) WHERE settled = 0;
CREATE INDEX unresolved_target ON slots (target) WHERE settled = 0;
CREATE INDEX funded_reservations ON slots (attempted_at) WHERE funded_cost > 0;
CREATE TABLE recovery (
    job TEXT NOT NULL,
    slot INTEGER NOT NULL,
    attempt INTEGER NOT NULL,
    result TEXT NOT NULL,
    PRIMARY KEY (job, slot, attempt),
    FOREIGN KEY (job, slot) REFERENCES slots (job, slot) ON DELETE CASCADE
);
CREATE TABLE counters (
    job TEXT NOT NULL,
    phase TEXT NOT NULL,
    reason TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (job, phase, reason)
);
CREATE TABLE duration (
    job TEXT NOT NULL,
    phase TEXT NOT NULL,
    count INTEGER NOT NULL,
    sum_ms INTEGER NOT NULL,
    b100 INTEGER NOT NULL,
    b500 INTEGER NOT NULL,
    b1000 INTEGER NOT NULL,
    b5000 INTEGER NOT NULL,
    b30000 INTEGER NOT NULL,
    b120000 INTEGER NOT NULL,
    PRIMARY KEY (job, phase)
);
";

#[derive(PartialEq, Eq)]
struct Fence {
    domain: String,
    generation: String,
    serial: i64,
}

fn connect(path: &Path, write: bool) -> Result<Connection> {
    let mode = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let connection = Connection::open_with_flags(path, mode | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    connection.busy_timeout(Duration::from_millis(350))?;
    if write {
        connection.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")?;
    }
    Ok(connection)
}

fn metadata(conn: &Connection) -> Result<Fence> {
    Ok(conn.query_row(
        "SELECT domain, generation, serial FROM fence WHERE singleton=1",
        [],
        |row| {
            Ok(Fence {
                domain: row.get(0)?,
                generation: row.get(1)?,
                serial: row.get(2)?,
            })
        },
    )?)
}

fn create(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    Ok(())
}

impl Ledger {
    pub fn new(config: &Config) -> Self {
        Self {
            state: config.state_path.clone(),
            witness: config.witness_path.clone(),
            domain: config.domain(),
            provenance: config.provenance(),
        }
    }

    pub fn init(&self) -> Result<()> {
        if self.state.exists() || self.witness.exists() {
            return Err(Error::Fence);
        }
        create(&self.state)?;
        create(&self.witness)?;
        let generation = uuid::Uuid::new_v4().to_string();
        for path in [&self.state, &self.witness] {
            let mut connection = connect(path, true)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(META)?;
            transaction.execute(
                "INSERT INTO fence VALUES (1, ?1, ?2, 0)",
                (&self.domain, &generation),
            )?;
            if path == &self.state {
                transaction.execute_batch(SCHEMA)?;
                transaction.execute_batch(misses::SCHEMA)?;
                transaction.execute_batch(observe::PLACEHOLDERS)?;
            }
            transaction.commit()?;
        }
        // Provisioning both files is required before a worker can make any reservation.
        for path in [&self.state, &self.witness] {
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::File::open(parent)?.sync_all()?;
            }
        }
        Ok(())
    }

    fn check(&self, transaction: &Connection, witness: &Connection) -> Result<Fence> {
        let fence = metadata(transaction)?;
        if fence.domain != self.domain || fence != metadata(witness)? {
            return Err(Error::Fence);
        }
        Ok(fence)
    }

    fn write<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>, &mut Connection, Fence) -> Result<T>,
    ) -> Result<T> {
        let mut state = connect(&self.state, true)?;
        // Give all workers one lock order, including newest-state and restored-state processes.
        let transaction = state.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut witness = connect(&self.witness, true)?;
        let fence = self.check(&transaction, &witness)?;
        let result = operation(&transaction, &mut witness, fence)?;
        transaction.commit()?;
        Ok(result)
    }

    pub fn configure(&self, jobs: &[(Job, bool)]) -> Result<()> {
        self.configure_at(jobs, now())
    }

    pub fn configure_at(&self, jobs: &[(Job, bool)], at: i64) -> Result<()> {
        self.write(|tx, _, _| {
            tx.execute_batch(misses::SCHEMA)?;
            tx.execute_batch(observe::PLACEHOLDERS)?;
            for &(job, enabled) in jobs {
                let key = job.key();
                let stored: Option<(i64, Option<i64>, Option<i64>)> = optional(tx.query_row(
                    "SELECT offset,enabled_since,last_slot FROM jobs WHERE job=?1",
                    [&key],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ))?;
                if let Some((offset, since, last_slot)) = stored {
                    if offset != job.offset {
                        return Err(Error::Fence);
                    }
                    if let (false, Some(since)) = (enabled, since) {
                        // Close the outgoing enabled interval using the same UTC schedule as dispatch.
                        misses::record(tx, job, &self.provenance, since, last_slot, at, at)?;
                    }
                    let enabled_since = enabled.then_some(since.unwrap_or(at));
                    tx.execute(
                        "UPDATE jobs SET enabled_since=?2 WHERE job=?1",
                        (&key, enabled_since),
                    )?;
                } else {
                    tx.execute(
                        "INSERT INTO jobs(job,offset,enabled_since) VALUES (?1,?2,?3)",
                        (&key, job.offset, enabled.then_some(at)),
                    )?;
                }
            }
            Ok(())
        })
    }

    pub fn available(&self) -> Result<()> {
        self.write(|_, _, _| Ok(()))
    }

    fn advance_witness(tx: &Transaction<'_>, witness: &mut Connection, serial: i64) -> Result<()> {
        let next = serial.checked_add(1).ok_or(Error::Fence)?;
        tx.execute("UPDATE fence SET serial=?1 WHERE singleton=1", [next])?;
        let reservation = witness.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if reservation.execute(
            "UPDATE fence SET serial=?1 WHERE singleton=1 AND serial=?2",
            (next, serial),
        )? != 1
        {
            return Err(Error::Fence);
        }
        // A crash between the two commits leaves the witness ahead and dispatch fenced.
        reservation.commit()?;
        Ok(())
    }

    pub fn prune(&self, at: i64) -> Result<()> {
        self.write(|tx, _, _| {
            tx.execute("DELETE FROM missed_ranges WHERE last_slot<?1", [at - observe::RETENTION])?;
            tx.execute("DELETE FROM slots WHERE rowid IN (SELECT rowid FROM slots WHERE slot<?1 AND settled=1 LIMIT 2000)", [at - observe::RETENTION])?;
            Ok(())
        })
    }
}

fn optional<T>(result: rusqlite::Result<T>) -> Result<Option<T>> {
    Ok(result.optional()?)
}

pub use observe::Metrics;
pub use reserve::{Claim, Recovery};
