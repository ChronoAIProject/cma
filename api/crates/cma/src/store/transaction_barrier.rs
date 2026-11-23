//! Every store transaction opens with one read before its first write.
//!
//! The Rust driver (3.9.1, unchanged in 3.10.2) aborts a dropped session's
//! transaction only once the transaction's first command has returned. A
//! caller cancelled while that first command is in flight (a request whose
//! client disconnected, a bounded timeout, an authority revocation) leaves the
//! server transaction open with that command's write locks until the server
//! session is reused or the transaction lifetime expires (#3189, #3217).
//! Opening with a read means such a cancellation strands at most a lock-free
//! snapshot, and the driver aborts every later drop as intended.
use mongodb::{
    bson::{doc, Bson, Document},
    error::Result,
    ClientSession,
};

/// `agents` is in every credential's exact collection grant, so the read is
/// authorized in both shared-schema and isolated modes.
const COLLECTION: &str = "agents";

/// Test clients carry no configured database. They read this one instead, so
/// every test transaction opens exactly as production does. No test creates or
/// drops it, so its intent lock never waits behind another test's DDL.
#[cfg(test)]
const TEST_DATABASE: &str = "cma_transaction_barrier_tests";

/// The barrier reads the transaction's own database: the one its client was
/// configured with. Production clients always carry it (`connection_options`
/// and the runtime routing client); outside tests a client without one skips
/// the barrier rather than read a database its credential may not cover.
/// The read is an `_id` lookup that matches nothing. A stranded barrier holds
/// only an intent-shared lock on `agents`, which conflicts with no write.
pub(crate) async fn open(session: &mut ClientSession) -> Result<()> {
    let database = match session.client().default_database() {
        Some(database) => database,
        #[cfg(test)]
        None => session.client().database(TEST_DATABASE),
        #[cfg(not(test))]
        None => return Ok(()),
    };
    database
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id": Bson::MinKey})
        .projection(doc! {"_id": 1})
        .session(session)
        .await
        .map(|_| ())
}
