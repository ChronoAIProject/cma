//! Export authority and metadata for one immutable home snapshot.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AdapterError, CheckpointReceipt, HomeSnapshotObservation, HomeSnapshotRequest, Result,
    WriterState,
};

pub const MAX_SNAPSHOT_EXPORT_CONVERSATIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotConversationIdentity {
    pub agent_id: String,
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotExportRequest {
    pub snapshot: HomeSnapshotRequest,
    pub observation: HomeSnapshotObservation,
    pub runtime_generation: u64,
    /// Identities admitted before capture; contents come exclusively from the disk view.
    pub conversations: Vec<HomeSnapshotConversationIdentity>,
}

impl HomeSnapshotExportRequest {
    /// Normalize the provider timestamp only when projecting into a service
    /// clock. The full original timestamp remains bound by the request digest.
    pub fn capture_time_unix_seconds(&self) -> Result<u64> {
        self.validate()?;
        let stamp = self.observation.captured_at.as_ref().ok_or_else(invalid)?;
        let time: k8s_openapi::apimachinery::pkg::apis::meta::v1::Time =
            serde_json::from_value(serde_json::Value::String(stamp.clone()))
                .map_err(|_| invalid())?;
        u64::try_from(time.0.as_second()).map_err(|_| invalid())
    }

    pub fn validate(&self) -> Result<()> {
        self.observation.receipt.validate_for(&self.snapshot)?;
        let stamp = self.observation.captured_at.as_ref().ok_or_else(invalid)?;
        let _: k8s_openapi::apimachinery::pkg::apis::meta::v1::Time =
            serde_json::from_value(serde_json::Value::String(stamp.clone()))
                .map_err(|_| invalid())?;
        if !self.observation.ready || self.runtime_generation == 0 {
            return Err(invalid());
        }
        Self::validate_conversations(&self.conversations)
    }

    /// Validate the identity set before the immutable snapshot is allocated.
    pub fn validate_conversations(identities: &[HomeSnapshotConversationIdentity]) -> Result<()> {
        if identities.len() > MAX_SNAPSHOT_EXPORT_CONVERSATIONS {
            return Err(invalid());
        }
        let mut agents = BTreeSet::new();
        let mut conversations = BTreeSet::new();
        let mut previous = None;
        for identity in identities {
            if identity.agent_id.is_empty()
                || identity.agent_id.len() > 64
                || identity.agent_id.starts_with('.')
                || !identity
                    .agent_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
                || identity.conversation_id.is_empty()
                || identity.conversation_id.len() > 256
                || identity.conversation_id.trim() != identity.conversation_id
                || identity.conversation_id.chars().any(char::is_control)
                || !agents.insert(&identity.agent_id)
                || !conversations.insert(&identity.conversation_id)
                || previous.is_some_and(|prior| prior >= identity)
            {
                return Err(invalid());
            }
            previous = Some(identity);
        }
        Ok(())
    }

    pub fn computed_sha256(&self) -> Result<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| invalid())?;
        let mut digest = Sha256::new();
        digest.update(b"cma-home-snapshot-export-v1\0");
        digest.update(bytes);
        Ok(format!("{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotExportConversation {
    pub identity: HomeSnapshotConversationIdentity,
    pub archive_sha256: String,
}

/// The home archive and conversation hashes were read from the same immutable source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSnapshotExportReceipt {
    pub request_sha256: String,
    pub checkpoint: CheckpointReceipt,
    pub archive_bytes: u64,
    pub conversations: Vec<HomeSnapshotExportConversation>,
}

impl HomeSnapshotExportReceipt {
    pub fn validate_for(&self, request: &HomeSnapshotExportRequest) -> Result<()> {
        if self.request_sha256 != request.computed_sha256()?
            || self.checkpoint.epoch != request.snapshot.epoch
            || self.checkpoint.writers != WriterState::Resumed
            || !sha256(&self.checkpoint.sha256)
            || self.checkpoint.object_reference != format!("home-{}.cma", self.checkpoint.sha256)
            || self.archive_bytes == 0
            || self.archive_bytes > 17 * 1024 * 1024 * 1024
            || self.conversations.len() != request.conversations.len()
            || self
                .conversations
                .iter()
                .zip(&request.conversations)
                .any(|(observed, expected)| {
                    &observed.identity != expected || !sha256(&observed.archive_sha256)
                })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("snapshot export authority or receipt is inconsistent".into())
}
