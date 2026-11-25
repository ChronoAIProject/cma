use super::profiles::BindingProfiles;
use super::*;
use crate::profiles::{AgentProvider, ProfileCapacityBounds};
use crate::sandbox::store::{AgentStore, SandboxStore, SandboxStoreError, WorkspaceStore};
use crate::sandbox::{AgentService, SandboxPolicySource, SandboxService, WorkspaceService};
use crate::{now_rfc3339, Caller, Capability};
use std::sync::Arc;

pub trait BindingServiceStore:
    BindingStore
    + SandboxStore
    + WorkspaceStore
    + AgentStore
    + crate::resource_access::ResourceAccessStore
{
}
impl<
        T: BindingStore
            + SandboxStore
            + WorkspaceStore
            + AgentStore
            + crate::resource_access::ResourceAccessStore,
    > BindingServiceStore for T
{
}

pub struct BindingResources {
    pub sandboxes: Arc<SandboxService>,
    pub workspaces: Arc<WorkspaceService>,
    pub agents: Arc<AgentService>,
}

pub struct BindingPolicy {
    pub provider: AgentProvider,
    pub supported_model_api: String,
    pub bounds: ProfileCapacityBounds,
    pub sandbox: Arc<dyn SandboxPolicySource>,
}

pub struct BindingService {
    pub(super) store: Arc<dyn BindingServiceStore>,
    pub(super) profiles: BindingProfiles,
    pub(super) resources: BindingResources,
    pub(super) credentials: Arc<dyn BindingCredentialCustody>,
    pub(super) policy: BindingPolicy,
}

#[path = "error.rs"]
mod error;
pub use error::BindingError;

impl BindingService {
    pub fn new<S>(
        store: Arc<S>,
        resources: BindingResources,
        credentials: Arc<dyn BindingCredentialCustody>,
        policy: BindingPolicy,
    ) -> Arc<Self>
    where
        S: BindingServiceStore + 'static,
    {
        Arc::new(Self {
            store,
            profiles: BindingProfiles,
            resources,
            credentials,
            policy,
        })
    }

    pub async fn read(&self, caller: &Caller, id: &str) -> Result<ProviderBinding, BindingError> {
        let binding = self.read_for_retirement(caller, id).await?;
        super::readiness::validate(self.store.as_ref(), &binding).await?;
        Ok(binding)
    }

    /// Ownership checks still apply, but missing backing resources must not
    /// prevent an authorized manager from retiring a stale binding.
    pub async fn read_for_retirement(
        &self,
        caller: &Caller,
        id: &str,
    ) -> Result<ProviderBinding, BindingError> {
        require(caller)?;
        let binding = self
            .store
            .binding(id)
            .await?
            .ok_or(BindingError::NotFound)?;
        if binding.manager != caller.subject {
            return Err(BindingError::NotFound);
        }
        Ok(binding)
    }

    pub async fn put(
        &self,
        caller: &Caller,
        id: &str,
        profile: serde_json::Value,
        digest: &str,
        credential: &ProvidedCredential,
    ) -> Result<ProviderBinding, BindingError> {
        self.put_with_tool_endpoint(caller, id, profile, digest, credential, None)
            .await
    }

    /// The interface must first prove support for the advertised endpoint. Its
    /// destination becomes immutable authority for the entire binding lifetime.
    pub async fn put_with_tool_endpoint(
        &self,
        caller: &Caller,
        id: &str,
        profile: serde_json::Value,
        digest: &str,
        credential: &ProvidedCredential,
        tool_endpoint: Option<String>,
    ) -> Result<ProviderBinding, BindingError> {
        require(caller)?;
        if tool_endpoint
            .as_deref()
            .is_some_and(|value| !super::valid_tool_endpoint(value))
        {
            return Err(BindingError::InvalidRequest("invalid tool endpoint".into()));
        }
        self.credentials
            .validate(credential)
            .map_err(credential_error)?;
        if !super::model::valid_id(id, 1) {
            return Err(BindingError::InvalidRequest(
                "invalid binding identifier".into(),
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
        let policy = self
            .policy
            .sandbox
            .current_sandbox_policy()
            .await
            .map_err(unavailable)?;
        if tool_endpoint.is_some() {
            plan.honoured.push("tools.native_mcp_url".into());
        } else {
            plan.refused.push(BindingFieldRefusal {
                field: "tools.native_mcp_url".into(),
                reason: "external tool endpoints are not supported".into(),
            });
        }
        if let Some(environment) = &mut plan.environment {
            // Pin the effective floor once. Replays keep the original reservation
            // even when the deployment policy changes between attempts.
            let services = &mut environment.sandbox.nyxid_services;
            let routes = environment.sandbox.agent_provider.connection_services();
            services.extend(
                policy
                    .baseline_services
                    .into_iter()
                    .filter(|service| !routes.contains(service.as_str())),
            );
            services.sort();
            services.dedup();
        }
        for _ in 0..8 {
            let current = self.store.binding(id).await?;
            if current
                .as_ref()
                .is_some_and(|value| value.manager != caller.subject)
            {
                return Err(BindingError::NotFound);
            }
            if current
                .as_ref()
                .is_some_and(|value| value.existing_agent.is_some())
            {
                return Err(BindingError::Conflict);
            }
            if current
                .as_ref()
                .is_some_and(|value| value.tool_endpoint != tool_endpoint)
            {
                return Err(BindingError::Conflict);
            }
            if let Some(value) = &current {
                super::readiness::validate(self.store.as_ref(), value).await?;
            }
            let binding = match current {
                Some(value) if value.state == BindingState::Retired => {
                    return Err(BindingError::Conflict)
                }
                Some(value)
                    if value.profile_digest == digest
                        && value.key_sha256 == credential.sha256() =>
                {
                    if matches!(value.state, BindingState::Failed { .. }) {
                        return self
                            .credentials
                            .retry_recovered_binding(&value, credential)
                            .await
                            .map_err(credential_error);
                    }
                    value
                }
                Some(value) if value.profile_digest == digest => {
                    if matches!(value.state, BindingState::Failed { .. }) {
                        return Err(BindingError::Conflict);
                    }
                    return self
                        .credentials
                        .rotate_binding(&value, credential)
                        .await
                        .map_err(credential_error);
                }
                Some(value) => {
                    if value.key_sha256 == credential.sha256() {
                        if let Some((id, installed, requested)) = value.skill_version_update(&plan)
                        {
                            return Err(BindingError::SkillVersionUpdateUnsupported {
                                id,
                                installed,
                                requested,
                            });
                        }
                    }
                    if !value.retains_runtime_configuration(&plan)
                        || value.key_sha256 != credential.sha256()
                    {
                        return Err(BindingError::ReconfigurationUnsupported);
                    }
                    let next = value
                        .reconfigure(plan.clone(), credential.sha256(), now_rfc3339())
                        .map_err(|_| BindingError::Conflict)?;
                    match self
                        .store
                        .save_binding(Some(value.revision), next.clone())
                        .await
                    {
                        Ok(()) => next,
                        Err(SandboxStoreError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
                None => {
                    if plan.environment.is_none() {
                        return Err(BindingError::InvalidRequest(
                            "environment is required for a profile-managed binding".into(),
                        ));
                    }
                    let next = ProviderBinding::new_with_tool_endpoint(
                        id.into(),
                        caller.subject.clone(),
                        plan.clone(),
                        credential.sha256(),
                        tool_endpoint.clone(),
                        now_rfc3339(),
                    )
                    .map_err(BindingError::InvalidRequest)?;
                    match self.store.save_binding(None, next.clone()).await {
                        Ok(()) => next,
                        Err(SandboxStoreError::Conflict) => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
            };
            if binding.work != BindingWork::AwaitingCredential {
                return Ok(binding);
            }
            self.credentials
                .retain_binding(&binding, credential)
                .await
                .map_err(credential_error)?;
            let mut next = binding.clone();
            next.work = if binding.previous.is_some() {
                BindingWork::RetirePreviousConversations
            } else {
                BindingWork::ProvisionSandbox
            };
            next.advance_revision(now_rfc3339())
                .map_err(BindingError::Corrupt)?;
            match self
                .store
                .save_binding(Some(binding.revision), next.clone())
                .await
            {
                Ok(()) => return Ok(next),
                Err(SandboxStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(BindingError::Conflict)
    }

    pub async fn retire(&self, caller: &Caller, id: &str) -> Result<(), BindingError> {
        for _ in 0..8 {
            let binding = match self.read_for_retirement(caller, id).await {
                Ok(value) => value,
                Err(BindingError::NotFound) => return Ok(()),
                Err(error) => return Err(error),
            };
            if binding.state == BindingState::Retired {
                return Ok(());
            }
            let mut next = binding.clone();
            next.state = BindingState::Retired;
            next.work = if next.previous.is_some() {
                BindingWork::RetirePreviousConversations
            } else {
                BindingWork::RetireCurrentConversations
            };
            next.advance_revision(now_rfc3339())
                .map_err(BindingError::Corrupt)?;
            match self.store.save_binding(Some(binding.revision), next).await {
                Ok(()) => return Ok(()),
                Err(SandboxStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(BindingError::Conflict)
    }

    pub async fn ensure_conversation(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
    ) -> Result<(BindingConversation, bool), BindingError> {
        self.ensure_conversation_at_generation(caller, id, conversation_id, None)
            .await
    }

    /// A provider request pins its parent before source-user routing. A
    /// generation change cannot redirect a late ensure into a new hierarchy.
    pub async fn ensure_conversation_at_generation(
        &self,
        caller: &Caller,
        id: &str,
        conversation_id: &str,
        expected_generation: Option<u64>,
    ) -> Result<(BindingConversation, bool), BindingError> {
        if !super::model::valid_id(conversation_id, 8) {
            return Err(BindingError::InvalidRequest(
                "invalid conversation identifier".into(),
            ));
        }
        for _ in 0..8 {
            let binding = self.read(caller, id).await?;
            binding
                .require_generation(expected_generation)
                .map_err(|_| BindingError::Conflict)?;
            let conversation_id = if binding.existing_agent.is_some() {
                EXISTING_AGENT_CONVERSATION
            } else {
                conversation_id
            };
            if binding.state != BindingState::Ready {
                return Err(BindingError::Conflict);
            }
            let resets = self.store.conversation_reset_state(id).await?;
            let head = resets.head(binding.generation, conversation_id);
            let conversation_id =
                head.map_or(conversation_id, |h| h.successor.conversation_id.as_str());
            if let Some(value) = self
                .store
                .binding_conversation(id, binding.generation, conversation_id)
                .await?
            {
                if matches!(
                    value.state,
                    BindingConversationState::Retiring | BindingConversationState::Retired
                ) {
                    return Err(BindingError::Conflict);
                }
                return Ok((value, false));
            }
            let value = match head {
                Some(head) => head.successor.clone(),
                None => BindingConversation::new(&binding, conversation_id.into(), now_rfc3339())
                    .map_err(BindingError::InvalidRequest)?,
            };
            match self
                .store
                .save_binding_conversation(binding.revision, None, value.clone())
                .await
            {
                Ok(()) => return Ok((value, true)),
                Err(SandboxStoreError::Conflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(BindingError::Conflict)
    }
}

pub(super) fn require(caller: &Caller) -> Result<(), BindingError> {
    caller
        .require(Capability::ManageGatewayRecords)
        .map_err(|_| BindingError::Forbidden)?;
    caller
        .require(Capability::ManageProviderBindings)
        .map_err(|_| BindingError::Forbidden)
}

pub(super) fn unavailable(error: impl std::fmt::Display) -> BindingError {
    BindingError::Unavailable(error.to_string())
}

pub(super) fn credential_error(error: crate::CoreError) -> BindingError {
    error.into()
}
