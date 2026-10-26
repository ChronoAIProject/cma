//! Closed aggregate predicates for launch-driven sandbox recovery.

use crate::profiles::ModelAccess;
use crate::seconds_from_rfc3339;

use super::{
    Sandbox, SandboxCredentialAuthority, SandboxLifecycleState, SandboxModelError, SandboxOrigin,
    SandboxSignInState, SandboxStatus, SandboxWorkState, SandboxWorkStep,
};

const MAX_LAUNCH_ID_BYTES: usize = 128;
const MAX_RUNTIME_VALUE_BYTES: usize = 512;
const MAX_FAILURE_DETAIL_BYTES: usize = 256;
const MAX_WORKER_HOLDER_BYTES: usize = 128;

impl Sandbox {
    pub(crate) fn repair_service_grant_failure(
        &mut self,
        launch_id: &str,
        requested_at: String,
    ) -> Result<(), SandboxModelError> {
        if !self.has_exact_launch_origin(launch_id)
            || !self.service_grant_failure_is_repairable()
            || seconds_from_rfc3339(&requested_at).is_none()
        {
            return Err(invalid_launch_recovery());
        }
        let mut next = self.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or_else(invalid_launch_recovery)?;
        next.status = SandboxStatus::Installing;
        next.failure = None;
        next.updated_at = requested_at.clone();
        next.work
            .schedule(SandboxWorkStep::MintInitialCredential, requested_at);
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub(crate) fn service_grant_failure_is_repairable(&self) -> bool {
        self.status == SandboxStatus::Failed
            && self.failure.as_ref().is_some_and(|failure| {
                failure.class == "service_not_granted"
                    && valid_safe_text(&failure.detail, MAX_FAILURE_DETAIL_BYTES)
                    && self
                        .credential
                        .reaches
                        .iter()
                        .filter(|service| service.as_str() == failure.detail)
                        .count()
                        == 1
            })
            && self.has_unchanged_initial_lifecycle()
            && self.has_valid_retained_runtime()
            && self.has_initial_credential_state()
            && self.work.step == SandboxWorkStep::None
            && self.work.lease.is_none()
            && self.work.attempts == 0
            && seconds_from_rfc3339(&self.work.ready_at).is_some()
    }

    pub(crate) fn is_post_install_repair_replay(&self) -> bool {
        self.has_unchanged_initial_lifecycle()
            && self.has_valid_retained_runtime()
            && self.has_structurally_valid_work()
            && match self.status {
                SandboxStatus::Installing => match self.work.step {
                    SandboxWorkStep::MintInitialCredential => self.has_initial_credential_state(),
                    SandboxWorkStep::DeliverInitialCredential => {
                        self.has_pending_initial_credential()
                    }
                    SandboxWorkStep::CreateProfileWorkspace(index) => {
                        usize::from(index) < self.profile.content.workspace_profiles.len()
                            && self.has_delivered_initial_credential()
                    }
                    SandboxWorkStep::Activate => self.has_delivered_initial_credential(),
                    _ => false,
                },
                SandboxStatus::Active => {
                    self.work.step == SandboxWorkStep::WaitForCredentialRotation
                        && self.work.attempts == 0
                        && self.has_current_credential()
                }
                _ => false,
            }
    }

    pub(crate) fn has_exact_launch_origin(&self, launch_id: &str) -> bool {
        valid_launch_id(launch_id)
            && matches!(
                &self.origin,
                SandboxOrigin::Trigger { launch_id: bound }
                    if valid_launch_id(bound) && bound == launch_id
            )
    }

    pub(crate) fn has_structurally_valid_origin(&self) -> bool {
        match &self.origin {
            SandboxOrigin::Manual => true,
            SandboxOrigin::Trigger { launch_id } => valid_launch_id(launch_id),
            SandboxOrigin::Lifecycle { operation_id } => valid_launch_id(operation_id),
        }
    }

    pub(crate) fn has_structurally_valid_work(&self) -> bool {
        valid_work(&self.work)
    }

    fn has_unchanged_initial_lifecycle(&self) -> bool {
        let runtime_generation_high_water = match self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.provision_generation)
        {
            None => 0,
            Some(1) => 1,
            Some(_) => return false,
        };
        self.lifecycle
            == SandboxLifecycleState {
                runtime_generation_high_water,
                ..SandboxLifecycleState::default()
            }
            && self.retired_at.is_none()
            && self.purge_after.is_none()
    }

    fn has_valid_retained_runtime(&self) -> bool {
        let Some(runtime) = &self.runtime else {
            return false;
        };
        valid_safe_text(&runtime.runtime_id, MAX_RUNTIME_VALUE_BYTES)
            && valid_safe_text(&runtime.runtime_version, MAX_RUNTIME_VALUE_BYTES)
            && seconds_from_rfc3339(&runtime.bound_at).is_some_and(|bound| {
                seconds_from_rfc3339(&self.updated_at).is_some_and(|updated| bound <= updated)
            })
    }

    fn has_initial_credential_state(&self) -> bool {
        !self.credential.reaches.is_empty()
            && self.credential.key_generation == 0
            && !self.credential.delivered
            && self.credential.current.is_none()
            && self.credential.pending.is_none()
            && self.credential.revoke_debt.is_empty()
            && self.sign_in_matches_profile()
    }

    fn has_pending_initial_credential(&self) -> bool {
        !self.credential.reaches.is_empty()
            && self.credential.key_generation == 1
            && !self.credential.delivered
            && self.credential.current.is_none()
            && self.credential.revoke_debt.is_empty()
            && self
                .credential
                .pending
                .as_ref()
                .is_some_and(|authority| self.valid_current_authority(authority, 1))
            && self.sign_in_matches_profile()
    }

    fn has_delivered_initial_credential(&self) -> bool {
        !self.credential.reaches.is_empty()
            && self.credential.key_generation == 1
            && self.credential.delivered
            && self.credential.pending.is_none()
            && self.credential.revoke_debt.is_empty()
            && self
                .credential
                .current
                .as_ref()
                .is_some_and(|authority| self.valid_current_authority(authority, 1))
            && self.sign_in_matches_profile()
    }

    fn has_current_credential(&self) -> bool {
        !self.credential.reaches.is_empty()
            && self.credential.key_generation > 0
            && self.credential.delivered
            && self.credential.pending.is_none()
            && self.credential.revoke_debt.is_empty()
            && self.credential.current.as_ref().is_some_and(|authority| {
                self.valid_current_authority(authority, self.credential.key_generation)
            })
            && self.sign_in_matches_profile()
    }

    fn valid_current_authority(
        &self,
        authority: &SandboxCredentialAuthority,
        generation: u64,
    ) -> bool {
        authority.generation == generation
            && valid_safe_text(&authority.provider_key_id, MAX_RUNTIME_VALUE_BYTES)
            && seconds_from_rfc3339(&authority.issued_at).is_some_and(|issued| {
                seconds_from_rfc3339(&self.updated_at).is_some_and(|updated| issued <= updated)
                    && seconds_from_rfc3339(&authority.expires_at)
                        .is_some_and(|expires| issued < expires)
            })
    }

    fn sign_in_matches_profile(&self) -> bool {
        matches!(
            (self.profile.content.model_access, self.credential.sign_in),
            (
                ModelAccess::ChronoaiManaged,
                SandboxSignInState::NotRequired
            ) | (
                ModelAccess::OwnCredentials,
                SandboxSignInState::Required | SandboxSignInState::Complete
            )
        )
    }
}

pub(crate) fn valid_launch_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_LAUNCH_ID_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_work(work: &SandboxWorkState) -> bool {
    seconds_from_rfc3339(&work.ready_at).is_some()
        && work.next_fence > 0
        && work.lease.as_ref().is_none_or(|lease| {
            valid_safe_text(&lease.holder, MAX_WORKER_HOLDER_BYTES)
                && lease.fence > 0
                && lease.fence < work.next_fence
                && seconds_from_rfc3339(&lease.expires_at).is_some()
        })
}

fn valid_safe_text(value: &str, maximum_bytes: usize) -> bool {
    value.trim() == value
        && !value.is_empty()
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
}

const fn invalid_launch_recovery() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.launch_recovery")
}
