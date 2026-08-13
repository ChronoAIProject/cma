//! Durable storage, wired to concrete backends.
//!
//! The core defines the ports; these are the implementations. Keeping them in
//! the binary is what lets the core stay free of any particular database or
//! object store.

pub mod context;
pub mod environment;
pub mod mongo;
pub mod token;

use layer2_core::store::StoreError;
use mongodb::Client;

pub use context::ChronoStorageContextStore;
pub use environment::MongoEnvironmentProfileStore;
pub use mongo::MongoSessionStore;

/// Everything this deployment keeps in MongoDB.
///
/// Returned together because they are opened together: a control plane that
/// started with one and not the other would serve half its surface, and the
/// failure would be discovered by a user rather than at startup.
pub struct MongoStores {
    /// Sessions and their event history.
    pub sessions: MongoSessionStore,
    /// Reusable environment profiles.
    pub profiles: MongoEnvironmentProfileStore,
}

/// Open every MongoDB-backed store over one connection.
///
/// One client rather than one per store: they address the same database, and a
/// second connection pool to it costs a control plane real connections for no
/// gain. Two ports over one connection is the point — the core still sees two
/// independent stores and could have either replaced without the other.
pub async fn connect(uri: &str, database: &str) -> Result<MongoStores, StoreError> {
    let client = Client::with_uri_str(uri)
        .await
        .map_err(|e| StoreError::Unavailable(format!("connect: {e}")))?;
    Ok(MongoStores {
        sessions: MongoSessionStore::open(&client, database).await?,
        profiles: MongoEnvironmentProfileStore::open(&client, database).await?,
    })
}
