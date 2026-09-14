//! Metadata from the same immutable disk as the uploaded home archive.

use std::time::{Duration, UNIX_EPOCH};

use layer3_adapter::{HomeSnapshotExportReceipt, HomeSnapshotExportRequest};

use super::{
    invalid_checkpoint, SandboxCheckpointConversationRef, SandboxCheckpointRef, SandboxModelError,
};
use crate::ids::AgentId;

impl SandboxCheckpointRef {
    /// Use the receipt returned by a verified immutable-view export/upload.
    /// This projection does not fetch storage or consult current Agent heads:
    /// those may have advanced while the captured archive was being uploaded.
    pub fn seal_export(
        request: &HomeSnapshotExportRequest,
        receipt: &HomeSnapshotExportReceipt,
    ) -> Result<Self, SandboxModelError> {
        receipt
            .validate_for(request)
            .map_err(|_| invalid_checkpoint())?;
        let captured = UNIX_EPOCH
            .checked_add(Duration::from_secs(
                request
                    .capture_time_unix_seconds()
                    .map_err(|_| invalid_checkpoint())?,
            ))
            .ok_or_else(invalid_checkpoint)?;
        let mut reference = Self {
            epoch: receipt.checkpoint.epoch,
            object_reference: receipt.checkpoint.object_reference.clone(),
            sha256: receipt.checkpoint.sha256.clone(),
            conversations: receipt
                .conversations
                .iter()
                .map(|conversation| SandboxCheckpointConversationRef {
                    agent_id: AgentId(conversation.identity.agent_id.clone()),
                    conversation_id: conversation.identity.conversation_id.clone(),
                    archive_sha256: conversation.archive_sha256.clone(),
                })
                .collect(),
            sealed_at: crate::rfc3339_at(captured),
            manifest_sha256: String::new(),
        };
        reference.manifest_sha256 = reference.canonical_digest()?;
        reference.validate()?;
        Ok(reference)
    }

    pub fn validate_export(
        &self,
        request: &HomeSnapshotExportRequest,
        receipt: &HomeSnapshotExportReceipt,
    ) -> Result<(), SandboxModelError> {
        if *self != Self::seal_export(request, receipt)? {
            return Err(invalid_checkpoint());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "checkpoint_export_tests.rs"]
mod tests;
