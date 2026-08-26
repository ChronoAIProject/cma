//! Durable storage, wired to concrete backends.
//!
//! The core defines the ports; these are the implementations. Keeping them in
//! the binary is what lets the core stay free of any particular database or
//! object store.

pub mod activity;
pub mod auth;
pub mod context;
pub mod directory;
mod durability;
mod encryption;
pub mod environment;
mod event_doc;
pub mod mongo;
pub mod operation;
pub mod settings;
pub mod tickets;
pub mod token;

use std::time::Duration;

use layer2_core::store::StoreError;
use mongodb::Client;

pub use activity::MongoActivityStore;
pub use auth::MongoAuthStore;
pub use context::ChronoStorageContextStore;
pub use directory::MongoUserDirectoryStore;
pub use environment::MongoEnvironmentProfileStore;
pub use mongo::MongoSessionStore;
pub use operation::MongoOperationStore;
pub use settings::MongoPlatformSettingsStore;
pub use tickets::MongoTerminalTicketStore;

pub(crate) use encryption::AuthEncryptionKey;

/// Everything this deployment keeps in MongoDB.
///
/// Returned together because they are opened together: a control plane that
/// started with one and not the others would serve part of its surface, and
/// the failure would be discovered by a user rather than at startup.
pub struct MongoStores {
    /// Sessions and their event history.
    pub sessions: MongoSessionStore,
    /// Reusable environment profiles.
    pub profiles: MongoEnvironmentProfileStore,
    /// What the service has been asked to do.
    pub activity: MongoActivityStore,
    /// Who has used the service.
    pub users: MongoUserDirectoryStore,
    /// Deployment-wide settings changed by administrators.
    pub settings: MongoPlatformSettingsStore,
    /// One-time grants used to authenticate terminal upgrades.
    pub tickets: MongoTerminalTicketStore,
    /// Browser, OAuth, provider-token, and session-credential authority.
    pub auth: MongoAuthStore,
    /// Subject-scoped operation claims and their replay outcomes.
    pub operations: MongoOperationStore,
}

/// Open every MongoDB-backed store over one connection.
///
/// One client rather than one per store: they address the same database, and a
/// second connection pool to it costs a control plane real connections for no
/// gain. Several ports over one connection is the point — the core still sees
/// independent stores and could have any of them replaced without the others.
///
/// `activity_retention` is how long a recorded action lives. It reaches the
/// database as an index, so changing it changes what is already stored rather
/// than only what is stored next.
pub async fn connect(
    uri: &str,
    database: &str,
    activity_retention: Duration,
    legacy_cutover_confirmed: bool,
    coordination_cutover_confirmed: bool,
    deployment: &str,
    auth_encryption_key: &AuthEncryptionKey,
) -> Result<MongoStores, StoreError> {
    let client = Client::with_uri_str(uri)
        .await
        .map_err(|e| StoreError::Unavailable(format!("connect: {e}")))?;
    Ok(MongoStores {
        sessions: MongoSessionStore::open(
            &client,
            database,
            legacy_cutover_confirmed,
            coordination_cutover_confirmed,
        )
        .await?,
        profiles: MongoEnvironmentProfileStore::open(&client, database).await?,
        activity: MongoActivityStore::open(&client, database, activity_retention).await?,
        users: MongoUserDirectoryStore::open(&client, database).await?,
        settings: MongoPlatformSettingsStore::open(&client, database).await?,
        tickets: MongoTerminalTicketStore::open(&client, database).await?,
        auth: MongoAuthStore::open(&client, database, deployment, auth_encryption_key).await?,
        operations: MongoOperationStore::open(&client, database).await?,
    })
}
