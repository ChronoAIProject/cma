use std::collections::BTreeSet;

use crate::{seconds_from_rfc3339, ServiceId, SubjectId};

use super::{
    Sandbox, SandboxEventKind, SandboxModelError, SandboxServiceGrant, SandboxStatus,
    SandboxWorkStep,
};

const MAX_ADDITIONAL_SERVICES: usize = 32;

impl Sandbox {
    pub(super) fn validate_service_access(&self) -> Result<(), SandboxModelError> {
        self.validate_service_scope()?;
        if self.credential.service_grants.is_empty() {
            return Ok(());
        }
        let created = seconds_from_rfc3339(&self.created_at).ok_or_else(invalid_access)?;
        let updated = seconds_from_rfc3339(&self.updated_at).ok_or_else(invalid_access)?;
        let mut seen = self
            .credential
            .reaches
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut previous = created;
        let mut count = 0;
        let mut generation_floor = 0;
        for grant in &self.credential.service_grants {
            let at = seconds_from_rfc3339(&grant.authorized_at).ok_or_else(invalid_access)?;
            if grant.authorized_by != self.owner
                || at < previous
                || at > updated
                || grant.services.is_empty()
                || grant.credential_generation_floor <= generation_floor
                || grant.credential_generation_floor > self.credential.key_generation
                || !grant.services.windows(2).all(|pair| pair[0] < pair[1])
                || grant
                    .services
                    .iter()
                    .any(|service| !valid_service(service) || !seen.insert(service.clone()))
            {
                return Err(invalid_access());
            }
            previous = at;
            generation_floor = grant.credential_generation_floor;
            count += grant.services.len();
            if count > MAX_ADDITIONAL_SERVICES {
                return Err(invalid_access());
            }
        }
        Ok(())
    }

    /// Admission changes authority only between runtimes, never during owner work.
    pub fn grant_service_access(
        &mut self,
        owner: &SubjectId,
        services: &[ServiceId],
        at: String,
    ) -> Result<Option<SandboxServiceGrant>, SandboxModelError> {
        self.validate()?;
        if !self.credential.source.is_managed()
            || owner != &self.owner
            || services.is_empty()
            || services.len() > MAX_ADDITIONAL_SERVICES
            || services.iter().any(|service| !valid_service(service))
            || seconds_from_rfc3339(&at)
                .zip(seconds_from_rfc3339(&self.updated_at))
                .is_none_or(|(at, updated)| at < updated)
        {
            return Err(invalid_access());
        }
        let effective = self
            .credential
            .effective_reaches()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let additions = services
            .iter()
            .filter(|service| !effective.contains(*service))
            .cloned()
            .collect::<BTreeSet<_>>();
        if additions.is_empty() {
            return Ok(None);
        }
        if !self.service_access_is_settled() || self.credential.service_scope.is_some() {
            return Err(invalid_access());
        }
        let grant = SandboxServiceGrant {
            services: additions.into_iter().collect(),
            authorized_by: owner.clone(),
            authorized_at: at.clone(),
            credential_generation_floor: self
                .credential
                .next_generation()
                .ok_or_else(invalid_access)?,
        };
        let mut next = self.clone();
        next.credential.service_grants.push(grant.clone());
        // Skip material an expired pre-grant worker might still mint in custody.
        next.credential.key_generation = grant.credential_generation_floor;
        next.updated_at = at;
        next.revision = next.revision.checked_add(1).ok_or_else(invalid_access)?;
        next.validate()?;
        *self = next;
        Ok(Some(grant))
    }

    pub fn validate_service_access_successor(&self, next: &Self) -> Result<(), SandboxModelError> {
        if self.credential.source != next.credential.source
            || self.credential.reaches != next.credential.reaches
            || self.credential.service_grants != next.credential.service_grants
            || self.credential.service_scope != next.credential.service_scope
            || next.credential.key_generation < self.credential.key_generation
        {
            return Err(invalid_access());
        }
        Ok(())
    }

    pub(super) fn service_access_is_settled(&self) -> bool {
        self.status == SandboxStatus::Paused
            && self.runtime.is_none()
            && self.work.step == SandboxWorkStep::None
            && self.work.lease.is_none()
            && self.lifecycle.operation.is_none()
            && self.lifecycle.checkpoint_capture.is_none()
            && self.credential.pending.is_none()
    }

    pub fn validate_service_access_transaction(
        &self,
        next: &Self,
        event: &SandboxEventKind,
        at: &str,
    ) -> Result<(), SandboxModelError> {
        if let SandboxEventKind::ServiceAccessRevised { scope } = event {
            let mut expected = self.clone();
            let actual = expected.revise_live_service_access_with_connection(
                &scope.authorized_by,
                &scope.services,
                scope.connection.clone(),
                at.into(),
            )?;
            return if &actual == scope.as_ref() && &expected == next {
                Ok(())
            } else {
                Err(invalid_access())
            };
        }
        let SandboxEventKind::ServiceAccessGranted { grant } = event else {
            return self.validate_service_access_successor(next);
        };
        let mut expected = self.clone();
        let actual =
            expected.grant_service_access(&grant.authorized_by, &grant.services, at.into())?;
        if actual.as_ref() != Some(grant) || &expected != next {
            return Err(invalid_access());
        }
        Ok(())
    }
}

fn valid_service(service: &ServiceId) -> bool {
    let value = service.as_str();
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn invalid_access() -> SandboxModelError {
    SandboxModelError::invalid_value("sandbox.service_access")
}
