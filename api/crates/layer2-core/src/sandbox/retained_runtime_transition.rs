//! The frozen, generation-reserving plan for an explicitly admitted retained home.

use layer3_adapter::RetainedStartupPolicyPredecessor;
use serde::{Deserialize, Serialize};

use super::{
    runtime_update_agents_are_idle, Agent, Sandbox, SandboxLifecycleOperation, SandboxModelError,
    SandboxRotationIdentity, SandboxRuntimeUpdateRequest, SandboxRuntimeUpdateState, SandboxStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedRuntimeTransitionRequest {
    pub sandbox_id: crate::SandboxId,
    pub owner: crate::SubjectId,
    pub expected_revision: u64,
    pub grant_id: String,
    pub policy_revision: u64,
    pub predecessor: RetainedStartupPolicyPredecessor,
    pub target_image: String,
    pub journal_recovery: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedRuntimeTransition {
    pub request: RetainedRuntimeTransitionRequest,
    pub successor_generation: u64,
    pub operation_id: String,
}

impl RetainedRuntimeTransition {
    pub(super) fn validate_for(
        &self,
        sandbox: &Sandbox,
        update: &SandboxRuntimeUpdateRequest,
    ) -> Result<(), SandboxModelError> {
        if self.request.sandbox_id != sandbox.id
            || self.request.owner != sandbox.owner
            || self.request.expected_revision >= sandbox.revision
            || update.automatic
            || update.started_at.is_none()
            || update.target_version != self.request.target_image
            || self.successor_generation <= self.request.predecessor.generation
            || self.successor_generation > sandbox.lifecycle.runtime_generation_high_water
        {
            return Err(invalid());
        }
        // Detached receipts can survive failure and retirement. They cannot
        // authorize another rotation or be interpreted as fresh admission.
        if update.completed_at.is_none()
            && sandbox.status == SandboxStatus::Rotating
            && (!matches!(
                &sandbox.lifecycle.operation,
                Some(SandboxLifecycleOperation::ImageRotation { authority, .. })
                    if authority.operation_id == self.operation_id
            ) || self.successor_generation != sandbox.lifecycle.runtime_generation_high_water)
        {
            return Err(invalid());
        }
        Ok(())
    }
}

impl Sandbox {
    pub fn admitted_retained_runtime_transition(&self) -> Option<&RetainedRuntimeTransition> {
        let authority = self.lifecycle.operation.as_ref()?.authority();
        let update = self
            .runtime_update_request()
            .filter(|request| request.completed_at.is_none())?;
        update
            .retained_transition
            .as_deref()
            .filter(|transition| transition.operation_id == authority.operation_id)
    }

    /// The store must validate and consume the scoped grant and fence the complete
    /// Agent and Workspace sets in the transaction that persists this successor.
    pub fn begin_retained_runtime_transition(
        &mut self,
        request: RetainedRuntimeTransitionRequest,
        agents: &[Agent],
        at: String,
    ) -> Result<RetainedRuntimeTransition, SandboxModelError> {
        self.validate()?;
        let binding = self
            .lifecycle
            .suspended_runtime
            .as_ref()
            .ok_or_else(invalid)?;
        let home = self
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .ok_or_else(invalid)?;
        let predecessor = &request.predecessor;
        if request.sandbox_id != self.id
            || request.owner != self.owner
            || request.expected_revision != self.revision
            || request.policy_revision == 0
            || !crate::operation::valid_idempotency_key(request.grant_id.as_bytes())
            || self.status != SandboxStatus::Paused
            || self.revive_eligibility().is_err()
            || !self.supports_retained_runtime_update()
            || self.lifecycle.operation.is_some()
            || self.lifecycle.unpublished_recovery.is_some()
            || self
                .runtime_update_request()
                .is_some_and(|request| request.completed_at.is_none())
            || !runtime_update_agents_are_idle(agents)
            || agents.iter().any(|agent| agent.owner != self.owner)
            || home
                .receipt
                .native_authority
                .as_ref()
                .is_none_or(|origin| origin.sandbox_uid != predecessor.uid)
            || home.identity().map_err(|_| invalid())? != predecessor.home_identity
            || binding.provision_generation != Some(predecessor.generation)
            || binding.runtime_version != predecessor.image
            || !crate::profiles::valid_runtime_image(&predecessor.image)
            || !super::store::valid_admission_fingerprint(&predecessor.request_sha256)
            || !valid_resource_version(&predecessor.resource_version)
            || request.target_image.len() > 512
            || !crate::profiles::valid_runtime_image(&request.target_image)
            || (predecessor.image == request.target_image
                && predecessor.journal_recovery == request.journal_recovery)
        {
            return Err(invalid());
        }

        let identity = SandboxRotationIdentity::capture(self, agents)?;
        let mut next = self.clone();
        next.begin_image_rotation(request.target_image.clone(), identity, at.clone())?;
        let operation = next
            .lifecycle
            .operation
            .as_ref()
            .ok_or_else(invalid)?
            .authority();
        let minimum = next_replacement_runtime_generation(operation.generation, agents)?;
        let successor_generation = self
            .lifecycle
            .runtime_generation_high_water
            .checked_add(1)
            .ok_or_else(invalid)?
            .max(minimum);
        let transition = RetainedRuntimeTransition {
            request,
            successor_generation,
            operation_id: operation.operation_id.clone(),
        };
        let target_version = transition.request.target_image.clone();
        let state =
            next.lifecycle
                .runtime_update
                .get_or_insert_with(|| SandboxRuntimeUpdateState {
                    checked_at: at.clone(),
                    observed_version: target_version.clone(),
                    request: None,
                });
        state.request = Some(SandboxRuntimeUpdateRequest {
            target_version,
            requested_at: at.clone(),
            automatic: false,
            started_at: Some(at),
            completed_at: None,
            failure: None,
            retained_transition: Some(Box::new(transition.clone())),
        });
        next.lifecycle.runtime_generation_high_water = successor_generation;
        next.validate()?;
        *self = next;
        Ok(transition)
    }
}

/// The reserved runtime generation must exceed existing Agent and operation authority.
pub fn next_replacement_runtime_generation(
    authority_generation: u64,
    agents: &[Agent],
) -> Result<u64, SandboxModelError> {
    let highest = agents
        .iter()
        .filter(|agent| agent.status.is_live())
        .filter_map(|agent| agent.conversation.runtime_generation)
        .fold(authority_generation, u64::max);
    highest.checked_add(1).ok_or_else(invalid)
}

fn valid_resource_version(version: &str) -> bool {
    !version.is_empty() && version.len() <= 512 && !version.chars().any(char::is_control)
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.retained_runtime_transition")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::lifecycle_test_support::NOW;
    use crate::sandbox::runtime_update_lifecycle_tests::{pause, step};
    use crate::sandbox::runtime_update_test_support::native;

    fn candidate() -> (Sandbox, RetainedRuntimeTransitionRequest) {
        let mut sandbox = native();
        let image = format!("registry.example/runtime@sha256:{}", "a".repeat(64));
        sandbox.runtime.as_mut().unwrap().runtime_version = image.clone();
        pause(&mut sandbox);
        let home = sandbox
            .lifecycle
            .home
            .as_ref()
            .unwrap()
            .attachment()
            .unwrap();
        let request = RetainedRuntimeTransitionRequest {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            expected_revision: sandbox.revision,
            grant_id: "retained-grant".into(),
            policy_revision: 1,
            predecessor: RetainedStartupPolicyPredecessor {
                uid: home
                    .receipt
                    .native_authority
                    .as_ref()
                    .unwrap()
                    .sandbox_uid
                    .clone(),
                resource_version: "17".into(),
                request_sha256: "2".repeat(64),
                home_identity: home.identity().unwrap(),
                image: image.clone(),
                generation: 1,
                journal_recovery: false,
            },
            target_image: image,
            journal_recovery: true,
        };
        (sandbox, request)
    }

    #[test]
    fn retained_transition_reserves_same_image_policy_change_once() {
        let (mut sandbox, request) = candidate();
        let original = sandbox.clone();
        let admitted = sandbox
            .begin_retained_runtime_transition(request.clone(), &[], NOW.into())
            .unwrap();
        assert_eq!(admitted.request, request);
        assert_eq!(sandbox.status, SandboxStatus::Rotating);
        assert_eq!(
            sandbox.lifecycle.runtime_generation_high_water,
            admitted.successor_generation
        );
        assert_eq!(sandbox.lifecycle.home, original.lifecycle.home);
        assert_eq!(
            sandbox.lifecycle.suspended_runtime,
            original.lifecycle.suspended_runtime
        );
        assert_eq!(
            sandbox.admitted_retained_runtime_transition(),
            Some(&admitted)
        );
        assert!(sandbox
            .request_runtime_update(request.target_image, false, NOW.into())
            .is_err());

        let source = sandbox.retained_home_recovery().unwrap().clone();
        step(&mut sandbox, |sandbox, fence| {
            sandbox.complete_lifecycle_home_verification(
                &source,
                "runtime-update-test",
                fence,
                NOW.into(),
            )
        });
        step(&mut sandbox, |sandbox, fence| {
            assert!(sandbox
                .reserve_lifecycle_runtime_generation(
                    admitted.successor_generation + 1,
                    "runtime-update-test",
                    fence,
                    NOW.into(),
                )
                .is_err());
            let generation = sandbox.reserve_lifecycle_runtime_generation(
                admitted.successor_generation,
                "runtime-update-test",
                fence,
                NOW.into(),
            )?;
            assert_eq!(generation, admitted.successor_generation);
            Ok(())
        });
        assert_eq!(
            sandbox.admitted_retained_runtime_transition(),
            Some(&admitted)
        );
    }

    #[test]
    fn retained_transition_refuses_stale_or_foreign_predecessors() {
        let (sandbox, request) = candidate();
        for change in [
            "owner",
            "home",
            "revision",
            "generation",
            "member",
            "no_change",
        ] {
            let mut next = sandbox.clone();
            let mut request = request.clone();
            match change {
                "owner" => request.owner = crate::SubjectId("other".into()),
                "home" => request.predecessor.home_identity = "other".into(),
                "revision" => request.expected_revision -= 1,
                "generation" => request.predecessor.generation += 1,
                "member" => request.predecessor.uid = "other".into(),
                "no_change" => request.journal_recovery = false,
                _ => unreachable!(),
            }
            assert!(
                next.begin_retained_runtime_transition(request, &[], NOW.into())
                    .is_err(),
                "{change}"
            );
            assert_eq!(next, sandbox);
        }
    }
}
