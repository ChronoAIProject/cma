//! Immutable completion evidence, distinct from admission and current head.
use layer2_core::{
    sandbox::{store::SandboxStoreError, Agent},
    seconds_from_rfc3339,
};
use mongodb::bson::{self, Binary, Document};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{corrupt, digest, Head, MAX_SEQUENCE};

/// Allocate before retry; retain with the durable command/work attempt. Mutable
/// lease or head state must not enter the original request fingerprint.
#[derive(Debug, Clone)]
pub struct PublicationIdentity {
    pub key: String,
    pub fingerprint: Binary,
    pub appended_at: String,
}

impl PublicationIdentity {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.key.is_empty()
            || self.key.len() > 512
            || !digest::valid_digest(&self.fingerprint)
            || self.appended_at.len() > 64
            || seconds_from_rfc3339(&self.appended_at).is_none()
        {
            return Err(corrupt());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    #[serde(rename = "_id")]
    pub id: String,
    pub format_version: i64,
    pub agent_id: String,
    pub incarnation: String,
    pub publication_key: String,
    pub fingerprint: Binary,
    pub appended_at: String,
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
    /// Format 2 binds the full Agent independently of the caller's domain result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_digest: Option<Binary>,
}

pub fn identity(agent: &str, incarnation: &str, key: &str) -> String {
    format!("{}:{agent}:{incarnation}:{}:{key}", agent.len(), key.len())
}

fn canonical_digest<T: Serialize>(domain: &[u8], value: &T) -> Result<Binary, SandboxStoreError> {
    let mut canonical = serde_json::to_value(value).map_err(|_| corrupt())?;
    canonical.sort_all_objects();
    let bytes = serde_json::to_vec(&canonical).map_err(|_| corrupt())?;
    let mut hash = Sha256::new();
    hash.update(domain);
    digest::field(&mut hash, &bytes);
    Ok(digest::binary(hash.finalize().to_vec()))
}

pub fn result_digest<T: Serialize>(result: &T) -> Result<Binary, SandboxStoreError> {
    canonical_digest(b"cma.agent-log.publication-result.v1\0", result)
}

pub fn request_digest<T: Serialize>(request: &T) -> Result<Binary, SandboxStoreError> {
    canonical_digest(b"cma.agent-log.publication-request.v1\0", request)
}

/// Separate from the immutable, caller-specific domain result witness.
pub fn agent_digest(agent: &Agent) -> Result<Binary, SandboxStoreError> {
    canonical_digest(b"cma.agent-log.current-agent.v1\0", agent)
}

impl Publication {
    pub fn new(
        current: &Agent,
        next: &Agent,
        head: &Head,
        operation: &PublicationIdentity,
        result_digest: Binary,
    ) -> Result<Self, SandboxStoreError> {
        operation.validate()?;
        head.validate()?;
        if current.id != next.id
            || digest::incarnation(current) != head.incarnation
            || digest::incarnation(next) != head.incarnation
            || head.agent_id != current.id.as_str()
            || u64::try_from(head.last_seq).ok() != Some(next.conversation.last_seq)
        {
            return Err(corrupt());
        }
        let value = Self {
            id: identity(current.id.as_str(), &head.incarnation, &operation.key),
            format_version: 2,
            agent_id: current.id.0.clone(),
            incarnation: head.incarnation.clone(),
            publication_key: operation.key.clone(),
            fingerprint: operation.fingerprint.clone(),
            appended_at: operation.appended_at.clone(),
            expected_revision: i64::try_from(current.revision).map_err(|_| corrupt())?,
            expected_seq: i64::try_from(current.conversation.last_seq).map_err(|_| corrupt())?,
            first_seq: i64::try_from(current.conversation.last_seq)
                .map_err(|_| corrupt())?
                .checked_add(1)
                .ok_or_else(corrupt)?,
            last_seq: head.last_seq,
            result_revision: i64::try_from(next.revision).map_err(|_| corrupt())?,
            head_revision: head.revision,
            active_ordinal: head.active_ordinal,
            previous_seal: head.previous_seal.clone(),
            result_digest,
            agent_digest: Some(agent_digest(next)?),
        };
        value.validate()?;
        Ok(value)
    }

    pub fn read(row: Document) -> Result<Self, SandboxStoreError> {
        // Historical v1 never carried an Agent witness, including explicit null.
        if row.get_i64("format_version").ok() == Some(1) && row.contains_key("agent_digest") {
            return Err(corrupt());
        }
        for field in [
            "format_version",
            "expected_revision",
            "expected_seq",
            "first_seq",
            "last_seq",
            "result_revision",
            "head_revision",
            "active_ordinal",
        ] {
            row.get_i64(field).map_err(|_| corrupt())?;
        }
        let value: Self = bson::deserialize_from_document(row).map_err(|_| corrupt())?;
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if !matches!(
            (self.format_version, &self.agent_digest),
            (1, None) | (2, Some(_))
        ) || self
            .agent_digest
            .as_ref()
            .is_some_and(|value| !digest::valid_digest(value))
            || self.agent_id.is_empty()
            || self.agent_id.len() > 512
            || self.incarnation.len() != 64
            || !self
                .incarnation
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.publication_key.is_empty()
            || self.publication_key.len() > 512
            || self.id != identity(&self.agent_id, &self.incarnation, &self.publication_key)
            || !digest::valid_digest(&self.fingerprint)
            || !digest::valid_digest(&self.result_digest)
            || self.appended_at.len() > 64
            || seconds_from_rfc3339(&self.appended_at).is_none()
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

    pub fn matches_request(
        &self,
        agent: &Agent,
        operation: &PublicationIdentity,
    ) -> Result<(), SandboxStoreError> {
        self.validate()?;
        operation.validate()?;
        if self.agent_id != agent.id.as_str()
            || self.incarnation != digest::incarnation(agent)
            || self.publication_key != operation.key
            || self.fingerprint != operation.fingerprint
            || self.appended_at != operation.appended_at
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}
