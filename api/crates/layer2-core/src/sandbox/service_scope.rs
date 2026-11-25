//! Exact owner scope revisions retain runtime, home and conversation identity.
use crate::{seconds_from_rfc3339, ServiceId, SubjectId};

use super::credential::ServiceConnectionSelection;
use super::{
    Sandbox, SandboxModelError, SandboxServiceScopeRevision, SandboxStatus, SandboxWorkStep,
};

impl Sandbox {
    pub(super) fn validate_service_scope(&self) -> Result<(), SandboxModelError> {
        let Some(scope) = &self.credential.service_scope else {
            return Ok(());
        };
        if !self.credential.source.is_managed()
            || scope.revision == 0
            || scope.authorized_by != self.owner
            || !valid_services(&scope.services)
            || scope
                .connection
                .as_ref()
                .is_some_and(|connection| !connection.matches_scope(&scope.services))
            || !scope.services.windows(2).all(|pair| pair[0] < pair[1])
            || !super::lifecycle_execution_support::valid_runtime_binding(
                &scope.runtime,
                &scope.runtime.bound_at,
            )
            || scope
                .runtime
                .provision_generation
                .is_none_or(|generation| generation == 0)
            || !scope
                .runtime_services
                .windows(2)
                .all(|pair| pair[0] < pair[1])
            || scope.runtime_services.iter().any(|service| {
                let value = service.as_str();
                value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
            })
            || seconds_from_rfc3339(&scope.runtime.bound_at)
                .zip(seconds_from_rfc3339(&scope.authorized_at))
                .is_none_or(|(bound, authorized)| bound > authorized)
            || seconds_from_rfc3339(&scope.authorized_at)
                .zip(seconds_from_rfc3339(&self.created_at))
                .zip(seconds_from_rfc3339(&self.updated_at))
                .is_none_or(|((at, created), updated)| at < created || at > updated)
            || scope.credential_generation_floor == 0
            || scope.credential_generation_floor > self.credential.key_generation
            || self.credential.service_grants.last().is_some_and(|grant| {
                grant.credential_generation_floor >= scope.credential_generation_floor
            })
        {
            return Err(invalid());
        }
        Ok(())
    }

    /// Admit the exact owner-authorized scope and a new credential delivery.
    /// The owner service validates its mandatory baseline before this operation.
    pub fn revise_live_service_access(
        &mut self,
        owner: &SubjectId,
        services: &[ServiceId],
        at: String,
    ) -> Result<SandboxServiceScopeRevision, SandboxModelError> {
        self.revise_live_service_access_with_connection(owner, services, None, at)
    }

    /// Adopt an explicit connection choice in the same revision that fences its
    /// credential issuance. A later scope edit preserves a retained choice
    /// until the owner changes it or removes that service.
    pub fn revise_live_service_access_with_connection(
        &mut self,
        owner: &SubjectId,
        services: &[ServiceId],
        connection: Option<ServiceConnectionSelection>,
        at: String,
    ) -> Result<SandboxServiceScopeRevision, SandboxModelError> {
        self.validate()?;
        if owner != &self.owner
            || !self.credential.source.is_managed()
            || !valid_services(services)
            || connection
                .as_ref()
                .is_some_and(|selection| !selection.matches_scope(services))
            || self.status != SandboxStatus::Active
            || self.runtime.as_ref().is_none_or(|runtime| {
                runtime
                    .provision_generation
                    .is_none_or(|generation| generation == 0)
            })
            || !matches!(
                self.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
            || self.work.lease.is_some()
            || self.lifecycle.operation.is_some()
            || self.lifecycle.background_checkpoint.is_some()
            || self.lifecycle.checkpoint_capture.is_some()
            || !self.credential.delivered
            || self.credential.current.is_none()
            || self.needs_service_access_credential()
            || self.credential.pending.is_some()
            || !self.credential.revoke_debt.is_empty()
            || seconds_from_rfc3339(&at)
                .zip(seconds_from_rfc3339(&self.updated_at))
                .is_none_or(|(at, updated)| at < updated)
        {
            return Err(invalid());
        }
        let mut services = services.to_vec();
        services.sort();
        if services.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(invalid());
        }
        let connection = connection.or_else(|| {
            self.credential
                .selected_connection()
                .filter(|selection| selection.matches_scope(&services))
                .cloned()
        });
        let runtime = self.runtime.as_ref().ok_or_else(invalid)?;
        let scope = SandboxServiceScopeRevision {
            revision: self
                .credential
                .service_scope
                .as_ref()
                .map_or(0, |scope| scope.revision)
                .checked_add(1)
                .ok_or_else(invalid)?,
            services,
            connection,
            authorized_by: owner.clone(),
            authorized_at: at.clone(),
            // Skip a generation any expired pre-revision worker may mint.
            credential_generation_floor: self.credential.next_generation().ok_or_else(invalid)?,
            runtime: runtime.clone(),
            runtime_services: self.credential.runtime_reaches(runtime),
        };
        let mut next = self.clone();
        next.credential.key_generation = scope.credential_generation_floor;
        next.credential.next_generation().ok_or_else(invalid)?;
        next.credential.service_scope = Some(scope.clone());
        next.work
            .schedule(SandboxWorkStep::WaitForCredentialRotation, at.clone());
        next.revision = next.revision.checked_add(1).ok_or_else(invalid)?;
        next.updated_at = at;
        next.validate()?;
        *self = next;
        Ok(scope)
    }
}

fn valid_services(services: &[ServiceId]) -> bool {
    !services.is_empty()
        && services.len() <= 32
        && services.iter().all(|service| {
            let value = service.as_str();
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.service_scope")
}
