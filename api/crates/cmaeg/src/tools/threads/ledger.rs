//! Durable idempotency plus a shared exact-thread fence. No timeout releases an uncertain write.
use super::unavailable;
use crate::{
    connections::store::index_id,
    records::{RecordClient, RecordError},
};
use cmaeg_core::{
    ids::RecordId,
    problem::{Problem, ProblemCode},
    seal::{open_interaction, seal_interaction, MasterKeyRing},
    thread_lifecycle::{ThreadOutcome, ThreadReceipt},
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::sync::Arc;

pub(super) struct Store {
    claims: RecordClient,
    fences: RecordClient,
    keys: Arc<MasterKeyRing>,
    telegram: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Claim {
    kind: String,
    id: String,
    owner: String,
    scope: String,
    request_hash: String,
    expires_at: i64,
    receipt: Option<ThreadReceipt>,
    #[serde(skip)]
    version: u64,
}
pub(super) enum Admission {
    Claimed(Claim),
    Replay(ThreadReceipt),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fence {
    kind: String,
    id: String,
    owner: String,
    destination: String,
    held: bool,
    #[serde(skip)]
    version: u64,
}
impl Store {
    pub fn new(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            claims: records.clone().interactions(),
            fences: records.thread_fences(),
            keys,
            telegram: false,
        }
    }
    pub fn telegram(records: RecordClient, keys: Arc<MasterKeyRing>) -> Self {
        Self {
            telegram: true,
            ..Self::new(records, keys)
        }
    }
    pub async fn claim(
        &self,
        scope: String,
        request_id: &str,
        request_hash: String,
        owner: &str,
        expires_at: i64,
    ) -> Result<Admission, Problem> {
        let mut claim = Claim {
            kind: if self.telegram {
                "telegram_topic_claim_v1"
            } else {
                "discord_thread_claim_v1"
            }
            .into(),
            id: index_id(
                if self.telegram {
                    "telegram-topic-claim-v1"
                } else {
                    "discord-thread-claim-v1"
                },
                &[&scope, request_id],
            ),
            owner: owner.into(),
            scope,
            request_hash,
            expires_at,
            receipt: None,
            version: 0,
        };
        if let Some((saved, _)) = self.read::<Claim>(&self.claims, &claim.id).await? {
            return replay(&claim, saved);
        }
        match self
            .save(
                &self.claims,
                &claim.id,
                owner,
                &claim,
                None,
                Some(expires_at),
            )
            .await
        {
            Ok(version) => {
                claim.version = version;
                Ok(Admission::Claimed(claim))
            }
            Err(RecordError::Conflict { .. }) => {
                let (saved, _) = self
                    .read::<Claim>(&self.claims, &claim.id)
                    .await?
                    .ok_or_else(unavailable)?;
                replay(&claim, saved)
            }
            Err(_) => Err(unavailable()),
        }
    }
    pub async fn finish(&self, mut claim: Claim, receipt: &ThreadReceipt) -> Result<(), Problem> {
        claim.receipt = Some(receipt.clone());
        self.save(
            &self.claims,
            &claim.id,
            &claim.owner,
            &claim,
            Some(claim.version),
            Some(claim.expires_at),
        )
        .await
        .map_err(|_| unavailable())?;
        Ok(())
    }
    pub async fn acquire(
        &self,
        bot: &str,
        thread: &str,
        owner: &str,
    ) -> Result<Option<Fence>, Problem> {
        let destination = super::hash(&serde_json::json!({"bot":bot,"thread":thread}));
        self.acquire_destination(destination, owner).await
    }
    pub async fn acquire_topic(
        &self,
        bot: &str,
        chat: &str,
        topic: &str,
        owner: &str,
    ) -> Result<Option<Fence>, Problem> {
        let destination = super::hash(
            &serde_json::json!({"platform":"telegram","bot":bot,"chat":chat,"topic":topic}),
        );
        self.acquire_destination(destination, owner).await
    }
    pub async fn acquire_topic_creation(
        &self,
        bot: &str,
        chat: &str,
        owner: &str,
    ) -> Result<Option<Fence>, Problem> {
        let destination = super::hash(
            &serde_json::json!({"platform":"telegram","bot":bot,"chat":chat,"operation":"create_topic"}),
        );
        self.acquire_destination(destination, owner).await
    }
    async fn acquire_destination(
        &self,
        destination: String,
        owner: &str,
    ) -> Result<Option<Fence>, Problem> {
        let mut fence = Fence {
            kind: if self.telegram {
                "telegram_topic_fence_v1"
            } else {
                "discord_thread_fence_v1"
            }
            .into(),
            id: index_id(
                if self.telegram {
                    "telegram-topic-fence-v1"
                } else {
                    "discord-thread-fence-v1"
                },
                &[&destination],
            ),
            owner: owner.into(),
            destination,
            held: true,
            version: 0,
        };
        let expected = match self.read::<Fence>(&self.fences, &fence.id).await? {
            None => None,
            Some((saved, version)) => {
                if saved.kind != fence.kind
                    || saved.id != fence.id
                    || saved.owner != fence.owner
                    || saved.destination != fence.destination
                {
                    return Err(unavailable());
                }
                if saved.held {
                    return Ok(None);
                }
                Some(version)
            }
        };
        // A future event must not erase an ambiguous write simply because the old event expired.
        match self
            .save(&self.fences, &fence.id, owner, &fence, expected, None)
            .await
        {
            Ok(version) => {
                fence.version = version;
                Ok(Some(fence))
            }
            Err(RecordError::Conflict { .. }) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }
    pub async fn release(&self, mut fence: Fence) -> Result<(), Problem> {
        fence.held = false;
        self.save(
            &self.fences,
            &fence.id,
            &fence.owner,
            &fence,
            Some(fence.version),
            None,
        )
        .await
        .map_err(|_| unavailable())?;
        Ok(())
    }
    async fn read<T: DeserializeOwned>(
        &self,
        records: &RecordClient,
        id: &str,
    ) -> Result<Option<(T, u64)>, Problem> {
        let url = records.record_url(id).map_err(|_| unavailable())?;
        let row = match records.get(url.as_str()).await {
            Ok(row) => row,
            Err(RecordError::NotFound) => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        let opened = open_interaction(&self.keys, id, &row.blob).map_err(|_| unavailable())?;
        let value = serde_json::from_slice(&opened.payload).map_err(|_| unavailable())?;
        Ok(Some((value, row.version)))
    }
    #[allow(clippy::too_many_arguments)]
    async fn save(
        &self,
        records: &RecordClient,
        id: &str,
        owner: &str,
        value: &impl Serialize,
        expected: Option<u64>,
        expires: Option<i64>,
    ) -> Result<u64, RecordError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| RecordError::Malformed("thread ledger encoding".into()))?;
        let blob = seal_interaction(&self.keys, id, &bytes)
            .map_err(|_| RecordError::Malformed("thread ledger sealing".into()))?;
        records
            .put_expiring(
                &RecordId::new(id)
                    .map_err(|_| RecordError::Malformed("thread ledger identity".into()))?,
                owner,
                &blob,
                expected,
                expires,
            )
            .await
    }
}
fn replay(expected: &Claim, saved: Claim) -> Result<Admission, Problem> {
    if saved.kind != expected.kind
        || saved.id != expected.id
        || saved.owner != expected.owner
        || saved.scope != expected.scope
        || saved.request_hash != expected.request_hash
        || saved.expires_at != expected.expires_at
    {
        return Err(Problem::new(
            ProblemCode::Conflict,
            409,
            "thread_idempotency_conflict",
        ));
    }
    Ok(Admission::Replay(saved.receipt.unwrap_or_else(|| {
        ThreadReceipt {
            outcome: ThreadOutcome::Unknown {
                reason: "thread_operation_in_progress_or_uncertain".into(),
            },
            thread: None,
        }
    })))
}

#[cfg(test)]
#[path = "ledger_mongo_tests.rs"]
mod mongo_tests;
