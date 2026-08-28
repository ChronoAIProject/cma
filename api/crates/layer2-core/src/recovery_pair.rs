//! Exact, immutable identity for a recoverable Trigger runtime checkpoint.

use layer3_adapter::WorkspacePackage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CheckpointId, LaunchId, SessionId, TriggerSessionOrigin, TriggerValidationError,
    WorkspaceCheckpoint,
};

pub const TRIGGER_RECOVERY_PAIR_VERSION: u32 = 1;
pub const MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

/// Durable identity shared by the workspace object, conversation object, and
/// every replacement request that is allowed to consume them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerRecoveryPairRef {
    pub version: u32,
    pub checkpoint_id: CheckpointId,
    pub launch_id: LaunchId,
    pub checkpoint_version: u64,
    pub session_id: SessionId,
    pub source_revision: String,
    pub baseline_manifest_digest: String,
    pub workspace_manifest_digest: String,
    pub workspace_epoch: u64,
    pub producer_runtime_generation: u64,
    pub workspace_object_reference: String,
    pub workspace_archive_sha256: String,
    pub conversation_object_reference: String,
    pub conversation_sha256: String,
    pub conversation_bytes: u64,
    pub captured_at: String,
    pub pair_sha256: String,
}

impl TriggerRecoveryPairRef {
    pub fn seal(
        checkpoint: &WorkspaceCheckpoint,
        baseline_manifest_digest: impl Into<String>,
        conversation_object_reference: impl Into<String>,
        conversation: &[u8],
    ) -> Result<Self, TriggerValidationError> {
        let conversation_bytes = u64::try_from(conversation.len()).map_err(|_| {
            TriggerValidationError::field(
                "checkpoint.recovery_pair.conversation_bytes",
                "exceeds the supported wire bound",
            )
        })?;
        let mut reference = Self {
            version: TRIGGER_RECOVERY_PAIR_VERSION,
            checkpoint_id: checkpoint.id.clone(),
            launch_id: checkpoint.launch_id.clone(),
            checkpoint_version: checkpoint.version,
            session_id: checkpoint.session_id.clone(),
            source_revision: checkpoint.manifest.source_revision.clone(),
            baseline_manifest_digest: baseline_manifest_digest.into(),
            workspace_manifest_digest: checkpoint.manifest.digest.clone(),
            workspace_epoch: checkpoint.workspace_epoch,
            producer_runtime_generation: checkpoint.runtime_generation,
            workspace_object_reference: checkpoint.object_reference.clone(),
            workspace_archive_sha256: checkpoint.archive_sha256.clone(),
            conversation_object_reference: conversation_object_reference.into(),
            conversation_sha256: digest(conversation),
            conversation_bytes,
            captured_at: checkpoint.created_at.clone(),
            pair_sha256: String::new(),
        };
        reference.pair_sha256 = reference.canonical_digest()?;
        reference.validate()?;
        Ok(reference)
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.version != TRIGGER_RECOVERY_PAIR_VERSION {
            return Err(field("version", "is not supported"));
        }
        if self.checkpoint_id.0.trim().is_empty()
            || self.launch_id.0.trim().is_empty()
            || self.session_id.0.trim().is_empty()
            || self.checkpoint_version == 0
            || self.workspace_epoch == 0
            || self.producer_runtime_generation == 0
        {
            return Err(field(
                "identity",
                "does not name one Launch, session, workspace epoch, and runtime generation",
            ));
        }
        require_git_sha("source_revision", &self.source_revision)?;
        require_sha256("baseline_manifest_digest", &self.baseline_manifest_digest)?;
        require_sha256("workspace_manifest_digest", &self.workspace_manifest_digest)?;
        require_sha256("workspace_archive_sha256", &self.workspace_archive_sha256)?;
        require_sha256("conversation_sha256", &self.conversation_sha256)?;
        require_sha256("pair_sha256", &self.pair_sha256)?;
        if self.workspace_object_reference.trim().is_empty()
            || self.conversation_object_reference.trim().is_empty()
            || self.workspace_object_reference == self.conversation_object_reference
        {
            return Err(field(
                "object_reference",
                "must name two distinct immutable objects",
            ));
        }
        if self.conversation_bytes == 0
            || self.conversation_bytes > MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES
        {
            return Err(field(
                "conversation_bytes",
                format!("must be between 1 and {MAX_TRIGGER_CONVERSATION_SNAPSHOT_BYTES} bytes"),
            ));
        }
        if crate::seconds_from_rfc3339(&self.captured_at).is_none() {
            return Err(field("captured_at", "is not a CMA timestamp"));
        }
        if self.canonical_digest()? != self.pair_sha256 {
            return Err(field(
                "pair_sha256",
                "does not bind the exact workspace and conversation identity",
            ));
        }
        Ok(())
    }

    pub fn validate_checkpoint(
        &self,
        checkpoint: &WorkspaceCheckpoint,
    ) -> Result<(), TriggerValidationError> {
        self.validate()?;
        if self.checkpoint_id != checkpoint.id
            || self.launch_id != checkpoint.launch_id
            || self.checkpoint_version != checkpoint.version
            || self.session_id != checkpoint.session_id
            || self.source_revision != checkpoint.manifest.source_revision
            || self.workspace_manifest_digest != checkpoint.manifest.digest
            || self.workspace_epoch != checkpoint.workspace_epoch
            || self.producer_runtime_generation != checkpoint.runtime_generation
            || self.workspace_object_reference != checkpoint.object_reference
            || self.workspace_archive_sha256 != checkpoint.archive_sha256
            || self.captured_at != checkpoint.created_at
        {
            return Err(field(
                "identity",
                "does not match its containing workspace checkpoint",
            ));
        }
        Ok(())
    }

    fn canonical_digest(&self) -> Result<String, TriggerValidationError> {
        let mut digest = Sha256::new();
        digest.update(b"cma-trigger-recovery-pair-v1\0");
        digest.update(self.version.to_be_bytes());
        hash_text(&mut digest, &self.checkpoint_id.0)?;
        hash_text(&mut digest, &self.launch_id.0)?;
        digest.update(self.checkpoint_version.to_be_bytes());
        hash_text(&mut digest, &self.session_id.0)?;
        hash_text(&mut digest, &self.source_revision)?;
        hash_text(&mut digest, &self.baseline_manifest_digest)?;
        hash_text(&mut digest, &self.workspace_manifest_digest)?;
        digest.update(self.workspace_epoch.to_be_bytes());
        digest.update(self.producer_runtime_generation.to_be_bytes());
        hash_text(&mut digest, &self.workspace_object_reference)?;
        hash_text(&mut digest, &self.workspace_archive_sha256)?;
        hash_text(&mut digest, &self.conversation_object_reference)?;
        hash_text(&mut digest, &self.conversation_sha256)?;
        digest.update(self.conversation_bytes.to_be_bytes());
        hash_text(&mut digest, &self.captured_at)?;
        Ok(hex(&digest.finalize()))
    }
}

/// Bytes already authorized and verified as one exact recovery point.
#[derive(Debug, Clone)]
pub struct TriggerRecoveryPair {
    pub reference: TriggerRecoveryPairRef,
    pub workspace: WorkspacePackage,
    pub conversation: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerRecoveryPairCommitReceipt {
    pub storage: crate::WorkspaceCheckpointCommitOutcome,
    pub reference: TriggerRecoveryPairRef,
}

impl TriggerRecoveryPair {
    pub fn validate_for(
        &self,
        origin: &TriggerSessionOrigin,
    ) -> Result<(), TriggerValidationError> {
        self.reference.validate()?;
        let expected_session = origin.session_id()?;
        if self.reference.launch_id != origin.launch_id
            || self.reference.session_id != expected_session
            || self.reference.source_revision != origin.source_revision
            || self.reference.baseline_manifest_digest != origin.manifest_digest
            || self.reference.workspace_epoch != origin.workspace_epoch
        {
            return Err(field(
                "identity",
                "does not match the Trigger session authority",
            ));
        }
        if self.workspace.epoch != self.reference.workspace_epoch
            || self.workspace.source_revision != self.reference.source_revision
            || self.workspace.baseline_manifest_digest != self.reference.baseline_manifest_digest
            || self.workspace.manifest_digest != self.reference.workspace_manifest_digest
            || self.workspace.archive_sha256 != self.reference.workspace_archive_sha256
            || digest(&self.workspace.archive) != self.reference.workspace_archive_sha256
        {
            return Err(field(
                "workspace",
                "does not match the retained recovery-pair workspace",
            ));
        }
        let conversation_bytes = u64::try_from(self.conversation.len()).unwrap_or(u64::MAX);
        if conversation_bytes != self.reference.conversation_bytes
            || digest(&self.conversation) != self.reference.conversation_sha256
        {
            return Err(field(
                "conversation",
                "does not match the retained recovery-pair conversation",
            ));
        }
        Ok(())
    }
}

fn field(name: &str, detail: impl Into<String>) -> TriggerValidationError {
    TriggerValidationError::field(format!("checkpoint.recovery_pair.{name}"), detail)
}

fn require_git_sha(name: &str, value: &str) -> Result<(), TriggerValidationError> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(field(name, "must be a Git SHA-1"));
    }
    Ok(())
}

fn require_sha256(name: &str, value: &str) -> Result<(), TriggerValidationError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(field(name, "must be a SHA-256 digest"));
    }
    Ok(())
}

fn hash_text(digest: &mut Sha256, value: &str) -> Result<(), TriggerValidationError> {
    let len = u64::try_from(value.len())
        .map_err(|_| field("identity", "contains a value outside the hash wire bound"))?;
    digest.update(len.to_be_bytes());
    digest.update(value.as_bytes());
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(CHARS[usize::from(byte >> 4)]));
        out.push(char::from(CHARS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BillingMode, CheckpointIntegrity, WorkspaceManifest};

    fn fixture() -> (
        TriggerSessionOrigin,
        WorkspaceCheckpoint,
        WorkspacePackage,
        Vec<u8>,
    ) {
        let launch_id = LaunchId("00000000-0000-0000-0000-000000000662".into());
        let session_id = crate::trigger_session_id(&launch_id).unwrap();
        let source_revision = "a".repeat(40);
        let baseline = WorkspaceManifest::seal(source_revision.clone(), Vec::new()).unwrap();
        let workspace_archive = b"exact sealed repository workspace".to_vec();
        let workspace_sha256 = digest(&workspace_archive);
        let checkpoint = WorkspaceCheckpoint {
            id: CheckpointId("ckpt_pair_1".into()),
            launch_id: launch_id.clone(),
            version: 7,
            workspace_epoch: 3,
            session_id,
            runtime_generation: 5,
            object_reference:
                "trigger-workspaces/subjects/owner/launches/launch/checkpoints/v7.workspace.pack"
                    .into(),
            archive_sha256: workspace_sha256.clone(),
            manifest: baseline.clone(),
            integrity: CheckpointIntegrity::Complete,
            recovery_pair: None,
            created_at: "2026-08-28T00:00:00Z".into(),
            delete_after: Some("2026-09-28T00:00:00Z".into()),
        };
        let origin = TriggerSessionOrigin {
            launch_id,
            revision_digest: "b".repeat(64),
            sandbox_profile: "sandbox".into(),
            github_service_id: "github".into(),
            github_catalog_service_id: Some("github-catalog".into()),
            github_user_service_slug: Some("api-github".into()),
            workspace_epoch: 3,
            source_revision: source_revision.clone(),
            manifest_digest: baseline.digest.clone(),
            object_reference: "baseline.pack".into(),
            archive_sha256: "c".repeat(64),
            timeout_seconds: 3600,
            pricing_policy_reference: "zero-cost".into(),
            billing_mode: BillingMode::ZeroCost,
            pricing: None,
        };
        let workspace = WorkspacePackage {
            epoch: 3,
            source_revision,
            baseline_manifest_digest: baseline.digest.clone(),
            manifest_digest: baseline.digest,
            archive_sha256: workspace_sha256,
            archive: workspace_archive,
        };
        (
            origin,
            checkpoint,
            workspace,
            b"opaque conversation snapshot".to_vec(),
        )
    }

    #[test]
    fn an_exact_workspace_and_conversation_pair_validates() {
        let (origin, checkpoint, workspace, conversation) = fixture();
        let reference = TriggerRecoveryPairRef::seal(
            &checkpoint,
            origin.manifest_digest.clone(),
            "trigger-workspaces/subjects/owner/launches/launch/checkpoints/v7.conversation.snapshot",
            &conversation,
        )
        .unwrap();
        reference.validate_checkpoint(&checkpoint).unwrap();
        TriggerRecoveryPair {
            reference,
            workspace,
            conversation,
        }
        .validate_for(&origin)
        .unwrap();
    }

    #[test]
    fn changing_any_identity_after_sealing_breaks_the_pair_digest() {
        let (origin, checkpoint, _workspace, conversation) = fixture();
        let reference = TriggerRecoveryPairRef::seal(
            &checkpoint,
            origin.manifest_digest,
            "trigger-workspaces/subjects/owner/launches/launch/checkpoints/v7.conversation.snapshot",
            &conversation,
        )
        .unwrap();
        for mut changed in [reference.clone(), reference.clone(), reference.clone()] {
            if changed.checkpoint_version == reference.checkpoint_version {
                changed.checkpoint_version += 1;
            }
            assert!(changed.validate().is_err());
        }

        let mut changed = reference.clone();
        changed.conversation_object_reference.push_str(".other");
        assert!(changed.validate().is_err());
        let mut changed = reference;
        changed.producer_runtime_generation += 1;
        assert!(changed.validate().is_err());
    }

    #[test]
    fn corrupt_or_missing_half_bytes_never_validate_as_recoverable() {
        let (origin, checkpoint, workspace, conversation) = fixture();
        let reference = TriggerRecoveryPairRef::seal(
            &checkpoint,
            origin.manifest_digest.clone(),
            "trigger-workspaces/subjects/owner/launches/launch/checkpoints/v7.conversation.snapshot",
            &conversation,
        )
        .unwrap();

        let mut corrupt_workspace = workspace.clone();
        corrupt_workspace.archive.push(0);
        assert!(TriggerRecoveryPair {
            reference: reference.clone(),
            workspace: corrupt_workspace,
            conversation: conversation.clone(),
        }
        .validate_for(&origin)
        .is_err());

        assert!(TriggerRecoveryPair {
            reference,
            workspace,
            conversation: Vec::new(),
        }
        .validate_for(&origin)
        .is_err());
    }
}
