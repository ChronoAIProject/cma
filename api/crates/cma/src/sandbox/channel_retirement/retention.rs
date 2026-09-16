//! A retained storage inventory is not authority to delete or publish Retired.
use super::model::{ChannelRetirementSnapshot, RetirementPhase};
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::{
    bridge::allocation::AllocationError,
    resource_access::{ResourceActor, ResourceOwner},
    sandbox::{RuntimeBinding, SandboxCheckpointHistory, SandboxHome, SandboxWorkStep},
    SandboxId, SubjectId,
};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_RETAINED_ARCHIVES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StorageDisposition {
    RetainForRecovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedArchive {
    pub manifest_id: String,
    pub object_reference: String,
    pub object_key: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChannelStorageRetention {
    pub schema_version: u16,
    pub allocation_id: String,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub owner: ResourceOwner,
    pub creation_approver: ResourceActor,
    pub original_human: ResourceActor,
    pub original_human_subject: SubjectId,
    pub tree_fingerprint: String,
    pub runtime: RuntimeBinding,
    /// Observed exact storage identity, retained for later ownership review.
    /// It does not supply the missing pre-deletion intent for physical cleanup.
    pub home: Option<Box<SandboxHome>>,
    pub checkpoints: SandboxCheckpointHistory,
    pub archives: Vec<RetainedArchive>,
    /// Includes resource tree, command/control/reply/source identities, logs,
    /// checkpoint metadata, idempotency tombstones and all existing audits.
    pub disposition: StorageDisposition,
    pub recorded_at: String,
}

impl ChannelStorageRetention {
    pub fn capture(
        snapshot: &ChannelRetirementSnapshot,
        human: &BridgeControlPlaneLease,
        archives: Vec<RetainedArchive>,
        recorded_at: String,
    ) -> Result<Self, AllocationError> {
        let state = snapshot.state.as_ref().ok_or(AllocationError::Corrupt)?;
        let value = Self {
            schema_version: 1,
            allocation_id: snapshot.allocation.id.clone(),
            sandbox_id: snapshot.sandbox.id.clone(),
            sandbox_created_at: snapshot.sandbox.created_at.clone(),
            owner: snapshot.allocation.owner.clone(),
            creation_approver: snapshot.creation.approved_by.clone(),
            original_human: human.actor_context().actor.clone(),
            original_human_subject: human.actor_subject().clone(),
            tree_fingerprint: snapshot.tree.fingerprint.clone(),
            runtime: state.runtime.clone().ok_or(AllocationError::Corrupt)?,
            home: snapshot.sandbox.lifecycle.home.clone(),
            checkpoints: snapshot.sandbox.lifecycle.checkpoints.clone(),
            archives,
            disposition: StorageDisposition::RetainForRecovery,
            recorded_at,
        };
        value.validate(snapshot)?;
        value.require_human(human)?;
        Ok(value)
    }

    pub fn validate(&self, snapshot: &ChannelRetirementSnapshot) -> Result<(), AllocationError> {
        let sandbox = &snapshot.sandbox;
        let state = snapshot.state.as_ref().ok_or(AllocationError::Corrupt)?;
        if self.schema_version != 1
            || state.phase != RetirementPhase::RuntimeGone
            || sandbox.work.step != SandboxWorkStep::MarkRetired
            || sandbox.runtime.is_some()
            || sandbox.retired_at.is_some()
            || sandbox.purge_after.is_some()
            || self.allocation_id != snapshot.allocation.id
            || self.sandbox_id != sandbox.id
            || self.sandbox_created_at != sandbox.created_at
            || self.owner != snapshot.allocation.owner
            || self.creation_approver != snapshot.creation.approved_by
            || direct_subject(&self.original_human) != Some(&self.original_human_subject)
            || direct_subject(&self.creation_approver) != Some(&self.original_human_subject)
            || self.tree_fingerprint != snapshot.tree.fingerprint
            || state.runtime.as_ref() != Some(&self.runtime)
            || self.home != sandbox.lifecycle.home
            || self.checkpoints != sandbox.lifecycle.checkpoints
            || self.archives.len() > MAX_RETAINED_ARCHIVES
            || layer2_core::seconds_from_rfc3339(&self.recorded_at).is_none()
            || self.recorded_at < sandbox.created_at
        {
            return Err(AllocationError::IdentityMismatch);
        }
        match (&snapshot.provisioning.settings.home_policy, &self.home) {
            (None, None) => (),
            (Some(policy), Some(home))
                if home.validate(&sandbox.id).is_ok()
                    && home.request.policy == *policy
                    && home.attachment().is_some() => {}
            _ => return Err(AllocationError::IdentityMismatch),
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, AllocationError> {
        layer2_core::profiles::content_digest(&("cma.channel.storage-retention.v1", self))
            .map_err(|_| AllocationError::Corrupt)
    }

    /// Called only after the current original grant and custody have been fenced.
    pub fn require_human(&self, human: &BridgeControlPlaneLease) -> Result<(), AllocationError> {
        if self.original_human != human.actor_context().actor
            || self.original_human_subject != *human.actor_subject()
            || self.original_human_subject != human.grant().subject
        {
            return Err(AllocationError::IdentityMismatch);
        }
        Ok(())
    }
}

// A renewable human lease can carry the API actor form. Retain that exact
// actor, and compare its direct subject with the separately retained approver.
fn direct_subject(actor: &ResourceActor) -> Option<&layer2_core::SubjectId> {
    match actor {
        ResourceActor::Human { subject }
        | ResourceActor::Api {
            subject,
            delegation_id: None,
        } => Some(subject),
        _ => None,
    }
}
