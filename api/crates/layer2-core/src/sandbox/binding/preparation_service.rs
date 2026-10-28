use super::{
    service::credential_error, BindingError, BindingPreparationIntent, BindingPreparationReceipt,
    BindingService, BindingState, ProvidedCredential,
};
use crate::{profiles::content_digest, Caller, SubjectId};

/// Authenticated Gateway admission facts. The Gateway creates these from its
/// committed eligible ordinary-event plan, never from producer-supplied opt-in JSON.
#[derive(Debug, Clone)]
pub struct BindingPreparationEvent {
    pub admission_id: String,
    pub target_id: String,
    pub request_hash: String,
    pub owner: SubjectId,
    pub admitted_at: i64,
    pub expires_at: i64,
}
impl BindingPreparationEvent {
    fn validate(&self, now: i64) -> Result<(), BindingError> {
        if !super::reprovision::bounded_id(&self.admission_id)
            || !super::reprovision::bounded_id(&self.target_id)
            || !super::reprovision::bounded_id(self.owner.as_str())
            || !super::reprovision::valid_hash(&self.request_hash)
            || self.admitted_at <= 0
            || self.admitted_at > now
            || self.expires_at <= now
            || self.expires_at.saturating_sub(self.admitted_at) > 7 * 86400
        {
            return Err(BindingError::InvalidRequest(
                "invalid fresh-event admission".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum BindingPreparationInspection {
    Current {
        binding_id: String,
        generation: u64,
    },
    Retired {
        intent: Box<BindingPreparationIntent>,
    },
}

impl BindingService {
    /// Read-only: no retry, credential rotation, reservation or ID allocation.
    pub async fn inspect_preparation(
        &self,
        caller: &Caller,
        id: &str,
        event: &BindingPreparationEvent,
        credential: &ProvidedCredential,
    ) -> Result<BindingPreparationInspection, BindingError> {
        let now = crate::now_rfc3339();
        event.validate(
            crate::seconds_from_rfc3339(&now)
                .and_then(|v| v.try_into().ok())
                .ok_or(BindingError::Conflict)?,
        )?;
        self.credentials
            .validate(credential)
            .map_err(credential_error)?;
        let binding = self.read_for_retirement(caller, id).await?;
        if binding.owner != event.owner || binding.key_sha256 != credential.sha256() {
            return Err(BindingError::NotFound);
        }
        if binding.existing_agent.is_some() || binding.state == BindingState::Retired {
            return Err(BindingError::Conflict);
        }
        if !matches!(binding.state, BindingState::Failed { .. }) {
            super::readiness::validate(self.store.as_ref(), &binding).await?;
            return Ok(BindingPreparationInspection::Current {
                binding_id: binding.binding_id,
                generation: binding.generation,
            });
        }
        let (sandbox, workspace) = tokio::try_join!(
            self.store.get_sandbox(&binding.sandbox_id),
            self.store.get_workspace(&binding.workspace_id),
        )?;
        let (Some(sandbox), Some(workspace)) = (sandbox, workspace) else {
            return Err(BindingError::ResourcesUnavailable);
        };
        if sandbox.id != binding.sandbox_id
            || sandbox.owner != binding.owner
            || workspace.id != binding.workspace_id
            || workspace.owner != binding.owner
            || workspace.sandbox_id != binding.sandbox_id
        {
            return Err(BindingError::Corrupt(
                "binding parent identity changed".into(),
            ));
        }
        // Recoverable originals remain on the existing background recovery path.
        if sandbox.status != crate::sandbox::SandboxStatus::Retired {
            return Ok(BindingPreparationInspection::Current {
                binding_id: binding.binding_id,
                generation: binding.generation,
            });
        }
        let intent = BindingPreparationIntent {
            version: 1,
            admission_id: event.admission_id.clone(),
            target_id: event.target_id.clone(),
            request_hash: event.request_hash.clone(),
            binding_id: binding.binding_id.clone(),
            owner: event.owner.clone(),
            manager: caller.subject.clone(),
            generation: binding.generation,
            binding_revision: binding.revision,
            binding_digest: content_digest(&binding).map_err(|_| BindingError::Conflict)?,
            sandbox_digest: content_digest(&sandbox).map_err(|_| BindingError::Conflict)?,
            workspace_digest: content_digest(&workspace).map_err(|_| BindingError::Conflict)?,
            admitted_at: event.admitted_at,
            expires_at: event.expires_at,
        };
        binding
            .require_retired_hierarchy(&intent, &sandbox, &workspace, credential, &now)
            .map_err(credential_error)?;
        Ok(BindingPreparationInspection::Retired {
            intent: Box::new(intent),
        })
    }

    /// The interface gates writer enablement. Current management authority is
    /// required even for exact replay of an immutable prior operation receipt.
    pub async fn prepare_retired_binding(
        &self,
        caller: &Caller,
        id: &str,
        intent: &BindingPreparationIntent,
        credential: &ProvidedCredential,
    ) -> Result<BindingPreparationReceipt, BindingError> {
        intent.validate().map_err(credential_error)?;
        self.credentials
            .validate(credential)
            .map_err(credential_error)?;
        let current = self.read_for_retirement(caller, id).await?;
        if intent.binding_id != id
            || intent.manager != caller.subject
            || intent.owner != current.owner
            || credential.sha256() != current.key_sha256
            || current.existing_agent.is_some()
            || current.state == BindingState::Retired
        {
            return Err(BindingError::NotFound);
        }
        self.credentials
            .prepare_retired_binding(intent, credential)
            .await
            .map_err(credential_error)
    }
}
