use crate::{now_rfc3339, Caller, Capability, SandboxId, ServiceId};

use super::service::{require, store_error};
use super::store::{SandboxStoreError, SandboxTransaction};
use super::{Sandbox, SandboxEventKind, SandboxService, SandboxServiceError};

impl SandboxService {
    /// Record explicit owner intent. Connected-account authority is still verified at minting.
    pub async fn grant_service_access(
        &self,
        caller: &Caller,
        id: &SandboxId,
        services: &[ServiceId],
    ) -> Result<Sandbox, SandboxServiceError> {
        require(caller, Capability::UseOwnSandboxes)?;
        let mut sandbox = self.owned(caller, id).await?;
        sandbox
            .validate()
            .map_err(|error| SandboxServiceError::Corrupt(error.to_string()))?;
        let effective = sandbox.credential.effective_reaches();
        if services.iter().any(|service| !effective.contains(service))
            && !sandbox.service_access_is_settled()
        {
            return Err(SandboxServiceError::ServiceAccessRequiresPause);
        }
        let revision = sandbox.revision;
        let at = now_rfc3339();
        let Some(grant) = sandbox
            .grant_service_access(&caller.subject, services, at.clone())
            .map_err(|_| SandboxServiceError::InvalidServiceAccess)?
        else {
            return Ok(sandbox);
        };
        match self
            .store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: revision,
                sandbox: Box::new(sandbox.clone()),
                event_at: at,
                event: SandboxEventKind::ServiceAccessGranted { grant },
            })
            .await
        {
            Ok(()) => Ok(sandbox),
            Err(SandboxStoreError::Conflict) => {
                let mut current = self.owned(caller, id).await?;
                if matches!(
                    current.grant_service_access(&caller.subject, services, now_rfc3339()),
                    Ok(None)
                ) {
                    Ok(current)
                } else {
                    Err(SandboxServiceError::Unavailable(
                        "sandbox changed while service access was requested".into(),
                    ))
                }
            }
            Err(error) => Err(store_error(error)),
        }
    }
}
