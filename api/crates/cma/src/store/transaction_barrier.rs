//! Every store transaction opens with one read before its first write.
//!
//! The Rust driver (3.9.1, unchanged in 3.10.2) aborts a dropped session's
//! transaction only once the transaction's first command has returned. A
//! caller cancelled while that first command is in flight (a request whose
//! client disconnected, a bounded timeout, an authority revocation) leaves the
//! server transaction open with that command's write locks until the server
//! session is reused or the transaction lifetime expires (#3189, #3217).
//! The collectionless read establishes the snapshot without taking a
//! collection lock. It lets the driver abort later operations on drop.
use mongodb::{bson::doc, error::Result, ClientSession};

/// Open on the client's configured application database. The literal
/// aggregate needs no collection permissions and lets an agents index build
/// proceed while the transaction holds its snapshot (#3276).
/// Clients without an application database skip the barrier.
pub(crate) async fn open(session: &mut ClientSession) -> Result<()> {
    let Some(database) = session
        .client()
        .default_database()
        .filter(|database| barrier_database(database.name()))
    else {
        return Ok(());
    };
    database
        .run_command(doc! {
            "aggregate": 1_i32,
            "pipeline": [{ "$documents": [doc! {}] }],
            "cursor": {},
        })
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
