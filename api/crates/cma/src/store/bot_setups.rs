//! Encrypted, actor-confined Bot setup plans with compare-and-swap revisions.
use super::{
    durability::majority_collection,
    encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope},
    sandbox_mongo::is_duplicate_key,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};
use layer2_core::{
    bot_setups::{BotSetup, BotSetupStore, ExternalActor},
    bots::BotError,
    StoreError,
};
use mongodb::{bson::doc, Client, Collection};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Document {
    #[serde(rename = "_id")]
    actor_key: String,
    document_schema_version: i64,
    revision: i64,
    payload: CipherEnvelope,
}
pub(crate) struct MongoBotSetupStore {
    records: Collection<Document>,
    cipher: AuthCipher,
}
impl MongoBotSetupStore {
    pub(crate) async fn open(
        client: &Client,
        database: &str,
        deployment: &str,
        key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &["bot_setups"]).await?;
        Ok(Self {
            records: majority_collection(&database, "bot_setups"),
            cipher: AuthCipher::new(deployment, key)?,
        })
    }
    fn document(&self, setup: &BotSetup) -> Result<Document, BotError> {
        let actor_key = setup.actor.key();
        let payload = serde_json::to_string(setup).map_err(|_| BotError::Unavailable)?;
        Ok(Document {
            payload: self
                .cipher
                .seal("bot-setup", actor_key.as_bytes(), "plan", &payload)
                .map_err(|_| BotError::Unavailable)?,
            actor_key,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            revision: i64::try_from(setup.revision).map_err(|_| BotError::Conflict)?,
        })
    }
    fn decode(&self, row: Document, actor: &ExternalActor) -> Result<BotSetup, BotError> {
        schema::require_current(row.document_schema_version).map_err(|_| BotError::Unavailable)?;
        if row.actor_key != actor.key() {
            return Err(BotError::Unavailable);
        }
        let payload = self
            .cipher
            .open("bot-setup", row.actor_key.as_bytes(), "plan", row.payload)
            .map_err(|_| BotError::Unavailable)?;
        let setup: BotSetup = serde_json::from_str(&payload).map_err(|_| BotError::Unavailable)?;
        if setup.actor != *actor || i64::try_from(setup.revision).ok() != Some(row.revision) {
            return Err(BotError::Unavailable);
        }
        Ok(setup)
    }
}
#[async_trait::async_trait]
impl BotSetupStore for MongoBotSetupStore {
    async fn get(&self, actor: &ExternalActor) -> Result<Option<BotSetup>, BotError> {
        self.records
            .find_one(doc! {"_id":actor.key()})
            .await
            .map_err(|_| BotError::Unavailable)?
            .map(|row| self.decode(row, actor))
            .transpose()
    }
    async fn insert(&self, setup: &BotSetup) -> Result<(), BotError> {
        match self.records.insert_one(self.document(setup)?).await {
            Ok(_) => Ok(()),
            Err(error) if is_duplicate_key(&error) => Err(BotError::Conflict),
            Err(_) => Err(BotError::Unavailable),
        }
    }
    async fn replace(&self, setup: &BotSetup, expected_revision: u64) -> Result<(), BotError> {
        if setup.revision != expected_revision.checked_add(1).ok_or(BotError::Conflict)? {
            return Err(BotError::Conflict);
        }
        let expected = i64::try_from(expected_revision).map_err(|_| BotError::Conflict)?;
        let result = self
            .records
            .replace_one(
                doc! {"_id":setup.actor.key(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,"revision":expected},
                self.document(setup)?,
            )
            .await
            .map_err(|_| BotError::Unavailable)?;
        if result.matched_count == 1 {
            Ok(())
        } else {
            Err(BotError::Conflict)
        }
    }
}

#[cfg(test)]
#[path = "bot_setups_mongo_integration_tests.rs"]
mod mongo_integration_tests;
