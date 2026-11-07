//! Attach a channel without acquiring ownership of its existing Agent resources.
use super::*;
use crate::resource_access::{ResourceAccessStoreError, ResourceOwner, ResourceTarget};
use crate::sandbox::{AgentStatus, SandboxStatus, WorkspaceStatus, WorktreeStatus};
use crate::{now_rfc3339, AgentId, Caller};

impl BindingService {
    pub async fn put_existing_agent(
        &self,
        caller: &Caller,
        id: &str,
        profile: serde_json::Value,
        digest: &str,
        credential: &ProvidedCredential,
        agent_id: AgentId,
    ) -> Result<ProviderBinding, BindingError> {
        super::service::require(caller)?;
        self.credentials
            .validate(credential)
            .map_err(super::service::credential_error)?;
        if !super::model::valid_id(id, 1) || !super::model::valid_id(agent_id.as_str(), 1) {
            return Err(BindingError::InvalidRequest(
                "invalid binding or agent identifier".into(),
            ));
        }
        let mut plan = map_profile(
            profile,
            digest,
            self.policy.provider,
            &self.policy.supported_model_api,
            self.policy.bounds,
        )
        .map_err(|error| BindingError::InvalidRequest(error.to_string()))?;
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let snapshot = self
            .store
            .resource_access_snapshot(&target)
            .await
            .map_err(resource_error)?;
        let owner = if plan.organization_owned {
            ResourceOwner::Organization {
                subject: plan.owner.clone(),
            }
        } else {
            ResourceOwner::Person {
                subject: plan.owner.clone(),
            }
        };
        if snapshot.scope(&target).map_err(resource_error)?.owner != owner {
            return Err(BindingError::NotFound);
        }
        let lineage = &snapshot.lineage;
        let agent = lineage.agent.as_ref().ok_or(BindingError::NotFound)?;
        let workspace = lineage.workspace.as_ref().ok_or(BindingError::NotFound)?;
        let worktree = lineage.worktree.as_ref().ok_or(BindingError::NotFound)?;
        if agent.id != agent_id
            || agent.owner != plan.owner
            || lineage.sandbox.profile.content.agent_provider != self.policy.provider
            || !matches!(
                agent.status,
                AgentStatus::Ready
                    | AgentStatus::Working
                    | AgentStatus::WaitingApproval
                    | AgentStatus::Paused
            )
            || !matches!(
                lineage.sandbox.status,
                SandboxStatus::Active | SandboxStatus::Paused
            )
            || workspace.status != WorkspaceStatus::Ready
            || worktree.status != WorktreeStatus::Ready
        {
            return Err(BindingError::Conflict);
        }
        // Profile settings are never applied to a borrowed Agent. Its existing
        // model/tool credentials remain untouched, so only automatic replies
        // are promised for the newly attached channel.
        plan.environment = None;
        plan.session_affinity = SessionAffinity::Conversation;
        plan.agent = agent.profile.content.clone();
        plan.honoured = vec!["metadata.owner".into(), "target.existing_agent".into()];
        if plan.queue_lifetime_seconds.is_some() {
            plan.honoured.push("queue_lifetime_seconds".into());
        }
        plan.refused = vec![
            BindingFieldRefusal { field: "profile".into(), reason: "existing Agent configuration is retained".into() },
            BindingFieldRefusal { field: "tools.native_mcp_url".into(), reason: "existing Agent credentials are retained; this channel supports automatic final replies".into() },
        ];
        let borrowed = ExistingBindingAgent {
            agent_id,
            worktree_id: worktree.id.clone(),
        };
        for _ in 0..8 {
            let current = self.store.binding(id).await?;
            if let Some(value) = &current {
                if value.manager != caller.subject {
                    return Err(BindingError::NotFound);
                }
                if value.state != BindingState::Ready
                    || value.existing_agent.as_ref() != Some(&borrowed)
                    || value.owner != plan.owner
                    || value.resource_owner() != owner
                    || value.sandbox_id != lineage.sandbox.id
                    || value.workspace_id != workspace.id
                {
                    return Err(BindingError::Conflict);
                }
                if value.profile_digest != digest {
                    if value.profile.configuration_digest() != plan.configuration_digest()
                        || value.key_sha256 != credential.sha256()
                    {
                        return Err(BindingError::Conflict);
                    }
                    // A queue-policy update never refreshes or applies Agent settings.
                    let mut updated = value.profile.clone();
                    updated.queue_lifetime_seconds = plan.queue_lifetime_seconds;
                    updated.configuration_digest = plan.configuration_digest.clone();
                    updated.profile_digest = plan.profile_digest.clone();
                    updated.honoured = plan.honoured.clone();
                    let next = value
                        .reconfigure(updated, value.key_sha256.clone(), now_rfc3339())
                        .map_err(|_| BindingError::Conflict)?;
                    match self
                        .store
                        .save_binding(Some(value.revision), next.clone())
                        .await
                    {
                        Ok(()) => return Ok(next),
                        Err(crate::sandbox::store::SandboxStoreError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                if value.key_sha256 == credential.sha256() {
                    return Ok(value.clone());
                }
                let next = value
                    .rotate_credential(credential.sha256(), now_rfc3339())
                    .map_err(super::service::credential_error)?;
                match self
                    .store
                    .save_binding(Some(value.revision), next.clone())
                    .await
                {
                    Ok(()) => return Ok(next),
                    Err(crate::sandbox::store::SandboxStoreError::Conflict) => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            let mut next = ProviderBinding::new(
                id.into(),
                caller.subject.clone(),
                plan.clone(),
                credential.sha256(),
                now_rfc3339(),
            )
            .map_err(BindingError::InvalidRequest)?;
            next.existing_agent = Some(borrowed.clone());
            next.sandbox_id = lineage.sandbox.id.clone();
            next.workspace_id = workspace.id.clone();
            next.level = 1;
            next.state = BindingState::Ready;
            next.work = BindingWork::Idle;
            next.validate().map_err(BindingError::InvalidRequest)?;
            match self.store.save_binding(None, next.clone()).await {
                Ok(()) => return Ok(next),
                Err(crate::sandbox::store::SandboxStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(BindingError::Conflict)
    }
}

fn resource_error(error: ResourceAccessStoreError) -> BindingError {
    match error {
        ResourceAccessStoreError::NotFound => BindingError::NotFound,
        ResourceAccessStoreError::Unavailable => {
            BindingError::Unavailable("resource ownership store unavailable".into())
        }
        ResourceAccessStoreError::Corrupt => {
            BindingError::Corrupt("resource ownership evidence is inconsistent".into())
        }
        _ => BindingError::Conflict,
    }
}
