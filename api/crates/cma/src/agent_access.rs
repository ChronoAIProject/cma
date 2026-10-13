//! Owner-only consent admission. Runtime tools can propose but cannot approve.
use crate::store::MongoSandboxStore;
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
use layer2_core::sandbox::SandboxPolicySource;
use layer2_core::{AgentId, Caller, Capability, ServiceId};
use std::sync::Arc;

pub(crate) struct AgentAccessService {
    store: Arc<MongoSandboxStore>,
    authority: Arc<dyn PersonalCommandAdmissionAuthority>,
    policy: Arc<dyn SandboxPolicySource>,
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
        })
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
        let baseline = self
            .policy
            .current_sandbox_policy()
            .await
            .map_err(|_| AgentAccessError::Unavailable)?
            .baseline_services;
        Ok(self
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
            .await?
            .view())
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
async fn retained_owner_authority(
    context: &CallerContext,
    credential: &str,
    authority: &dyn PersonalCommandAdmissionAuthority,
) -> Result<AgentAuthorizationRevision, AgentAccessError> {
    require_owner_capability(&context.caller)?;
    // Browser cookies project to direct API attribution. Retained custody and
    // the exact current personal credential establish consent authority.
    if !context.validate_principal()
        || context.credential_source != CredentialSource::Retained
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
        .personal_command_authority(&context.caller, credential)
        .await
        .map_err(|error| match error {
            CommandAuthorizationError::Revoked => AgentAccessError::Forbidden,
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
