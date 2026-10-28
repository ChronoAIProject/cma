//! Durable barrier between recovery admission and old-runtime suspension.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    Sandbox, SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxModelError,
    SandboxRecoveryConversationProof, SandboxUnpublishedRecoveryProof,
};

const MAX_PROVIDER_PLAN_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxUnpublishedRecoveryState {
    pub operation_id: String,
    pub proof: Box<SandboxUnpublishedRecoveryProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiescence: Option<SandboxUnpublishedRecoveryQuiescence>,
}

/// Trusted adapter evidence after graceful completion. The provider verifies
/// original history prefixes and files, final persisted histories and process
/// absence, and journals its receipt on the original home for crash replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxUnpublishedRecoveryQuiescence {
    pub operation_id: String,
    pub runtime_id: String,
    pub runtime_generation: u64,
    pub home_identity: String,
    pub boot_id: Option<String>,
    pub provider_plan_sha256: String,
    pub runtime_process_absent: bool,
    pub conversations: Vec<SandboxRecoveryConversationProof>,
    pub completed_at: String,
}

impl SandboxUnpublishedRecoveryProof {
    pub fn provider_plan_digest(plan: &serde_json::Value) -> Result<String, SandboxModelError> {
        if plan.as_object().is_none_or(|object| object.is_empty()) {
            return Err(invalid());
        }
        let bytes = serde_json::to_vec(plan).map_err(|_| invalid())?;
        if bytes.len() > MAX_PROVIDER_PLAN_BYTES {
            return Err(invalid());
        }
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    pub(super) fn validate_provider_plan(&self) -> Result<(), SandboxModelError> {
        if !self.requires_native_quiescence
            || Self::provider_plan_digest(&self.provider_plan)? != self.provider_plan_sha256
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl SandboxUnpublishedRecoveryState {
    pub fn validate_for(&self, sandbox: &Sandbox) -> Result<(), SandboxModelError> {
        self.proof.validate_provider_plan()?;
        self.validate_reserved_provision_for(sandbox)?;
        let operation = sandbox.lifecycle.operation.as_ref().ok_or_else(invalid)?;
        let SandboxLifecycleOperation::ImageRotation { identity, .. } = operation else {
            return Err(invalid());
        };
        let source = sandbox.retained_home_recovery().ok_or_else(invalid)?;
        if self.operation_id != operation.authority().operation_id
            || self.proof.sandbox_id != sandbox.id
            || self.proof.runtime_generation != source.source_generation
            || self.proof.home_identity != source.home_identity
            || (self.proof.boot_id.is_none()
                && (!self.proof.runtime_process_absent || !self.proof.conversations.is_empty()))
            || self.proof.conversations.len() != identity.agents.len()
            || self.proof.conversations.iter().any(|proof| {
                identity
                    .agents
                    .iter()
                    .filter(|agent| {
                        agent.agent_id == proof.agent_id
                            && agent.conversation_id == proof.conversation_id
                    })
                    .count()
                    != 1
            })
        {
            return Err(invalid());
        }
        if let Some(binding) = &sandbox.lifecycle.suspended_runtime {
            if self.proof.runtime_id != binding.runtime_id
                || Some(self.proof.runtime_generation) != binding.provision_generation
                || self
                    .proof
                    .boot_id
                    .as_ref()
                    .is_some_and(|boot| binding.boot_id.as_ref().is_some_and(|held| held != boot))
            {
                return Err(invalid());
            }
        }
        if let Some(receipt) = &self.quiescence {
            self.validate_receipt(receipt)?;
            if crate::seconds_from_rfc3339(&receipt.completed_at)
                < crate::seconds_from_rfc3339(&operation.authority().requested_at)
            {
                return Err(invalid());
            }
        } else if !matches!(
            operation,
            SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::CheckpointVerification,
                ..
            }
        ) || sandbox.runtime.is_some()
            || sandbox.lifecycle.suspended_runtime.is_none()
            || sandbox.lifecycle.runtime_provision.is_some()
            || (sandbox.lifecycle.runtime_generation_high_water != self.proof.runtime_generation
                && (self.proof.boot_id.is_some()
                    || sandbox.lifecycle.runtime_generation_high_water
                        < self.proof.runtime_generation))
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn validate_receipt(
        &self,
        receipt: &SandboxUnpublishedRecoveryQuiescence,
    ) -> Result<(), SandboxModelError> {
        if receipt.operation_id != self.operation_id
            || receipt.runtime_id != self.proof.runtime_id
            || receipt.runtime_generation != self.proof.runtime_generation
            || receipt.home_identity != self.proof.home_identity
            || receipt.boot_id != self.proof.boot_id
            || receipt.provider_plan_sha256 != self.proof.provider_plan_sha256
            || !receipt.runtime_process_absent
            || crate::seconds_from_rfc3339(&receipt.completed_at).is_none()
        {
            return Err(invalid());
        }
        let identities = |proofs: &[SandboxRecoveryConversationProof]| {
            proofs
                .iter()
                .map(|proof| (proof.agent_id.clone(), proof.conversation_id.clone()))
                .collect::<BTreeSet<_>>()
        };
        let actual = identities(&receipt.conversations);
        if actual != identities(&self.proof.conversations)
            || actual.len() != receipt.conversations.len()
            || receipt.conversations.iter().any(|proof| {
                proof.history_sha256.len() != 64
                    || !proof
                        .history_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl Sandbox {
    /// Recovery admission and receipt publication require their dedicated
    /// transactions. Generic saves may only retain this authority or retire it.
    pub fn validate_unpublished_recovery_save(&self, next: &Self) -> Result<(), SandboxModelError> {
        let retiring = next.lifecycle.unpublished_recovery.is_none()
            && matches!(
                next.status,
                super::SandboxStatus::Retiring | super::SandboxStatus::Retired
            );
        if !retiring
            && (self.lifecycle.unpublished_recovery != next.lifecycle.unpublished_recovery
                || self.lifecycle.unpublished_recovery_attempted
                    != next.lifecycle.unpublished_recovery_attempted)
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(super) fn validate_unpublished_recovery_successor(
        &self,
        next: &Self,
    ) -> Result<(), SandboxModelError> {
        let finishes_incident = matches!(
            next.status,
            super::SandboxStatus::Retiring | super::SandboxStatus::Retired
        ) || (matches!(
            self.lifecycle.operation,
            Some(SandboxLifecycleOperation::ImageRotation {
                phase: SandboxImageRotationPhase::PublishActive,
                ..
            })
        ) && matches!(
            next.status,
            super::SandboxStatus::Active | super::SandboxStatus::Pausing
        ));
        if self.lifecycle.unpublished_recovery_attempted
            && !next.lifecycle.unpublished_recovery_attempted
            && !finishes_incident
        {
            return Err(invalid());
        }
        let Some(current) = &self.lifecycle.unpublished_recovery else {
            return Ok(());
        };
        match &next.lifecycle.unpublished_recovery {
            Some(after)
                if after.operation_id == current.operation_id
                    && after.proof == current.proof
                    && current
                        .quiescence
                        .as_ref()
                        .is_none_or(|receipt| after.quiescence.as_ref() == Some(receipt)) =>
            {
                Ok(())
            }
            None if finishes_incident
                || (current.quiescence.is_some()
                    && next.lifecycle.unpublished_recovery_attempted
                    && next
                        .runtime
                        .as_ref()
                        .and_then(|binding| binding.provision_generation)
                        .is_some_and(|generation| {
                            generation > current.proof.runtime_generation
                        })) =>
            {
                Ok(())
            }
            _ => Err(invalid()),
        }
    }

    /// Call before provider suspension or allocation; ordinary lifecycles pass.
    pub fn require_unpublished_runtime_quiescence(&self) -> Result<(), SandboxModelError> {
        if let Some(recovery) = &self.lifecycle.unpublished_recovery {
            recovery.validate_for(self)?;
            if recovery.quiescence.is_none() {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// Persist this as its own lifecycle transaction before any suspension I/O.
    pub fn complete_unpublished_runtime_quiescence(
        &mut self,
        receipt: SandboxUnpublishedRecoveryQuiescence,
        holder: &str,
        fence: u64,
        at: String,
    ) -> Result<(), SandboxModelError> {
        self.apply_claimed(holder, fence, at.clone(), at.clone(), |next| {
            if !matches!(
                next.lifecycle.operation,
                Some(SandboxLifecycleOperation::ImageRotation {
                    phase: SandboxImageRotationPhase::CheckpointVerification,
                    ..
                })
            ) || crate::seconds_from_rfc3339(&receipt.completed_at)
                > crate::seconds_from_rfc3339(&at)
            {
                return Err(invalid());
            }
            let recovery = next
                .lifecycle
                .unpublished_recovery
                .as_mut()
                .ok_or_else(invalid)?;
            recovery.validate_receipt(&receipt)?;
            if recovery
                .quiescence
                .as_ref()
                .is_some_and(|held| held != &receipt)
            {
                return Err(invalid());
            }
            recovery.quiescence = Some(receipt);
            Ok(())
        })
    }
}

const fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.unpublished_runtime_recovery.quiescence")
}
