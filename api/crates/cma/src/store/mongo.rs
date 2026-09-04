//! One MongoDB connection shared by the retained durable adapters.

use std::{sync::Arc, time::Duration};

use layer2_core::StoreError;
use mongodb::{bson::doc, Client};

use super::{
    AuthEncryptionKey, MongoActivityStore, MongoAuthStore, MongoOperationStore,
    MongoPlatformSettingsStore, MongoUserDirectoryStore,
};

/// Aggregate-neutral stores opened atomically at process startup.
pub struct MongoStores {
    pub auth: Arc<MongoAuthStore>,
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
    ) -> Result<Self, StoreError> {
        let client = Client::with_uri_str(uri)
            .await
            .map_err(|error| StoreError::Unavailable(format!("connect to MongoDB: {error}")))?;
        let auth = Arc::new(
            MongoAuthStore::open(&client, database, deployment, auth_encryption_key).await?,
        );
        let activity =
            Arc::new(MongoActivityStore::open(&client, database, activity_retention).await?);
        let users = Arc::new(MongoUserDirectoryStore::open(&client, database).await?);
        let settings = Arc::new(MongoPlatformSettingsStore::open(&client, database).await?);
        let operations = Arc::new(MongoOperationStore::open(&client, database).await?);
        let stores = Self {
            auth,
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
