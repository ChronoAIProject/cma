//! Encrypted, immutable Father activities retained independently of model commands.
use super::{
    durability::majority_collection,
    encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope},
    sandbox_mongo::is_duplicate_key,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};
use layer2_core::{bots::BotError, StoreError};
use mongodb::{
    bson::{doc, DateTime},
    options::IndexOptions,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
struct Document {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    expires_at: DateTime,
    payload: CipherEnvelope,
}
pub(crate) struct MongoBotSetupContextStore {
    records: Collection<Document>,
    cipher: AuthCipher,
}
impl MongoBotSetupContextStore {
    pub(crate) async fn open(
        client: &Client,
        database: &str,
        deployment: &str,
        key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, &["bot_setup_contexts"]).await?;
        let records = majority_collection(&database, "bot_setup_contexts");
        records
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"expires_at": 1})
                    .options(
                        IndexOptions::builder()
                            .expire_after(std::time::Duration::ZERO)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|_| StoreError::Unavailable("setup context index unavailable".into()))?;
        Ok(Self {
            records,
            cipher: AuthCipher::new(deployment, key)?,
        })
    }

    pub(crate) async fn retain(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
        expires_at: i64,
        context: &Value,
    ) -> Result<(), BotError> {
        let id = context_id(binding, conversation, event)?;
        let expires_at =
            DateTime::from_millis(expires_at.checked_mul(1000).ok_or(BotError::Forbidden)?);
        if expires_at <= DateTime::now() {
            return Err(BotError::Forbidden);
        }
        let payload = serde_json::to_string(
            &serde_json::json!({"expires_at":expires_at.timestamp_millis(),"context":context}),
        )
        .map_err(|_| BotError::Unavailable)?;
        let row = Document {
            payload: self
                .cipher
                .seal("bot-setup-context", id.as_bytes(), "activity", &payload)
                .map_err(|_| BotError::Unavailable)?,
            id,
            expires_at,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
        };
        match self.records.insert_one(&row).await {
            Ok(_) => Ok(()),
            Err(error) if is_duplicate_key(&error) => {
                let retained = self
                    .get(binding, conversation, event)
                    .await?
                    .ok_or(BotError::Conflict)?;
                // Redelivery can carry a newly sealed reply reference, but the
                // authenticated event identity and full original activity cannot change.
                if retained["channel_id"] != context["channel_id"]
                    || retained["conversation_id"] != context["conversation_id"]
                    || retained["activity"] != context["activity"]
                {
                    return Err(BotError::Conflict);
                }
                Ok(())
            }
            Err(_) => Err(BotError::Unavailable),
        }
    }

    pub(crate) async fn get(
        &self,
        binding: &str,
        conversation: &str,
        event: &str,
    ) -> Result<Option<Value>, BotError> {
        let id = context_id(binding, conversation, event)?;
        let Some(row) = self
            .records
            .find_one(doc! {"_id": &id,"expires_at":{"$gt":DateTime::now()}})
            .await
            .map_err(|_| BotError::Unavailable)?
        else {
            return Ok(None);
        };
        schema::require_current(row.document_schema_version).map_err(|_| BotError::Unavailable)?;
        if row.id != id {
            return Err(BotError::Unavailable);
        }
        let plaintext = self
            .cipher
            .open("bot-setup-context", id.as_bytes(), "activity", row.payload)
            .map_err(|_| BotError::Unavailable)?;
        let retained: Value =
            serde_json::from_str(&plaintext).map_err(|_| BotError::Unavailable)?;
        if retained["expires_at"].as_i64() != Some(row.expires_at.timestamp_millis()) {
            return Err(BotError::Unavailable);
        }
        let context = retained["context"].clone();
        if context["conversation_id"].as_str() != Some(conversation)
            || context["activity"]["event_id"].as_str() != Some(event)
        {
            return Err(BotError::Unavailable);
        }
        Ok(Some(context))
    }
}

fn context_id(binding: &str, conversation: &str, event: &str) -> Result<String, BotError> {
    layer2_core::profiles::content_digest(&(binding, conversation, event))
        .map_err(|_| BotError::Unavailable)
}
