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
pub mod skill_packs;
pub mod sponsorship;
mod sponsorship_encryption;
pub mod tickets;
pub mod token;
pub mod trigger;
pub mod trigger_workspace;

use std::time::Duration;

use async_trait::async_trait;
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
pub use skill_packs::ChronoStorageSkillPackStore;
pub use sponsorship::MongoSponsorshipStore;
pub use tickets::MongoTerminalTicketStore;
pub use trigger::MongoTriggerStore;
pub use trigger_workspace::ChronoStorageTriggerWorkspaceStore;

pub(crate) use encryption::AuthEncryptionKey;
pub(crate) use sponsorship_encryption::{SponsorshipEncryptionKey, SponsorshipKeyring};

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
    /// Publisher revisions and contributor Launch aggregates.
    pub triggers: MongoTriggerStore,
}

#[async_trait]
trait LegacyCutoverClient: Sized {
    async fn stage_legacy_cutover(&self, database: &str) -> Result<(), StoreError>;
    async fn shutdown(self);
}

struct MongoLegacyCutoverClient(Client);

#[async_trait]
impl LegacyCutoverClient for MongoLegacyCutoverClient {
    async fn stage_legacy_cutover(&self, database: &str) -> Result<(), StoreError> {
        MongoSessionStore::stage_legacy_cutover(&self.0, database).await
    }

    async fn shutdown(self) {
        self.0.shutdown().await;
    }
}

async fn run_legacy_cutover_with_client(
    client: impl LegacyCutoverClient,
    database: &str,
) -> Result<(), StoreError> {
    let result = client.stage_legacy_cutover(database).await;
    client.shutdown().await;
    result
}

/// Run only the legacy session-namespace cutover, then close MongoDB completely.
pub async fn run_legacy_cutover_stage(uri: &str, database: &str) -> Result<(), StoreError> {
    let client = MongoLegacyCutoverClient(
        Client::with_uri_str(uri)
            .await
            .map_err(|e| StoreError::Unavailable(format!("connect: {e}")))?,
    );
    run_legacy_cutover_with_client(client, database).await
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
        triggers: MongoTriggerStore::open(&client, database)
            .await
            .map_err(|error| StoreError::Unavailable(format!("open trigger store: {error}")))?,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct ProbeLegacyCutoverClient {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_stage: bool,
    }

    #[async_trait]
    impl LegacyCutoverClient for ProbeLegacyCutoverClient {
        async fn stage_legacy_cutover(&self, database: &str) -> Result<(), StoreError> {
            assert_eq!(database, "stage_database");
            self.events.lock().unwrap().push("stage-complete");
            if self.fail_stage {
                Err(StoreError::Unavailable("deliberate stage failure".into()))
            } else {
                Ok(())
            }
        }

        async fn shutdown(self) {
            self.events.lock().unwrap().push("client-shutdown");
        }
    }

    #[tokio::test]
    async fn legacy_cutover_closes_the_client_after_success() {
        let events = Arc::new(Mutex::new(Vec::new()));

        run_legacy_cutover_with_client(
            ProbeLegacyCutoverClient {
                events: Arc::clone(&events),
                fail_stage: false,
            },
            "stage_database",
        )
        .await
        .expect("the staged migration succeeds");

        assert_eq!(
            *events.lock().unwrap(),
            ["stage-complete", "client-shutdown"]
        );
    }

    #[tokio::test]
    async fn legacy_cutover_closes_the_client_after_a_stage_error() {
        let events = Arc::new(Mutex::new(Vec::new()));

        let error = run_legacy_cutover_with_client(
            ProbeLegacyCutoverClient {
                events: Arc::clone(&events),
                fail_stage: true,
            },
            "stage_database",
        )
        .await
        .expect_err("the deliberate stage failure is returned after shutdown");

        assert!(matches!(error, StoreError::Unavailable(_)));
        assert_eq!(
            *events.lock().unwrap(),
            ["stage-complete", "client-shutdown"]
        );
    }
}
