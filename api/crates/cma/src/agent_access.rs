//! Owner-only consent admission. Runtime tools can propose but cannot approve.
use crate::store::MongoSandboxStore;
use crate::trigger::{CmaServiceConnectionBroker, GITHUB_SERVICE};
use layer2_core::resource_access::{CallerContext, CredentialSource, ResourceActor};
use layer2_core::sandbox::agent_access::{
    AgentAccessError, AgentAccessRequest, AgentAccessRequests,
};
use layer2_core::sandbox::agent_access_tool::{
    AgentAccessProposal, AgentAccessRequester, AgentAccessToolAdmission,
};
use layer2_core::sandbox::agent_io::{
    AgentAuthorizationRevision, CommandAuthorizationError, PersonalCommandAdmissionAuthority,
};
use layer2_core::sandbox::{SandboxPolicySource, ServiceConnectionSelection};
use layer2_core::{AgentId, Caller, Capability, ServiceId};
use std::sync::Arc;

pub(crate) struct AgentAccessService {
    store: Arc<MongoSandboxStore>,
    authority: Arc<dyn PersonalCommandAdmissionAuthority>,
    policy: Arc<dyn SandboxPolicySource>,
    selection_broker: Option<Arc<CmaServiceConnectionBroker>>,
}
impl AgentAccessService {
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        authority: Arc<dyn PersonalCommandAdmissionAuthority>,
        policy: Arc<dyn SandboxPolicySource>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            authority,
            policy,
            selection_broker: None,
        })
    }

    pub(crate) fn with_connection_selection(
        mut self: Arc<Self>,
        broker: Option<Arc<CmaServiceConnectionBroker>>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("before sharing")
            .selection_broker = broker;
        self
    }
}
#[async_trait::async_trait]
impl AgentAccessToolAdmission for AgentAccessService {
    async fn request(
        &self,
        requester: AgentAccessRequester,
        proposal: AgentAccessProposal,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        Ok(self
            .store
            .request_agent_access(requester, proposal)
            .await?
            .view())
    }
}
#[async_trait::async_trait]
impl AgentAccessRequests for AgentAccessService {
    async fn list(
        &self,
        caller: &Caller,
        agent: &AgentId,
    ) -> Result<Vec<AgentAccessRequest>, AgentAccessError> {
        require_owner_capability(caller)?;
        Ok(self
            .store
            .list_agent_access_requests(&caller.subject, agent)
            .await?
            .iter()
            .map(|value| value.view())
            .collect())
    }
    #[allow(clippy::too_many_arguments)]
    async fn approve(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        request: &str,
        services: Vec<String>,
        connection: Option<ServiceConnectionSelection>,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        let authorization =
            retained_owner_authority(context, credential, self.authority.as_ref()).await?;
        let service_ids: Vec<_> = services.iter().cloned().map(ServiceId::new).collect();
        let other_services = if let Some(selection) = &connection {
            let broker = self
                .selection_broker
                .as_ref()
                .ok_or(AgentAccessError::Invalid)?;
            verify_github_selection(broker, credential, selection, &service_ids).await?;
            services
                .into_iter()
                .filter(|service| service != GITHUB_SERVICE)
                .collect()
        } else {
            services
        };
        if connection.is_none() || !other_services.is_empty() {
            self.authority
                .confirm_owner_service_scope(context, credential, &other_services)
                .await?;
        }
        let baseline = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .baseline_services;
        let record = self
            .store
            .approve_agent_access_with_connection(
                &context.caller.subject,
                agent,
                request,
                service_ids,
                connection,
                &baseline,
                authorization,
                key,
            )
            .await?;
        self.store.access_admitted.notify_one();
        Ok(record.view())
    }
    async fn replace_services(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        services: Vec<String>,
        expected_services: Vec<String>,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        let authorization =
            retained_owner_authority(context, credential, self.authority.as_ref()).await?;
        self.authority
            .confirm_owner_service_scope(context, credential, &services)
            .await?;
        let baseline = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .baseline_services;
        let record = self
            .store
            .replace_agent_service_access(
                &context.caller.subject,
                agent,
                services.into_iter().map(ServiceId::new).collect(),
                expected_services.into_iter().map(ServiceId::new).collect(),
                &baseline,
                authorization,
                key,
            )
            .await?;
        self.store.access_admitted.notify_one();
        Ok(record.view())
    }
    async fn deny(
        &self,
        caller: &Caller,
        agent: &AgentId,
        request: &str,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        require_owner_capability(caller)?;
        Ok(self
            .store
            .deny_agent_access(&caller.subject, agent, request, key)
            .await?
            .view())
    }
}
pub(crate) async fn verify_github_selection(
    broker: &CmaServiceConnectionBroker,
    credential: &str,
    selection: &ServiceConnectionSelection,
    services: &[ServiceId],
) -> Result<(), AgentAccessError> {
    use crate::auth::persistence::SecretValue;
    use layer2_core::trigger_domain::ServiceConnectionError;
    if selection.service.as_str() != GITHUB_SERVICE || !selection.matches_scope(services) {
        return Err(AgentAccessError::Invalid);
    }
    broker
        .verify_github_repo_token(
            &SecretValue::new(credential.to_owned()),
            &selection.connection_id,
        )
        .await
        .map(|_| ())
        .map_err(|error| match error {
            ServiceConnectionError::Refused | ServiceConnectionError::NotFound => {
                AgentAccessError::ServiceConnectionRequired
            }
            _ => AgentAccessError::Unavailable,
        })
}

async fn retained_owner_authority(
    context: &CallerContext,
    credential: &str,
    authority: &dyn PersonalCommandAdmissionAuthority,
) -> Result<AgentAuthorizationRevision, AgentAccessError> {
    require_owner_capability(&context.caller)?;
    // Both transports require direct owner attribution and exact custody.
    if !context.validate_principal()
        || !matches!(
            context.credential_source,
            CredentialSource::Retained
                | CredentialSource::Independent {
                    expires_at: Some(_)
                }
        )
        || !matches!(
            &context.actor,
            ResourceActor::Human { subject }
                | ResourceActor::Api { subject, delegation_id: None }
                if subject == &context.caller.subject
        )
    {
        return Err(AgentAccessError::Forbidden);
    }
    authority
        .owner_service_access_authority(context, credential)
        .await
        .map_err(|error| match error {
            CommandAuthorizationError::Revoked => {
                if matches!(
                    context.credential_source,
                    CredentialSource::Independent { .. }
                ) {
                    AgentAccessError::OwnerAuthorityRequired
                } else {
                    AgentAccessError::Forbidden
                }
            }
            CommandAuthorizationError::Unavailable => AgentAccessError::Unavailable,
        })
}
fn require_owner_capability(caller: &Caller) -> Result<(), AgentAccessError> {
    caller
        .authorization
        .allows(Capability::UseOwnSandboxes)
        .then_some(())
        .ok_or(AgentAccessError::Forbidden)
}

#[cfg(test)]
#[path = "agent_access_tests.rs"]
mod tests;
