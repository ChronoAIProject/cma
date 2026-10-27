//! One exact Lark message has one mutation at a time, across gateway replicas.
//! An abandoned or ambiguous write is never made safe by a lease timeout.
use crate::records::{RecordClient, RecordError};
use cmaeg_core::{
    ids::RecordId,
    problem::{Problem, ProblemCode},
    seal::{open_interaction, seal_interaction, MasterKeyRing},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub(crate) struct Store {
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lock {
    kind: String,
    id: String,
    owner: String,
    destination: String,
    held: bool,
    pub reaction: Option<Reaction>,
    expires_at: i64,
    #[serde(skip)]
    version: u64,
}
impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            records: records.interactions(),
            keys,
        }
    }
    pub async fn acquire(
        &self,
        destination: String,
        owner: &str,
        expires_at: i64,
    ) -> Result<Option<Lock>, Problem> {
        let digest = Sha256::digest(format!("cmaeg.lark-message-mutation.v1\0{destination}"));
        let mut bytes: [u8; 16] = digest[..16].try_into().unwrap();
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let mut lock = Lock {
            kind: "lark_message_mutation_v1".into(),
            id: uuid::Uuid::from_bytes(bytes).to_string(),
            owner: owner.into(),
            destination,
            held: true,
            reaction: None,
            expires_at,
            version: 0,
        };
        let url = self
            .records
            .record_url(&lock.id)
            .map_err(|_| unavailable())?;
        let expected = match self.records.get(url.as_str()).await {
            Ok(row) => {
                let opened =
                    open_interaction(&self.keys, &lock.id, &row.blob).map_err(|_| unavailable())?;
                let previous: Lock =
                    serde_json::from_slice(&opened.payload).map_err(|_| unavailable())?;
                if previous.kind != lock.kind
                    || previous.id != lock.id
                    || previous.owner != lock.owner
                    || previous.destination != lock.destination
                {
                    return Err(unavailable());
                }
                if previous.held {
                    return Ok(None);
                }
                lock.expires_at = lock.expires_at.max(previous.expires_at);
                lock.reaction = previous.reaction;
                Some(row.version)
            }
            Err(RecordError::NotFound) => None,
            Err(_) => return Err(unavailable()),
        };
        match self.save(&lock, expected).await {
            Ok(version) => {
                lock.version = version;
                Ok(Some(lock))
            }
            Err(RecordError::Conflict { .. }) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }
    pub async fn retain(&self, lock: &mut Lock, reaction: Option<Reaction>) -> Result<(), Problem> {
        lock.reaction = reaction;
        lock.version = self
            .save(lock, Some(lock.version))
            .await
            .map_err(|_| unavailable())?;
        Ok(())
    }
    pub async fn release(&self, mut lock: Lock) -> Result<(), Problem> {
        lock.held = false;
        self.save(&lock, Some(lock.version))
            .await
            .map_err(|_| unavailable())?;
        Ok(())
    }
    async fn save(&self, lock: &Lock, expected: Option<u64>) -> Result<u64, RecordError> {
        let bytes = serde_json::to_vec(lock)
            .map_err(|_| RecordError::Malformed("native effect lock encoding".into()))?;
        let blob = seal_interaction(&self.keys, &lock.id, &bytes)
            .map_err(|_| RecordError::Malformed("native effect lock sealing".into()))?;
        self.records
            .put_expiring(
                &RecordId::new(&lock.id)
                    .map_err(|_| RecordError::Malformed("native effect lock identity".into()))?,
                &lock.owner,
                &blob,
                expected,
                Some(lock.expires_at),
            )
            .await
    }
}
fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::Internal,
        503,
        "native_message_effect_fence_unavailable",
    )
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reaction {
    pub id: String,
    pub emoji: String,
}

pub(crate) fn destination(message: &cmaeg_core::seal::MessageRefPayload) -> String {
    let a = message
        .lark_effect
        .as_ref()
        .expect("checked Lark authority");
    super::receipts::hash(
        &serde_json::json!({"platform":a.platform,"app":a.binding.connection.app_id,"bot":a.bot_id,"chat":a.chat_id,"message":message.platform_message_id}),
    )
}
