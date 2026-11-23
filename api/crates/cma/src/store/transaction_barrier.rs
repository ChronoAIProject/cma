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

/// The barrier reads the transaction's own database: the one its client was
/// configured with. Production clients always carry it (`connection_options`
/// and the runtime routing client). A client without one, such as an operator
/// CLI or most test fixtures, skips the barrier rather than read a database its
/// credential may not cover; tests that need production's shape configure it.
/// The read is an `_id` lookup that matches nothing. A stranded barrier holds
/// only an intent-shared lock on `agents`: it conflicts with no write, but it
/// does delay the exclusive phases of new-index DDL on `agents` (#3276).
pub(crate) async fn open(session: &mut ClientSession) -> Result<()> {
    let Some(database) = session
        .client()
        .default_database()
        .filter(|database| barrier_database(database.name()))
    else {
        return Ok(());
    };
    database
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id": Bson::MinKey})
        .projection(doc! {"_id": 1})
        .session(session)
        .await
        .map(|_| ())
}

/// Binds the database the barrier reads to the configured one. A connection
/// string's path (often only the authentication database) must never select it.
pub(crate) fn bind(options: &mut mongodb::options::ClientOptions, database: &str) {
    options.default_database = Some(database.to_owned());
}

/// System databases cannot host a transactional read; such a client skips the
/// barrier instead of failing every transaction it opens.
fn barrier_database(name: &str) -> bool {
    !matches!(name, "admin" | "local" | "config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_configured_database_overrides_a_connection_string_path() {
        let mut options =
            mongodb::options::ClientOptions::parse("mongodb://user:secret@localhost/admin")
                .await
                .unwrap();
        assert_eq!(options.default_database.as_deref(), Some("admin"));
        bind(&mut options, "cma");
        assert_eq!(options.default_database.as_deref(), Some("cma"));
        assert_eq!(
            options
                .credential
                .as_ref()
                .and_then(|credential| credential.source.as_deref()),
            Some("admin")
        );
    }

    #[test]
    fn system_databases_never_host_the_barrier() {
        for name in ["admin", "local", "config"] {
            assert!(!barrier_database(name), "{name}");
        }
        assert!(barrier_database("cma"));
    }
}
