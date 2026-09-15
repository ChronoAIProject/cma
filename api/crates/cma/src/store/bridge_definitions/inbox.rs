//! Majority-durable callback acceptance. Configuration state, deduplication,
//! admission limits, normalized inbox and encrypted reply context commit once.
use std::{sync::Arc, time::Duration};

use layer2_core::bridge::ingress::{
    CallbackError, CallbackInboxStore, InboxAdmission, InboxReceipt, InboxWrite, StoredInbox,
};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::IndexOptions,
    ClientSession, Collection, IndexModel,
};

use crate::store::{
    durability::majority_collection,
    encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope},
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, read_record, record_bson,
        start_transaction, CommitOutcome, DOCUMENT_SCHEMA_VERSION,
    },
    schema::initialize_collections,
};

use super::{decode_bridge, MongoBridgeStore};

#[derive(Clone)]
pub(crate) struct MongoCallbackInbox {
    bridges: MongoBridgeStore,
    inbox: Collection<Document>,
    replies: Collection<Document>,
    rates: Collection<Document>,
    cipher: Arc<AuthCipher>,
}

impl MongoCallbackInbox {
    pub(crate) async fn open(
        bridges: &MongoBridgeStore,
        name: &str,
        deployment: &str,
        key: &AuthEncryptionKey,
    ) -> Result<Self, layer2_core::StoreError> {
        let database = bridges.client.database(name);
        initialize_collections(
            &database,
            &[
                "bridge_callback_inbox",
                "bridge_callback_replies",
                "bridge_callback_rates",
            ],
        )
        .await?;
        let store = Self {
            bridges: bridges.clone(),
            inbox: majority_collection(&database, "bridge_callback_inbox"),
            replies: majority_collection(&database, "bridge_callback_replies"),
            rates: majority_collection(&database, "bridge_callback_rates"),
            cipher: Arc::new(AuthCipher::new(deployment, key)?),
        };
        for collection in [&store.inbox, &store.replies, &store.rates] {
            collection
                .create_index(
                    IndexModel::builder()
                        .keys(doc! {"expires_at":1_i32})
                        .options(IndexOptions::builder().expire_after(Duration::ZERO).build())
                        .build(),
                )
                .await
                .map_err(|_| {
                    layer2_core::StoreError::Unavailable("create callback retention index".into())
                })?;
        }
        store
            .inbox
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"bridge_id":1_i32,"admission":1_i32,"expires_at":1_i32})
                    .build(),
            )
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable("create callback admission index".into())
            })?;
        Ok(store)
    }

    async fn transaction(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<InboxReceipt, MutationError> {
        write.require_current(write.bridge())?;
        let current = self
            .bridges
            .definitions
            .find_one(doc! {"_id":write.bridge().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(CallbackError::BindingUnavailable)?;
        let current = decode_bridge(&current).map_err(|_| CallbackError::BindingUnavailable)?;
        write.require_current(&current)?;
        self.bridges
            .credentials
            .require_binding(&current, write.custody_pin(), &mut *session)
            .await
            .map_err(|_| CallbackError::BindingUnavailable)?;
        if let Some(document) = self
            .inbox
            .find_one(doc! {"_id": &write.record().id})
            .session(&mut *session)
            .await?
        {
            return Ok(write.match_replay(&decode(&document)?)?);
        }
        let record = write.record();
        let config = &current
            .current()
            .map_err(|_| CallbackError::BindingUnavailable)?
            .config;
        if record.admission == InboxAdmission::Accepted {
            let queued = self
                .inbox
                .count_documents(
                    doc! {"bridge_id":current.id.as_str(),"admission":"accepted",
                    "expires_at":{"$gt":DateTime::from_millis(record.accepted_at * 1000)}},
                )
                .session(&mut *session)
                .await?;
            if queued >= u64::from(config.limits.max_queued_messages) {
                return Err(CallbackError::Capacity.into());
            }
        }
        let minute = record.accepted_at / 60;
        let rate_id = format!("{}:{minute}", current.id.as_str());
        let used = self
            .rates
            .find_one(doc! {"_id":&rate_id})
            .session(&mut *session)
            .await?
            .map(|d| d.get_i64("count").map_err(|_| CallbackError::Unavailable))
            .transpose()?
            .unwrap_or(0);
        if used < 0 || used >= i64::from(config.limits.max_messages_per_minute) {
            return Err(CallbackError::Capacity.into());
        }
        // A real write to the definition document serializes concurrent admissions
        // and races with disable, emergency stop, rotation or retirement. This
        // internal fence does not alter the user-visible configuration revision.
        let changed = self
            .bridges
            .definitions
            .update_one(
                doc! {"_id":current.id.as_str(),"revision":current.revision as i64},
                doc! {"$inc":{"inbox_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(CallbackError::BindingUnavailable.into());
        }
        self.rates.update_one(doc!{"_id":&rate_id},doc!{"$inc":{"count":1_i64},"$setOnInsert":{"document_schema_version":DOCUMENT_SCHEMA_VERSION,"expires_at":DateTime::from_millis((minute+2)*60000)}})
            .upsert(true).session(&mut *session).await?;
        self.inbox.insert_one(doc!{"_id":&record.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "bridge_id":record.pin.bridge_id.as_str(),"admission":admission(&record.admission),
            "expires_at":DateTime::from_millis(record.expires_at*1000),"record":record_bson(record).map_err(|_|CallbackError::Unavailable)?})
            .session(&mut *session).await?;
        if let Some(expires) = record.reply_expires_at {
            let mut document = doc! {"_id":&record.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
            "expires_at":DateTime::from_millis(expires*1000)};
            for (field, value) in [
                ("reply_token", write.reply_token()),
                ("thread_context", write.thread_context()),
            ] {
                if let Some(value) = value {
                    let envelope = self
                        .cipher
                        .seal("bridge_callback_reply", record.id.as_bytes(), field, value)
                        .map_err(|_| CallbackError::Unavailable)?;
                    document.insert(
                        field,
                        record_bson(&envelope).map_err(|_| CallbackError::Unavailable)?,
                    );
                }
            }
            self.replies
                .insert_one(document)
                .session(&mut *session)
                .await?;
        }
        write.require_current(&current)?;
        Ok(record.receipt())
    }

    async fn replay(&self, write: &InboxWrite) -> Result<Option<InboxReceipt>, CallbackError> {
        write.require_current(write.bridge())?;
        self.inbox
            .find_one(doc! {"_id": &write.record().id})
            .await
            .map_err(|_| CallbackError::Unavailable)?
            .map(|document| write.match_replay(&decode(&document)?))
            .transpose()
    }

    /// After an uncertain commit, failure to observe a matching durable receipt
    /// cannot establish rejection. Keep authentication checks intact, but do not
    /// report proof expiry during reconciliation as definite non-acceptance.
    pub(super) async fn reconcile_commit(
        &self,
        write: &InboxWrite,
    ) -> Result<InboxReceipt, CallbackError> {
        match self.replay(write).await {
            Ok(Some(receipt)) => Ok(receipt),
            Err(CallbackError::ReplayConflict) => Err(CallbackError::ReplayConflict),
            Ok(None) | Err(_) => Err(CallbackError::Unavailable),
        }
    }

    /// Ciphertext deletion is best-effort TTL cleanup; logical expiry is checked
    /// before opening. This is internal custody, never a public read operation.
    #[allow(dead_code)] // Deferred reply workers need independent live effect authority.
    pub(crate) async fn reply_context(
        &self,
        id: &str,
    ) -> Result<Option<crate::auth::persistence::SecretValue>, CallbackError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| CallbackError::Unavailable)?
            .as_secs()
            .min(i64::MAX as u64) as i64;
        let Some(document) = self.replies.find_one(doc!{"_id":id,"expires_at":{"$gt":DateTime::from_millis(now.saturating_mul(1000))}})
            .await.map_err(|_|CallbackError::Unavailable)? else { return Ok(None); };
        let Some(envelope) = document.get("reply_token") else {
            return Ok(None);
        };
        let envelope: CipherEnvelope = mongodb::bson::deserialize_from_bson(envelope.clone())
            .map_err(|_| CallbackError::Unavailable)?;
        let token = self
            .cipher
            .open(
                "bridge_callback_reply",
                id.as_bytes(),
                "reply_token",
                envelope,
            )
            .map_err(|_| CallbackError::Unavailable)?;
        Ok(Some(crate::auth::persistence::SecretValue::new(token)))
    }
}

#[async_trait::async_trait]
impl CallbackInboxStore for MongoCallbackInbox {
    async fn accept(&self, write: InboxWrite) -> Result<InboxReceipt, CallbackError> {
        for _ in 0..8 {
            write.require_current(write.bridge())?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            let receipt = match self.transaction(&write, &mut session).await {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if is_duplicate_key(&error) || is_transient(&error) =>
                        {
                            continue
                        }
                        MutationError::Mongo(_) => return Err(CallbackError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return self.reconcile_commit(&write).await
                }
            }
        }
        Err(CallbackError::Unavailable)
    }
}

fn admission(value: &InboxAdmission) -> &'static str {
    match value {
        InboxAdmission::Accepted => "accepted",
        InboxAdmission::SenderDenied => "sender_denied",
        InboxAdmission::PartitionDenied { .. } => "partition_denied",
    }
}
fn decode(document: &Document) -> Result<StoredInbox, CallbackError> {
    let record: StoredInbox = read_record(document).map_err(|_| CallbackError::Unavailable)?;
    if document.get_str("_id").ok() != Some(record.id.as_str())
        || document.get_str("bridge_id").ok() != Some(record.pin.bridge_id.as_str())
        || document.get_str("admission").ok() != Some(admission(&record.admission))
        || document.get_datetime("expires_at").ok()
            != Some(&DateTime::from_millis(
                record.expires_at.saturating_mul(1000),
            ))
    {
        return Err(CallbackError::Unavailable);
    }
    Ok(record)
}

#[derive(Debug, thiserror::Error)]
enum MutationError {
    #[error(transparent)]
    Core(#[from] CallbackError),
    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),
}
