//! One MongoDB connection shared by the retained durable adapters.

use std::{sync::Arc, time::Duration};

use layer2_core::StoreError;
use mongodb::{bson::doc, Client};

use super::{
    AuthEncryptionKey, MongoActivityStore, MongoAuthStore, MongoCheckpointObjectStore,
    MongoOperationStore, MongoPlatformSettingsStore, MongoProfileStore,
    MongoSandboxCredentialStore, MongoSandboxStore, MongoTriggerDefinitionStore,
    MongoUserDirectoryStore,
};

/// Aggregate-neutral stores opened atomically at process startup.
pub struct MongoStores {
    pub(crate) bots: Arc<super::bots::MongoBotStore>,
    /// Sandbox -> Workspace -> Worktree -> Agent aggregates and logs.
    pub sandbox_model: Arc<MongoSandboxStore>,
    /// Profile drafts, immutable revisions, and ownership indexes.
    pub sandbox_profiles: Arc<MongoProfileStore>,
    /// Encrypted one-time key values awaiting runtime acknowledgement.
    pub(crate) sandbox_credentials: Arc<MongoSandboxCredentialStore>,
    /// Sandbox-confined whole-home checkpoint archive authority.
    pub checkpoint_objects: Arc<MongoCheckpointObjectStore>,
    /// Versioned Trigger definitions, placements, and immutable revisions.
    #[allow(dead_code)] // Opened and validated here; #824 composes the public routes.
    pub trigger_definitions: Arc<MongoTriggerDefinitionStore>,
    /// Durable bridge configuration; public connection/dispatch adapters compose separately.
    #[allow(dead_code)]
    pub bridge_definitions: Arc<super::MongoBridgeStore>,
    pub(crate) bridge_credentials: Arc<super::MongoBridgeCredentialStore>,
    /// Ingress is closed until selected-key custody and a pinned callback route compose.
    #[allow(dead_code)]
    pub(crate) bridge_callback_inbox: Arc<super::MongoCallbackInbox>,
    pub auth: Arc<MongoAuthStore>,
    pub(crate) codex_subscriptions: Arc<super::MongoSubscriptionStore>,
    pub activity: Arc<MongoActivityStore>,
    pub users: Arc<MongoUserDirectoryStore>,
    pub settings: Arc<MongoPlatformSettingsStore>,
    pub operations: Arc<MongoOperationStore>,
    client: Client,
    database: String,
}

impl MongoStores {
    pub async fn connect(
        uri: &str,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        checkpoint_storage: Arc<dyn super::CheckpointBlobStore>,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
    ) -> Result<Self, StoreError> {
        let client = Client::with_uri_str(uri)
            .await
            .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
        super::isolation::verify(&client.database(database)).await?;
        let mut sandbox_model =
            MongoSandboxStore::open_with_storage(&client, database, checkpoint_storage).await?;
        sandbox_model.warm_runtime_version = Some(runtime.0.to_owned());
        sandbox_model.warm_home_policy = runtime.1.cloned();
        let sandbox_model = Arc::new(sandbox_model);
        let checkpoint_objects = Arc::new(sandbox_model.checkpoint_objects.clone());
        let sandbox_profiles = Arc::new(MongoProfileStore::open(&client, database).await?);
        let sandbox_credentials = Arc::new(
            MongoSandboxCredentialStore::open(&client, database, deployment, auth_encryption_key)
                .await?,
        );
        let trigger_definitions = Arc::new(
            MongoTriggerDefinitionStore::open(&client, database, Arc::clone(&sandbox_profiles))
                .await?,
        );
        let bridge_definitions = Arc::new(
            super::MongoBridgeStore::open(&client, database, deployment, auth_encryption_key)
                .await?,
        );
        let bridge_credentials = bridge_definitions.credentials();
        let bridge_callback_inbox = Arc::new(
            super::MongoCallbackInbox::open(
                &bridge_definitions,
                database,
                deployment,
                auth_encryption_key,
            )
            .await?,
        );
        let auth = Arc::new(
            MongoAuthStore::open(&client, database, deployment, auth_encryption_key).await?,
        );
        let codex_subscriptions = Arc::new(
            super::MongoSubscriptionStore::open(&client, database, deployment, auth_encryption_key)
                .await?,
        );
        let activity =
            Arc::new(MongoActivityStore::open(&client, database, activity_retention).await?);
        let users = Arc::new(MongoUserDirectoryStore::open(&client, database).await?);
        let settings = Arc::new(MongoPlatformSettingsStore::open(&client, database).await?);
        let operations = Arc::new(MongoOperationStore::open(&client, database).await?);
        let bots = Arc::new(
            super::bots::MongoBotStore::open(&client, database)
                .await
                .map_err(|_| StoreError::Unavailable("open bot store".into()))?,
        );
        let stores = Self {
            bots,
            sandbox_model,
            sandbox_profiles,
            sandbox_credentials,
            checkpoint_objects,
            trigger_definitions,
            bridge_definitions,
            bridge_credentials,
            bridge_callback_inbox,
            auth,
            codex_subscriptions,
            activity,
            users,
            settings,
            operations,
            client,
            database: database.to_owned(),
        };
        stores.ping().await?;
        Ok(stores)
    }

    pub async fn ping(&self) -> Result<(), StoreError> {
        self.client
            .database(&self.database)
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|error| StoreError::Unavailable(format!("ping MongoDB: {error}")))?;
        Ok(())
    }
}
