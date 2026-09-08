//! Durable identity for one complete sandbox recovery point.

use std::collections::BTreeSet;

use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, RestoreRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::AgentId;
use crate::seconds_from_rfc3339;

use super::{Sandbox, SandboxModelError, SandboxStatus};

pub const SANDBOX_CHECKPOINT_INTERVAL_SECONDS: u64 =
    crate::platform::DEFAULT_CHECKPOINT_INTERVAL_SECONDS;
pub const MAX_SANDBOX_CONVERSATION_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

/// One conversation identity and digest sealed into the whole-home manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCheckpointConversationRef {
    pub agent_id: AgentId,
    pub conversation_id: String,
    pub archive_sha256: String,
}

/// Transient capture evidence. Conversation archive bytes are never serialized.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxCheckpointConversation {
    agent_id: AgentId,
    conversation_id: String,
    archive_sha256: String,
    archive: Vec<u8>,
}

impl std::fmt::Debug for SandboxCheckpointConversation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SandboxCheckpointConversation")
            .field("agent_id", &self.agent_id)
            .field("conversation_id", &self.conversation_id)
            .field("archive_sha256", &self.archive_sha256)
            .field("archive_bytes", &self.archive.len())
            .finish()
    }
}

impl SandboxCheckpointConversation {
    /// Bind a runtime capture to the conversation the durable Agent expected.
    pub fn new(
        agent_id: AgentId,
        expected_conversation_id: impl Into<String>,
        snapshot: ConversationSnapshot,
    ) -> Result<Self, SandboxModelError> {
        let expected_conversation_id = expected_conversation_id.into();
        let capture = Self {
            agent_id,
            conversation_id: snapshot.conversation_id,
            archive_sha256: snapshot.archive_sha256,
            archive: snapshot.archive,
        };
        if capture.conversation_id != expected_conversation_id {
            return Err(invalid_checkpoint());
        }
        capture.reference()?;
        Ok(capture)
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    fn reference(&self) -> Result<SandboxCheckpointConversationRef, SandboxModelError> {
        let bytes = u64::try_from(self.archive.len()).unwrap_or(u64::MAX);
        if self.agent_id.as_str().trim().is_empty()
            || self.conversation_id.trim().is_empty()
            || self.conversation_id.len() > 512
            || bytes == 0
            || bytes > MAX_SANDBOX_CONVERSATION_SNAPSHOT_BYTES
            || !valid_sha256(&self.archive_sha256)
            || digest(&self.archive) != self.archive_sha256
        {
            return Err(invalid_checkpoint());
        }
        Ok(SandboxCheckpointConversationRef {
            agent_id: self.agent_id.clone(),
            conversation_id: self.conversation_id.clone(),
            archive_sha256: self.archive_sha256.clone(),
        })
    }
}

/// Immutable whole-home object plus every live conversation captured with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCheckpointRef {
    pub epoch: u64,
    pub object_reference: String,
    pub sha256: String,
    pub conversations: Vec<SandboxCheckpointConversationRef>,
    pub sealed_at: String,
    pub manifest_sha256: String,
}

impl SandboxCheckpointRef {
    /// Seal adapter receipts and content-bearing capture evidence into metadata.
    pub fn seal(
        receipt: &CheckpointReceipt,
        conversations: &[SandboxCheckpointConversation],
        sealed_at: impl Into<String>,
    ) -> Result<Self, SandboxModelError> {
        let mut conversation_refs = conversations
            .iter()
            .map(SandboxCheckpointConversation::reference)
            .collect::<Result<Vec<_>, _>>()?;
        conversation_refs.sort_by(|left, right| {
            left.agent_id
                .cmp(&right.agent_id)
                .then_with(|| left.conversation_id.cmp(&right.conversation_id))
        });
        let mut reference = Self {
            epoch: receipt.epoch,
            object_reference: receipt.object_reference.clone(),
            sha256: receipt.sha256.clone(),
            conversations: conversation_refs,
            sealed_at: sealed_at.into(),
            manifest_sha256: String::new(),
        };
        reference.manifest_sha256 = reference.canonical_digest()?;
        reference.validate()?;
        Ok(reference)
    }

    /// Validate durable metadata without trusting the referenced object.
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if self.epoch == 0
            || self.object_reference.trim().is_empty()
            || !valid_sha256(&self.sha256)
            || !valid_sha256(&self.manifest_sha256)
            || seconds_from_rfc3339(&self.sealed_at).is_none()
        {
            return Err(invalid_checkpoint());
        }

        let mut agent_ids = BTreeSet::new();
        let mut conversation_ids = BTreeSet::new();
        let mut previous_agent: Option<&AgentId> = None;
        for conversation in &self.conversations {
            if conversation.agent_id.as_str().trim().is_empty()
                || conversation.conversation_id.trim().is_empty()
                || conversation.conversation_id.len() > 512
                || !valid_sha256(&conversation.archive_sha256)
                || previous_agent.is_some_and(|previous| previous >= &conversation.agent_id)
                || !agent_ids.insert(&conversation.agent_id)
                || !conversation_ids.insert(conversation.conversation_id.as_str())
            {
                return Err(invalid_checkpoint());
            }
            previous_agent = Some(&conversation.agent_id);
        }
        if self.canonical_digest()? != self.manifest_sha256 {
            return Err(invalid_checkpoint());
        }
        Ok(())
    }

    /// Verify the exact adapter receipt and conversation bytes used to seal this ref.
    pub fn validate_capture(
        &self,
        receipt: &CheckpointReceipt,
        conversations: &[SandboxCheckpointConversation],
    ) -> Result<(), SandboxModelError> {
        self.validate()?;
        if receipt.epoch != self.epoch
            || receipt.object_reference != self.object_reference
            || receipt.sha256 != self.sha256
            || !valid_sha256(&receipt.sha256)
        {
            return Err(invalid_checkpoint());
        }
        let mut observed = conversations
            .iter()
            .map(SandboxCheckpointConversation::reference)
            .collect::<Result<Vec<_>, _>>()?;
        observed.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        if observed != self.conversations {
            return Err(invalid_checkpoint());
        }
        Ok(())
    }

    /// Project only the identity Layer 3 is allowed to restore.
    pub fn restore_request(&self) -> Result<RestoreRequest, SandboxModelError> {
        self.validate()?;
        Ok(RestoreRequest {
            object_reference: self.object_reference.clone(),
            sha256: self.sha256.clone(),
        })
    }

    fn canonical_digest(&self) -> Result<String, SandboxModelError> {
        let mut digest = Sha256::new();
        digest.update(b"cma-sandbox-checkpoint-v1\0");
        digest.update(self.epoch.to_be_bytes());
        hash_text(&mut digest, &self.object_reference)?;
        hash_text(&mut digest, &self.sha256)?;
        digest.update(
            u64::try_from(self.conversations.len())
                .map_err(|_| invalid_checkpoint())?
                .to_be_bytes(),
        );
        for conversation in &self.conversations {
            hash_text(&mut digest, conversation.agent_id.as_str())?;
            hash_text(&mut digest, &conversation.conversation_id)?;
            hash_text(&mut digest, &conversation.archive_sha256)?;
        }
        hash_text(&mut digest, &self.sealed_at)?;
        Ok(hex(&digest.finalize()))
    }
}

/// Latest recovery point, accepting a legacy previous generation until publication.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxCheckpointHistory {
    pub latest: Option<SandboxCheckpointRef>,
    pub previous: Option<SandboxCheckpointRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxCheckpointPublication {
    Replayed,
    Published { evicted: Vec<SandboxCheckpointRef> },
}

impl SandboxCheckpointHistory {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if self.latest.is_none() && self.previous.is_some() {
            return Err(invalid_checkpoint());
        }
        if let Some(latest) = self.latest.as_ref() {
            latest.validate()?;
        }
        if let Some(previous) = self.previous.as_ref() {
            previous.validate()?;
            if self
                .latest
                .as_ref()
                .is_none_or(|latest| previous.epoch >= latest.epoch)
            {
                return Err(invalid_checkpoint());
            }
        }
        Ok(())
    }

    /// Publish monotonically, evicting old generations only with a complete replacement.
    pub fn publish(
        &mut self,
        checkpoint: SandboxCheckpointRef,
    ) -> Result<SandboxCheckpointPublication, SandboxModelError> {
        self.validate()?;
        checkpoint.validate()?;
        if self.latest.as_ref() == Some(&checkpoint) {
            return Ok(SandboxCheckpointPublication::Replayed);
        }
        if self
            .latest
            .as_ref()
            .is_some_and(|latest| checkpoint.epoch <= latest.epoch)
        {
            return Err(invalid_checkpoint());
        }
        let evicted = self
            .previous
            .take()
            .into_iter()
            .chain(self.latest.replace(checkpoint))
            .collect();
        self.validate()?;
        Ok(SandboxCheckpointPublication::Published { evicted })
    }
}

/// Periodic captures age from the later of the last snapshot and runtime binding.
pub fn sandbox_checkpoint_due(sandbox: &Sandbox, now: &str) -> bool {
    sandbox_checkpoint_due_with_interval(sandbox, now, SANDBOX_CHECKPOINT_INTERVAL_SECONDS)
}

pub fn sandbox_checkpoint_due_with_interval(
    sandbox: &Sandbox,
    now: &str,
    interval_seconds: u64,
) -> bool {
    let Some(now_seconds) = seconds_from_rfc3339(now) else {
        return false;
    };
    if !(60..=86_400).contains(&interval_seconds)
        || sandbox.status != SandboxStatus::Active
        || sandbox.runtime.is_none()
        || !sandbox.checkpoint_capture_cooldown_elapsed(now)
    {
        return false;
    }
    let Some(bound_at) = sandbox
        .runtime
        .as_ref()
        .and_then(|runtime| seconds_from_rfc3339(&runtime.bound_at))
    else {
        return false;
    };
    let baseline = match sandbox.lifecycle.checkpoints.latest.as_ref() {
        Some(checkpoint) if checkpoint.validate().is_ok() => {
            let Some(sealed_at) = seconds_from_rfc3339(&checkpoint.sealed_at) else {
                return false;
            };
            bound_at.max(sealed_at)
        }
        Some(_) => return false,
        None => bound_at,
    };
    baseline
        .checked_add(interval_seconds)
        .is_some_and(|due| now_seconds >= due)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hash_text(digest: &mut Sha256, value: &str) -> Result<(), SandboxModelError> {
    digest.update(
        u64::try_from(value.len())
            .map_err(|_| invalid_checkpoint())?
            .to_be_bytes(),
    );
    digest.update(value.as_bytes());
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

const fn invalid_checkpoint() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.checkpoint")
}
