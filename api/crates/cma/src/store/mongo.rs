//! One MongoDB connection shared by the retained durable adapters.

use std::{sync::Arc, time::Duration};

use layer2_core::StoreError;
use mongodb::{bson::doc, Client};

use super::{
    AuthEncryptionKey, MongoActivityStore, MongoAdmissionControlStore, MongoAuthStore,
    MongoOperationStore, MongoPlatformSettingsStore, MongoProfileStore,
    MongoSandboxCredentialStore, MongoSandboxStore, MongoTriggerDefinitionStore,
    MongoUserDirectoryStore,
};

mod creation_outcomes;
mod options;
use options::connection_options;
#[cfg(test)]
pub(super) use options::STARTUP_COMMANDS;

#[path = "account_mongo.rs"]
mod account;
pub(crate) use account::MongoAccountStores;

pub(crate) struct MongoConnectionPolicy {
    pub authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    pub agent_manager_replicas: bool,
    pub startup_schema: crate::config::StartupSchemaMode,
    pub shared_observation: bool,
    pub initial_settings: super::settings_defaults::InitialSettings,
    /// The process's single released-default authority; fixtures may omit it.
    pub released: Option<Arc<dyn layer3_adapter::ReleasedRuntimeSource>>,
    pub runtime_pointer_required: bool,
}

/// Aggregate-neutral stores opened atomically at process startup.
pub struct MongoStores {
    /// The one released-default authority for new work in this process.
    pub(crate) released_runtime: Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
    pub(crate) runtime_pointer: Option<Arc<super::released_runtimes::StoreReleasedRuntime>>,
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
    /// Legal-document acceptances on the same retained directory rows.
    pub(crate) legal_consents: Arc<super::legal_consents::MongoLegalConsentStore>,
    pub settings: Arc<MongoPlatformSettingsStore>,
    pub admission_control: Arc<MongoAdmissionControlStore>,
    pub admission_ceilings: Arc<super::MongoAdmissionCeilingsStore>,
    pub(crate) registration_capacity: Arc<super::MongoRegistrationCapacityStore>,
    pub operations: Arc<MongoOperationStore>,
    pub(crate) native_warm_pools: Arc<super::native_warm_pools::MongoNativeWarmPools>,
    pub(crate) agent_lifecycle: Arc<super::MongoAgentLifecycleStore>,
    client: Client,
    database: String,
    initial_settings: super::settings_defaults::InitialSettings,
}

impl MongoStores {
    pub(crate) async fn configure_admission_ceilings(
        mut self,
        deployment_limit: u16,
        writes_enabled: bool,
        pending_home_reconciliation: bool,
    ) -> Result<Self, StoreError> {
        if writes_enabled {
            self.registration_capacity = Arc::new(
                super::MongoRegistrationCapacityStore::open_with_defaults(
                    &self.client,
                    &self.database,
                    true,
                    self.initial_settings,
                )
                .await?,
            );
        }
        self.admission_ceilings = Arc::new(
            super::MongoAdmissionCeilingsStore::open_with_defaults(
                &self.client,
                &self.database,
                deployment_limit,
                self.initial_settings,
            )
            .await?
            .with_activation(self.registration_capacity.activation_observation())
            .with_pending_home_reconciliation(pending_home_reconciliation),
        );
        Ok(self)
    }

    pub(crate) fn with_empty_discovery(mut self, enabled: bool) -> Self {
        let policy = super::empty_discovery::Policy::new(enabled);
        Arc::make_mut(&mut self.sandbox_model).empty_discovery = policy.clone();
        Arc::make_mut(&mut self.trigger_definitions).empty_discovery = policy;
        self
    }

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

    #[cfg(test)]
    pub async fn connect_with_authority(
        uri: &str,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
        authority: cma_service_auth::mongodb_authority::MongoAuthorityMode,
    ) -> Result<Self, StoreError> {
        Self::connect_with_policy(
            uri,
            database,
            deployment,
            auth_encryption_key,
            activity_retention,
            runtime,
            MongoConnectionPolicy {
                authority,
                agent_manager_replicas: false,
                startup_schema: Default::default(),
                shared_observation: false,
                initial_settings: Default::default(),
                released: None,
                runtime_pointer_required: false,
            },
        )
        .await
    }

    pub(crate) async fn connect_with_policy(
        uri: &str,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
        policy: MongoConnectionPolicy,
    ) -> Result<Self, StoreError> {
        let options = connection_options(uri, database).await?;
        let client = Client::with_options(options)
            .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
        super::isolation::replicas::verify(
            &client.database(database),
            policy.authority,
            policy.agent_manager_replicas,
        )
        .await?;
        if policy.shared_observation {
            super::isolation::verify_observation(&client.database(database)).await?;
        }
        Self::open_configured(
            client,
            database,
            deployment,
            auth_encryption_key,
            activity_retention,
            StoreStartup {
                checkpoint_storage: Arc::new(super::checkpoint_storage::RetiredCheckpointStorage),
                runtime,
                schema: super::schema::StartupObserver::new(policy.startup_schema),
                initial_settings: policy.initial_settings,
                released: policy.released,
                runtime_pointer_required: policy.runtime_pointer_required,
            },
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn open(
        client: Client,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        checkpoint_storage: Arc<dyn super::CheckpointBlobStore>,
        runtime: (&str, Option<&layer3_adapter::HomeVolumePolicy>),
    ) -> Result<Self, StoreError> {
        Self::open_configured(
            client,
            database,
            deployment,
            auth_encryption_key,
            activity_retention,
            StoreStartup {
                checkpoint_storage,
                runtime,
                schema: super::schema::StartupObserver::new(Default::default()),
                initial_settings: Default::default(),
                released: None,
                runtime_pointer_required: false,
            },
        )
        .await
    }

    async fn open_configured(
        client: Client,
        database: &str,
        deployment: &str,
        auth_encryption_key: &AuthEncryptionKey,
        activity_retention: Duration,
        mut startup: StoreStartup<'_>,
    ) -> Result<Self, StoreError> {
        // Legacy Profile warm records are retained only for cleanup and already
        // claimed runtime recovery. New admissions use the regular Sandbox lifecycle.
        let sandbox_model = MongoSandboxStore::open_with_schema(
            &client,
            database,
            startup.checkpoint_storage,
            &mut startup.schema,
            startup.initial_settings,
        )
        .await?
        .with_managed_runtime_version(startup.runtime.0.into());
        let released_runtime = startup.released.clone().unwrap_or_else(|| {
            Arc::new(layer3_adapter::PinnedReleasedRuntime::new(
                startup.runtime.0,
                None,
            ))
        });
        let runtime_pointer = match &startup.released {
            Some(_) => super::released_runtimes::StoreReleasedRuntime::open(
                &client.database(database),
                deployment,
                released_runtime.snapshot(),
                startup.runtime_pointer_required,
            )
            .await?
            .map(Arc::new),
            None => None,
        };
        let (released_runtime, mut sandbox_model) = match &runtime_pointer {
            Some(source) => {
                let mut sandbox_model = sandbox_model;
                sandbox_model.released_lanes = Some(source.lanes.clone());
                (
                    source.clone() as Arc<dyn layer3_adapter::ReleasedRuntimeSource>,
                    sandbox_model,
                )
            }
            None => (released_runtime, sandbox_model),
        };
        sandbox_model = sandbox_model.with_released_runtime(released_runtime.clone());
        let sandbox_model = Arc::new(sandbox_model);
        let sandbox_profiles = Arc::new(MongoProfileStore::open(&client, database).await?);
        let sandbox_credentials = Arc::new(
            MongoSandboxCredentialStore::open(&client, database, deployment, auth_encryption_key)
                .await?,
        );
        let trigger_definitions = Arc::new(
            MongoTriggerDefinitionStore::open(&client, database, Arc::clone(&sandbox_profiles))
                .await?
                .with_initial_settings(startup.initial_settings),
        );
        let auth = Arc::new(
            MongoAuthStore::open(&client, database, deployment, auth_encryption_key).await?,
        );
        let codex_subscriptions = Arc::new(
            super::MongoSubscriptionStore::open(&client, database, deployment, auth_encryption_key)
                .await?,
        );
        let activity = Arc::new(
            MongoActivityStore::open_with_schema(
                &client,
                database,
                activity_retention,
                &mut startup.schema,
            )
            .await?,
        );
        let users = Arc::new(MongoUserDirectoryStore::open(&client, database).await?);
        let legal_consents = Arc::new(super::legal_consents::MongoLegalConsentStore::new(
            &client.database(database),
        ));
        let settings = Arc::new(
            MongoPlatformSettingsStore::open_with_defaults(
                &client,
                database,
                startup.initial_settings,
            )
            .await?,
        );
        let admission_control = Arc::new(
            MongoAdmissionControlStore::open_with_defaults(
                &client,
                database,
                startup.initial_settings,
            )
            .await?,
        );
        let registration_capacity = Arc::new(
            super::MongoRegistrationCapacityStore::open_with_defaults(
                &client,
                database,
                false,
                startup.initial_settings,
            )
            .await?,
        );
        let admission_ceilings = Arc::new(
            super::MongoAdmissionCeilingsStore::open_with_defaults(
                &client,
                database,
                0,
                startup.initial_settings,
            )
            .await?
            .with_activation(registration_capacity.activation_observation()),
        );
        let operations = Arc::new(MongoOperationStore::open(&client, database).await?);
        let mut agent_lifecycle =
            super::MongoAgentLifecycleStore::open(&client.database(database)).await?;
        agent_lifecycle.released_lanes =
            runtime_pointer.as_ref().map(|source| source.lanes.clone());
        let agent_lifecycle = Arc::new(agent_lifecycle);
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
        let mut native_warm_pools = super::native_warm_pools::MongoNativeWarmPools::open(
            &client.database(database),
            deployment,
        )
        .await?;
        native_warm_pools.released_lanes =
            runtime_pointer.as_ref().map(|source| source.lanes.clone());
        let native_warm_pools = Arc::new(native_warm_pools);
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
            super::account_deletions::MongoAccountDeletionStore::from_database(
                &client.database(database),
            ),
        );
        let request_limits = Arc::new(
            super::request_limits::MongoRequestLimits::open(&client.database(database)).await?,
        );
        let stores = Self {
            released_runtime,
            runtime_pointer,
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
            legal_consents,
            settings,
            admission_control,
            admission_ceilings,
            registration_capacity,
            operations,
            agent_lifecycle,
            client,
            database: database.to_owned(),
            initial_settings: startup.initial_settings,
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
        super::billing_authority::require_legacy_compatible(&self.client.database(&self.database))
            .await
    }
}

#[cfg(test)]
#[path = "mongo_options_tests.rs"]
mod options_tests;

struct StoreStartup<'a> {
    checkpoint_storage: Arc<dyn super::CheckpointBlobStore>,
    runtime: (&'a str, Option<&'a layer3_adapter::HomeVolumePolicy>),
    schema: super::schema::StartupObserver,
    initial_settings: super::settings_defaults::InitialSettings,
    released: Option<Arc<dyn layer3_adapter::ReleasedRuntimeSource>>,
    runtime_pointer_required: bool,
}
