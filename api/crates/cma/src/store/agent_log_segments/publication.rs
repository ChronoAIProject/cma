//! Immutable publication receipts read before aggregate equality reconciliation.
//! Writers add these in the same transaction as all recorded effects. A later
//! aggregate is never, on its own, evidence that an uncertain publication won.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::{
    bson::{self, doc, Binary},
    options::ReadConcern,
    ClientSession,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{corrupt, digest, read_format, read_head, MAX_SEQUENCE};
use crate::store::{agents::read_agent, sandbox_mongo::MongoSandboxStore};

/// Allocated from the admitted operation before transaction/reconciliation
/// retries; the fingerprint covers the original request, not today's state.
#[derive(Clone)]
pub(in crate::store) struct PublicationIdentity {
    pub key: String,
    pub fingerprint: Binary,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct Publication {
    #[serde(rename = "_id")]
    pub id: String,
    pub format_version: i64,
    pub agent_id: String,
    pub incarnation: String,
    pub publication_key: String,
    pub fingerprint: Binary,
    pub expected_revision: i64,
    pub expected_seq: i64,
    pub first_seq: i64,
    pub last_seq: i64,
    pub result_revision: i64,
    pub head_revision: i64,
    pub active_ordinal: i64,
    #[serde(deserialize_with = "super::required_nullable")]
    pub previous_seal: Option<Binary>,
    pub result_digest: Binary,
}

pub(in crate::store) fn identity(agent: &str, incarnation: &str, key: &str) -> String {
    format!("{}:{agent}:{incarnation}:{}:{key}", agent.len(), key.len())
}

/// Sort every object recursively so dependency feature unification cannot make
/// a receipt depend on map insertion order. Preserve every value.
pub(in crate::store) fn result_digest<T: Serialize>(
    result: &T,
) -> Result<Binary, SandboxStoreError> {
    let mut canonical = serde_json::to_value(result).map_err(|_| corrupt())?;
    canonical.sort_all_objects();
    let bytes = serde_json::to_vec(&canonical).map_err(|_| corrupt())?;
    let mut hash = Sha256::new();
    hash.update(b"cma.agent-log.publication-result.v1\0");
    digest::field(&mut hash, &bytes);
    Ok(digest::binary(hash.finalize().to_vec()))
}

impl Publication {
    fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.format_version != 1
            || self.agent_id.is_empty()
            || self.incarnation.len() != 64
            || self.publication_key.is_empty()
            || self.publication_key.len() > 512
            || self.id != identity(&self.agent_id, &self.incarnation, &self.publication_key)
            || !digest::valid_digest(&self.fingerprint)
            || !digest::valid_digest(&self.result_digest)
            || self.expected_revision < 0
            || self.result_revision < self.expected_revision
            || self.expected_seq < 0
            || self.expected_seq > self.last_seq
            || self.last_seq > MAX_SEQUENCE
            || self.expected_seq.checked_add(1) != Some(self.first_seq)
            || self.last_seq - self.expected_seq > 64
            || self.head_revision < 1
            || self.active_ordinal < 0
            || self.previous_seal.is_some() != (self.active_ordinal > 0)
            || self
                .previous_seal
                .as_ref()
                .is_some_and(|seal| !digest::valid_digest(seal))
        {
            return Err(corrupt());
        }
        Ok(())
    }
}

/// None selects only the finite legacy proof. A segmented Agent requires its
/// exact durable receipt even if its current aggregate happens to match.
pub(in crate::store) async fn reconcile(
    store: &MongoSandboxStore,
    operation: Option<&PublicationIdentity>,
    expected: &Agent,
    logs: &[AgentLogEntry],
    result: &Binary,
    session: &mut ClientSession,
) -> Result<Option<bool>, SandboxStoreError> {
    let Some(row) = store
        .agents
        .find_one(doc! {"_id":expected.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
    else {
        return Ok(Some(false));
    };
    if read_format(&row)?.is_none() {
        return Ok(None);
    }
    let operation = operation.ok_or_else(|| {
        SandboxStoreError::Unavailable(
            "Segmented publication requires its original receipt identity".into(),
        )
    })?;
    let current = read_agent(&row)?;
    let head = read_head(store, &row, session, true)
        .await
        .map_err(|e| match e {
            crate::store::agent_log_integrity::TransactionLogError::Domain(e) => e,
            crate::store::agent_log_integrity::TransactionLogError::Driver(e) => {
                SandboxStoreError::Unavailable(e.to_string())
            }
        })?;
    let incarnation = digest::incarnation(expected);
    if head.incarnation != incarnation
        || current.revision < expected.revision
        || current.conversation.last_seq < expected.conversation.last_seq
    {
        return Err(corrupt());
    }
    let Some(row) = store
        .segment_logs
        .publications
        .find_one(doc! {
            "_id":identity(expected.id.as_str(), &incarnation, &operation.key),
        })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
    else {
        return Ok(Some(false));
    };
    let publication: Publication = bson::deserialize_from_document(row).map_err(|_| corrupt())?;
    publication.validate()?;
    if publication.fingerprint != operation.fingerprint {
        return Err(SandboxStoreError::Conflict);
    }
    if publication.agent_id != expected.id.as_str()
        || publication.incarnation != incarnation
        || publication.publication_key != operation.key
        || publication.result_digest != *result
        || publication.result_revision as u64 != expected.revision
        || publication.last_seq as u64 != expected.conversation.last_seq
        || publication.head_revision > head.revision
        || publication.active_ordinal > head.active_ordinal
        || (publication.head_revision == head.revision && publication.last_seq != head.last_seq)
        || (publication.active_ordinal == head.active_ordinal
            && publication.previous_seal != head.previous_seal)
        || (publication.last_seq - publication.expected_seq) as usize != logs.len()
        || logs
            .first()
            .is_some_and(|entry| entry.seq != publication.first_seq as u64)
        || logs
            .last()
            .is_some_and(|entry| entry.seq != publication.last_seq as u64)
    {
        return Err(corrupt());
    }
    if publication.active_ordinal > 0 {
        let seal = store.segment_logs.seals.find_one(doc! {
            "_id":super::Seal::identity(&head.agent_id, &head.incarnation, publication.active_ordinal - 1),
        }).read_concern(ReadConcern::snapshot()).session(&mut *session).await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?.ok_or_else(corrupt)?;
        let seal = super::Seal::read(seal)?;
        if publication.previous_seal.as_ref() != Some(&seal.seal_digest)
            || publication.last_seq < seal.end_seq
        {
            return Err(corrupt());
        }
    }
    crate::store::agent_log_receipts::matches(store, &current, logs, session)
        .await
        .map(Some)
}
