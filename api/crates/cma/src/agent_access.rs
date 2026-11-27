//! Owner-only consent admission. Runtime tools can propose but cannot approve.
use crate::store::{agent_access_requests::ConnectionApprovalBinding, MongoSandboxStore};
use crate::trigger::GITHUB_SERVICE;
use layer2_core::resource_access::{CallerContext, CredentialSource, ResourceActor};
use layer2_core::sandbox::agent_access::{
    AgentAccessError, AgentAccessRequest, AgentAccessRequests, GithubConnectionApproval,
};
use layer2_core::sandbox::agent_access_tool::{
    AgentAccessProposal, AgentAccessRequester, AgentAccessToolAdmission,
};
use layer2_core::sandbox::agent_io::{
    AgentAuthorizationRevision, CommandAuthorizationError, PersonalCommandAdmissionAuthority,
};
use layer2_core::sandbox::{SandboxPolicySource, ServiceConnectionSelection};
use layer2_core::trigger_domain::ServiceConnectionError;
use layer2_core::{AgentId, Caller, Capability, ServiceId};
use std::sync::Arc;

pub(crate) struct AgentAccessService {
    store: Arc<MongoSandboxStore>,
    authority: Arc<dyn PersonalCommandAdmissionAuthority>,
    policy: Arc<dyn SandboxPolicySource>,
    github_approval: Option<GithubApprovalProvider>,
}

struct GithubApprovalProvider {
    auth: Arc<crate::auth::AuthState>,
    broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
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
            github_approval: None,
        })
    }

    pub(crate) fn with_github_approval(
        mut self: Arc<Self>,
        auth: Arc<crate::auth::AuthState>,
        broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("approval configuration precedes request service sharing")
            .github_approval = Some(GithubApprovalProvider { auth, broker });
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
        let mut requests: Vec<_> = self
            .store
            .list_agent_access_requests(&caller.subject, agent)
            .await?
            .iter()
            .map(|value| value.view())
            .collect();
        let can_choose = |request: &AgentAccessRequest| {
            request.status == layer2_core::sandbox::agent_access::AgentAccessStatus::Pending
                && request
                    .services
                    .iter()
                    .any(|service| service == GITHUB_SERVICE)
        };
        if self.github_approval.is_some() && requests.iter().any(can_choose) {
            let context = self
                .store
                .github_approval_context(&caller.subject, agent)
                .await?;
            for request in requests.iter_mut().filter(|request| can_choose(request)) {
                request.github_approval = Some(context.clone());
            }
        }
        Ok(requests)
    }
    async fn approve(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        request: &str,
        services: Vec<String>,
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
            .approve_agent_access(
                &context.caller.subject,
                agent,
                request,
                services.into_iter().map(ServiceId::new).collect(),
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
    async fn approve_github_connection(
        &self,
        context: &CallerContext,
        credential: &str,
        agent: &AgentId,
        request: &str,
        mut services: Vec<String>,
        choice: GithubConnectionApproval,
        key: &str,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        let provider = self
            .github_approval
            .as_ref()
            .ok_or(AgentAccessError::Unavailable)?;
        let authorization =
            retained_owner_authority(context, credential, self.authority.as_ref()).await?;
        let owner = &context.caller.subject;
        services.sort();
        let scope: Vec<_> = services.iter().cloned().map(ServiceId::new).collect();
        let connection = ServiceConnectionSelection {
            service: ServiceId::new(GITHUB_SERVICE),
            connection_id: choice.user_service_id,
        };
        if !connection.matches_scope(&scope) || services.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(AgentAccessError::Invalid);
        }
        let binding = ConnectionApprovalBinding {
            sandbox_id: choice.sandbox_id,
            scope_revision: choice.scope_revision,
            credential_generation: choice.credential_generation,
        };
        if let Some(record) = self
            .store
            .github_approval_replay(owner, agent, request, &scope, &connection, &binding, key)
            .await?
        {
            return Ok(record.view());
        }
        let other_services: Vec<_> = services
            .iter()
            .filter(|service| service.as_str() != GITHUB_SERVICE)
            .cloned()
            .collect();
        if !other_services.is_empty() {
            self.authority
                .confirm_owner_service_scope(context, credential, &other_services)
                .await?;
        }
        let (_, token) = provider
            .auth
            .retained_owner_service_authority(owner, &authorization)
            .await
            .map_err(|error| match error {
                CommandAuthorizationError::Unavailable => AgentAccessError::Unavailable,
                CommandAuthorizationError::Revoked => AgentAccessError::OwnerAuthorityRequired,
            })?;
        provider
            .broker
            .verify_github_repo_token(&token, &connection.connection_id)
            .await
            .map_err(github_proof_error)?;
        let baseline = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .baseline_services;
        let record = self
            .store
            .approve_agent_access_bound(
                owner,
                agent,
                request,
                scope,
                Some(connection),
                Some(binding),
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
pub(crate) fn github_proof_error(error: ServiceConnectionError) -> AgentAccessError {
    match error {
        ServiceConnectionError::NotFound | ServiceConnectionError::Refused => {
            AgentAccessError::ServiceConnectionRequired
        }
        ServiceConnectionError::Unavailable(_) | ServiceConnectionError::Malformed(_) => {
            AgentAccessError::Unavailable
        }
    }
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
