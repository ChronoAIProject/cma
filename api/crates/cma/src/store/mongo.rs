//! One MongoDB connection shared by the retained durable adapters.

use std::{sync::Arc, time::Duration};

use layer2_core::StoreError;
use mongodb::{bson::doc, options::ClientOptions, Client};

use super::{
    AuthEncryptionKey, MongoActivityStore, MongoAuthStore, MongoOperationStore,
    MongoPlatformSettingsStore, MongoProfileStore, MongoSandboxCredentialStore, MongoSandboxStore,
    MongoTriggerDefinitionStore, MongoUserDirectoryStore,
};

/// Aggregate-neutral stores opened atomically at process startup.
pub struct MongoStores {
    pub(crate) account_deletions: Arc<super::account_deletions::MongoAccountDeletionStore>,
    pub(crate) request_limits: Arc<super::request_limits::MongoRequestLimits>,
    pub(crate) document_artifacts: Arc<crate::document_artifacts::MongoDocumentStore>,
    pub(crate) message_feedback: Arc<super::message_feedback::MongoMessageFeedbackStore>,
    /// Owner presentation palettes, kept apart from the strictly read Sandbox records.
    pub(crate) sandbox_colors: Arc<super::sandbox_colors::MongoSandboxColorStore>,
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
    /// Versioned Trigger definitions, placements, and immutable revisions.
    #[allow(dead_code)] // Opened and validated here; #824 composes the public routes.
    pub trigger_definitions: Arc<MongoTriggerDefinitionStore>,
    pub auth: Arc<MongoAuthStore>,
    pub(crate) codex_subscriptions: Arc<super::MongoSubscriptionStore>,
    pub activity: Arc<MongoActivityStore>,
    pub users: Arc<MongoUserDirectoryStore>,
    pub settings: Arc<MongoPlatformSettingsStore>,
    pub operations: Arc<MongoOperationStore>,
    pub(crate) native_warm_pools: Arc<super::native_warm_pools::MongoNativeWarmPools>,
    pub(crate) agent_lifecycle: Arc<super::MongoAgentLifecycleStore>,
    client: Client,
    database: String,
}

impl MongoStores {
    pub(crate) fn worker_observations(&self) -> super::worker_observations::Observations {
        super::worker_observations::Observations::new(self.client.database(&self.database))
    }

    pub(crate) fn with_writer_policy(
        mut self,
        policy: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    ) -> Self {
        Arc::make_mut(&mut self.sandbox_model).writer_policy = policy.clone();
        Arc::make_mut(&mut self.message_feedback).writer_policy = policy;
        self
    }

    #[cfg(test)]
    pub async fn connect(
        uri: &str,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
    ) -> Result<Self, StoreError> {
        Self::connect_with_authority(
            uri,
            database,
            deployment,
            auth_encryption_key,
            activity_retention,
            runtime,
            cma_service_auth::mongodb_authority::MongoAuthorityMode::Isolated,
        )
        .await
    }

    pub async fn connect_with_authority(
        uri: &str,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
        authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    ) -> Result<Self, StoreError> {
        let mut options = ClientOptions::parse(uri)
            .await
            .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
        super::mongo_latency::observe(&mut options);
        let client = Client::with_options(options)
            .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
        super::isolation::verify_with_authority(&client.database(database), authority).await?;
        Self::open(
            client,
            database,
            deployment,
            auth_encryption_key,
            activity_retention,
            Arc::new(super::checkpoint_storage::RetiredCheckpointStorage),
            runtime,
        )
        .await
    }

    pub(super) async fn open(
        client: Client,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        checkpoint_storage: Arc<dyn super::CheckpointBlobStore>,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
    ) -> Result<Self, StoreError> {
        // Legacy Profile warm records are retained only for cleanup and already
        // claimed runtime recovery. New admissions use the regular Sandbox lifecycle.
        let sandbox_model =
            MongoSandboxStore::open_with_storage(&client, database, checkpoint_storage)
                .await?
                .with_managed_runtime_version(runtime.0.into());
        let sandbox_model = Arc::new(sandbox_model);
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
        let agent_lifecycle =
            Arc::new(super::MongoAgentLifecycleStore::open(&client.database(database)).await?);
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
        let native_warm_pools = Arc::new(
            super::native_warm_pools::MongoNativeWarmPools::open(
                &client.database(database),
                deployment,
            )
            .await?,
        );
        let message_feedback = Arc::new(
            super::message_feedback::MongoMessageFeedbackStore::open(&client.database(database))
                .await?,
        );
        let sandbox_colors = Arc::new(
            super::sandbox_colors::MongoSandboxColorStore::open(&client.database(database)).await?,
        );
        super::schema::initialize_collections(
            &client.database(database),
            &[
                crate::document_artifacts::COLLECTION,
                crate::document_artifacts::QUOTA_COLLECTION,
            ],
        )
        .await?;
        let document_artifacts = Arc::new(crate::document_artifacts::MongoDocumentStore::new(
            client.database(database),
        ));
        document_artifacts
            .ensure_indexes()
            .await
            .map_err(|_| StoreError::Unavailable("initialize document registry".into()))?;
        let account_deletions = Arc::new(
            super::account_deletions::MongoAccountDeletionStore::open(&client.database(database))
                .await?,
        );
        let request_limits = Arc::new(
            super::request_limits::MongoRequestLimits::open(&client.database(database)).await?,
        );
        let stores = Self {
            account_deletions,
            request_limits,
            document_artifacts,
            message_feedback,
            sandbox_colors,
            native_warm_pools,
            event_workers,
            bot_setups,
            bot_setup_contexts,
            bots,
            sandbox_model,
            sandbox_profiles,
            sandbox_credentials,
            trigger_definitions,
            auth,
            codex_subscriptions,
            activity,
            users,
            settings,
            operations,
            agent_lifecycle,
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
