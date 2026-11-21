//! Immutable original request attribution. This is historical evidence, never
//! reusable Caller authority; runtime work retains its own Trigger identity.
use super::{Launch, LaunchServiceError, TriggerError, TriggerId};
use crate::resource_access::{CallerContext, ResourceActor};
use crate::{LaunchId, SubjectId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchInitiation {
    schema_version: u16,
    launch_id: LaunchId,
    trigger_id: TriggerId,
    owner: SubjectId,
    created_at: String,
    command_fingerprint: String,
    actor: ResourceActor,
}

impl LaunchInitiation {
    pub(super) fn capture(
        context: &CallerContext,
        launch: &Launch,
    ) -> Result<Self, LaunchServiceError> {
        if !context.validate_principal()
            || context.caller.subject != launch.owner
            || launch.revision != 1
            || launch.event_head != 0
            || launch.initiation.is_some()
        {
            return Err(LaunchServiceError::Forbidden);
        }
        let evidence = Self {
            schema_version: 1,
            launch_id: launch.id.clone(),
            trigger_id: launch.trigger_id.clone(),
            owner: launch.owner.clone(),
            created_at: launch.created_at.clone(),
            command_fingerprint: launch.command_fingerprint.clone(),
            actor: context.actor.clone(),
        };
        evidence
            .require_launch(launch)
            .map_err(|_| LaunchServiceError::Forbidden)?;
        Ok(evidence)
    }

    pub fn actor(&self) -> &ResourceActor {
        &self.actor
    }
    pub fn launch_id(&self) -> &LaunchId {
        &self.launch_id
    }
    pub fn trigger_id(&self) -> &TriggerId {
        &self.trigger_id
    }
    pub fn require_launch(&self, launch: &Launch) -> Result<(), TriggerError> {
        if self.schema_version != 1
            || self.launch_id != launch.id
            || self.trigger_id != launch.trigger_id
            || self.owner != launch.owner
            || self.created_at != launch.created_at
            || self.command_fingerprint != launch.command_fingerprint
            || !self.valid_actor()
        {
            return Err(TriggerError::new(
                "invalid_launch_initiation",
                "launch_initiation",
            ));
        }
        Ok(())
    }

    pub fn require_agent_admission(
        &self,
        admission: &crate::sandbox::store::AgentAdmission,
    ) -> Result<(), crate::sandbox::store::SandboxStoreError> {
        if self.schema_version != 1
            || crate::seconds_from_rfc3339(&self.created_at)
                .zip(crate::seconds_from_rfc3339(&admission.event_at))
                .is_none_or(|(created, admitted)| created > admitted)
            || !super::requirements::valid_digest(&self.command_fingerprint)
            || !self.launch_id.as_str().starts_with("lch_")
            || !self.trigger_id.as_str().starts_with("trg_")
            || !self.valid_actor()
            || self.owner != admission.owner
            || !matches!(&admission.origin, crate::sandbox::AgentOrigin::Trigger { trigger_id, launch_id }
                if trigger_id == self.trigger_id.as_str() && launch_id == self.launch_id.as_str())
            || !matches!(&admission.created_by, Some(ResourceActor::Trigger { subject, launch_id })
                if subject == &self.owner && launch_id == &self.launch_id)
        {
            return Err(crate::sandbox::store::SandboxStoreError::Conflict);
        }
        Ok(())
    }

    fn valid_actor(&self) -> bool {
        matches!(&self.actor,
            ResourceActor::Human { subject } | ResourceActor::Api { subject, delegation_id: None }
            | ResourceActor::Trigger { subject, .. } if subject == &self.owner)
    }
}
