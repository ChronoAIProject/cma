//! One MongoDB connection shared by the retained durable adapters.

use std::{sync::Arc, time::Duration};

use layer2_core::StoreError;
use mongodb::{bson::doc, Client};

use super::{
    AuthEncryptionKey, MongoActivityStore, MongoAuthStore, MongoCheckpointObjectStore,
    MongoGatewayRecordStore, MongoOperationStore, MongoPlatformSettingsStore, MongoProfileStore,
    MongoSandboxCredentialStore, MongoSandboxStore, MongoTriggerDefinitionStore,
    MongoUserDirectoryStore,
};

/// Aggregate-neutral stores opened atomically at process startup.
pub struct MongoStores {
    pub(crate) event_workers: Arc<super::event_workers::MongoEventWorkerProfileStore>,
    pub(crate) bot_setups: Arc<super::bot_setups::MongoBotSetupStore>,
    pub(crate) bot_setup_contexts: Arc<super::bot_setup_contexts::MongoBotSetupContextStore>,
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
    pub auth: Arc<MongoAuthStore>,
    pub(crate) codex_subscriptions: Arc<super::MongoSubscriptionStore>,
    pub activity: Arc<MongoActivityStore>,
    pub users: Arc<MongoUserDirectoryStore>,
    pub settings: Arc<MongoPlatformSettingsStore>,
    pub operations: Arc<MongoOperationStore>,
    pub gateway_records: Arc<MongoGatewayRecordStore>,
    pub(crate) gateway_connections: Arc<MongoGatewayRecordStore>,
    pub(crate) gateway_interactions: Arc<super::MongoGatewayInteractionStore>,
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
        let gateway_records = Arc::new(MongoGatewayRecordStore::open(&client, database).await?);
        let gateway_connections =
            Arc::new(MongoGatewayRecordStore::open_connections(&client, database).await?);
        let gateway_interactions =
            Arc::new(super::MongoGatewayInteractionStore::open(&client, database).await?);
        let bot_setups = Arc::new(
            super::bot_setups::MongoBotSetupStore::open(
                &client,
                database,
                deployment,
                auth_encryption_key,
            )
            .await?,
        );
        let bots = Arc::new(
            super::bots::MongoBotStore::open(&client, database)
                .await
                .map_err(|_| StoreError::Unavailable("open bot store".into()))?,
        );
        let bot_setup_contexts = Arc::new(
            super::bot_setup_contexts::MongoBotSetupContextStore::open(
                &client,
                database,
                deployment,
                auth_encryption_key,
            )
            .await?,
        );
        let event_workers = Arc::new(
            super::event_workers::MongoEventWorkerProfileStore::open(&client, database)
                .await
                .map_err(|_| StoreError::Unavailable("open event worker profiles".into()))?,
        );
        let stores = Self {
            event_workers,
            bot_setups,
            bot_setup_contexts,
            bots,
            sandbox_model,
            sandbox_profiles,
            sandbox_credentials,
            checkpoint_objects,
            trigger_definitions,
            auth,
            codex_subscriptions,
            activity,
            users,
            settings,
            operations,
            gateway_records,
            gateway_connections,
            gateway_interactions,
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
