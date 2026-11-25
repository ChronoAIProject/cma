//! Durable ports for the control plane that does not admit execution work.
use super::*;
use crate::config::BackendFreeConfig;
use mongodb::Database;

#[path = "account_inventory.rs"]
mod inventory;

pub(crate) struct MongoAccountStores {
    pub auth: Arc<MongoAuthStore>,
    pub activity: Arc<MongoActivityStore>,
    pub users: Arc<MongoUserDirectoryStore>,
    pub settings: Arc<MongoPlatformSettingsStore>,
    pub registration: Arc<crate::store::MongoRegistrationCapacityStore>,
    pub registration_policy: Arc<dyn crate::auth::registration::RegistrationPolicy>,
    pub accounts: Arc<crate::store::account_deletions::MongoAccountDeletionStore>,
    pub request_limits: Arc<crate::store::request_limits::MongoRequestLimits>,
    pub database: Database,
}

impl MongoAccountStores {
    pub(crate) async fn connect(config: &BackendFreeConfig) -> Result<Arc<Self>, StoreError> {
        let database_config = &config.database;
        let name = &database_config.mongodb_database;
        let options = connection_options(&database_config.mongodb_uri, name).await?;
        let client = Client::with_options(options)
            .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
        let database = client.database(name);
        crate::store::isolation::replicas::verify(
            &database,
            database_config.mongodb_authority,
            false,
        )
        .await?;
        let initial_settings = crate::store::settings_defaults::InitialSettings::new(
            config.visitor_two_sandbox_defaults,
        );
        let mut schema = crate::store::schema::StartupObserver::new(config.startup_schema);
        Ok(Arc::new(Self {
            auth: Arc::new(
                MongoAuthStore::open(
                    &client,
                    name,
                    &database_config.deployment_id,
                    &config.encryption_key,
                )
                .await?,
            ),
            activity: Arc::new(
                MongoActivityStore::open_with_schema(
                    &client,
                    name,
                    config.activity_retention,
                    &mut schema,
                )
                .await?,
            ),
            users: Arc::new(MongoUserDirectoryStore::open(&client, name).await?),
            settings: Arc::new(
                MongoPlatformSettingsStore::open_with_defaults(&client, name, initial_settings)
                    .await?,
            ),
            registration: Arc::new(
                crate::store::MongoRegistrationCapacityStore::open_with_defaults(
                    &client,
                    name,
                    config.admission_ceilings_writes,
                    initial_settings,
                )
                .await?,
            ),
            registration_policy: Arc::new(crate::store::budget::RegistrationReader::new(
                &database,
                config.platform_counter_shards,
            )),
            accounts: Arc::new(
                crate::store::account_deletions::MongoAccountDeletionStore::from_database(
                    &database,
                ),
            ),
            request_limits: Arc::new(
                crate::store::request_limits::MongoRequestLimits::open(&database).await?,
            ),
            database,
        }))
    }
}

#[async_trait::async_trait]
impl crate::health::ReadinessProbe for MongoAccountStores {
    async fn ping(&self) -> Result<(), StoreError> {
        self.database
            .run_command(doc! { "ping": 1 })
            .await
            .map_err(|error| StoreError::Unavailable(format!("ping MongoDB: {error}")))?;
        crate::store::billing_authority::require_legacy_compatible(&self.database).await
    }
}
