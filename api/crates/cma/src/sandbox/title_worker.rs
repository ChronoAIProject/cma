//! Bind durable title claims to the current Sandbox runtime and scoped credential.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::profiles::ModelAccess;
use layer2_core::sandbox::store::{AgentLifecycleStore, Page, PageRequest};
use layer2_core::sandbox::{
    Agent, AgentStatus, AgentTitleGenerationContext, AgentTitleGenerationError,
    AgentTitleGenerationRequest, AgentTitleGenerationResponse, AgentTitleGenerator,
    AgentTitleOutcome, AgentTitleService, AgentTitleServiceError, Sandbox, SandboxStatus,
    AGENT_TITLE_CALL_TIMEOUT_SECONDS,
};
use layer2_core::{seconds_from_rfc3339, AgentId};
use layer3_adapter::hosted::MODEL_PROXY_BASE_URL;
use layer3_adapter::provider::{ProviderEndpoint, SandboxClient};
use layer3_adapter::SandboxAdapter;

use super::credential::{CredentialMaterial, SandboxCredentialController};
use super::runtime_attachment::adopt_bound_runtime;
use super::titles::HttpAgentTitleGenerator;
use crate::auth::persistence::SecretValue;

const PREPARE_TIMEOUT: Duration = Duration::from_secs(5);

#[async_trait::async_trait]
pub(crate) trait TitleRuntimeEndpoint: Send + Sync {
    async fn endpoint(&self, sandbox: &Sandbox) -> Option<TitleProxy>;
}

pub(crate) struct TitleProxy {
    pub endpoint: ProviderEndpoint,
    pub authorization: Option<SecretValue>,
}

pub(crate) struct ProviderTitleEndpoint {
    pub adapter: Arc<dyn SandboxAdapter>,
    pub client: SandboxClient,
    pub deployment_id: String,
}

#[async_trait::async_trait]
impl TitleRuntimeEndpoint for ProviderTitleEndpoint {
    async fn endpoint(&self, sandbox: &Sandbox) -> Option<TitleProxy> {
        let handle = adopt_bound_runtime(self.adapter.as_ref(), sandbox, &self.deployment_id)
            .await
            .ok()?;
        let port = reqwest::Url::parse(MODEL_PROXY_BASE_URL).ok()?.port()?;
        let mut endpoint = self.client.endpoint(handle.as_str(), port).await.ok()?;
        let configured = reqwest::Url::parse(self.client.base_url()).ok()?;
        if !matches!(configured.scheme(), "http" | "https") {
            return None;
        }
        if !endpoint.endpoint.contains("://") {
            endpoint.endpoint = format!("{}://{}", configured.scheme(), endpoint.endpoint);
        }
        let mut normalized = reqwest::Url::parse(&endpoint.endpoint).ok()?;
        if !matches!(normalized.scheme(), "http" | "https") {
            return None;
        }
        normalized.set_scheme(configured.scheme()).ok()?;
        endpoint.endpoint = normalized.into();
        Some(TitleProxy {
            endpoint,
            authorization: Some(SecretValue::new(self.client.auth_header().1.into())),
        })
    }
}

pub(crate) struct AgentTitleWorker {
    service: Arc<AgentTitleService>,
    store: Arc<dyn AgentLifecycleStore>,
    credentials: Arc<dyn SandboxCredentialController>,
    runtime: Arc<dyn TitleRuntimeEndpoint>,
    deployment_model: String,
}

impl AgentTitleWorker {
    pub(super) fn new(
        service: Arc<AgentTitleService>,
        store: Arc<dyn AgentLifecycleStore>,
        credentials: Arc<dyn SandboxCredentialController>,
        runtime: Arc<dyn TitleRuntimeEndpoint>,
        deployment_model: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            service,
            store,
            credentials,
            runtime,
            deployment_model,
        })
    }

    pub(super) async fn candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleServiceError> {
        self.service.candidates(page).await
    }

    pub(super) async fn run_one(
        self: &Arc<Self>,
        id: &AgentId,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
        let Some(authority) = tokio::time::timeout(PREPARE_TIMEOUT, self.authority(id))
            .await
            .ok()
            .flatten()
        else {
            return Ok(AgentTitleOutcome::Skipped);
        };
        if authority.sandbox.profile.content.model_access == ModelAccess::OwnCredentials {
            // Own-account runtimes intentionally have no deployment model proxy.
            return self.service.generate_agent_title(id, None, None).await;
        }
        let Some(material) = tokio::time::timeout(PREPARE_TIMEOUT, self.material(&authority))
            .await
            .ok()
            .flatten()
        else {
            return Ok(AgentTitleOutcome::Skipped);
        };
        let Some(context) =
            AgentTitleGenerationContext::new(&self.deployment_model, material.value.expose())
        else {
            return Ok(AgentTitleOutcome::Skipped);
        };
        let generator = FencedTitleGenerator {
            worker: Arc::clone(self),
            authority,
        };
        self.service
            .generate_agent_title(id, Some(&context), Some(&generator))
            .await
    }

    async fn authority(&self, id: &AgentId) -> Option<TitleAuthority> {
        let agent = self.store.get_agent(id).await.ok()??;
        let sandbox = self.store.get_sandbox(&agent.sandbox_id).await.ok()??;
        if agent.id != *id
            || agent.sandbox_id != sandbox.id
            || agent.owner != sandbox.owner
            || agent.title.is_some()
            || agent.status == AgentStatus::Deleted
            || sandbox.status != SandboxStatus::Active
            || sandbox.lifecycle.operation.is_some()
            || sandbox.runtime.as_ref().is_none_or(|binding| {
                binding.provision_generation.is_none()
                    || binding.provision_generation != agent.conversation.runtime_generation
            })
            || !sandbox.credential.validate()
            || !sandbox.credential.delivered
        {
            return None;
        }
        Some(TitleAuthority { agent, sandbox })
    }

    async fn material(&self, authority: &TitleAuthority) -> Option<CredentialMaterial> {
        let sandbox = &authority.sandbox;
        let current = sandbox.credential.current.as_ref()?;
        let expires = seconds_from_rfc3339(&current.expires_at)?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs();
        if expires <= now.checked_add(AGENT_TITLE_CALL_TIMEOUT_SECONDS)? {
            return None;
        }
        let material = self
            .credentials
            .material(&sandbox.id, current.generation)
            .await
            .ok()??;
        (material.sandbox_id == sandbox.id
            && material.owner == sandbox.owner
            && material.authority == *current
            && !material.value.expose().trim().is_empty())
        .then_some(material)
    }

    async fn is_current(&self, expected: &TitleAuthority, bearer: &str) -> bool {
        let Some(current) = self.authority(&expected.agent.id).await else {
            return false;
        };
        if !expected.matches(&current) {
            return false;
        }
        if self
            .material(&current)
            .await
            .is_none_or(|material| material.value.expose() != bearer)
        {
            return false;
        }
        self.authority(&expected.agent.id)
            .await
            .is_some_and(|current| expected.matches(&current))
    }
}

struct TitleAuthority {
    agent: Agent,
    sandbox: Sandbox,
}

impl TitleAuthority {
    fn matches(&self, other: &Self) -> bool {
        self.agent.id == other.agent.id
            && self.agent.owner == other.agent.owner
            && self.agent.sandbox_id == other.agent.sandbox_id
            && self.agent.workspace_id == other.agent.workspace_id
            && self.agent.worktree_id == other.agent.worktree_id
            && self.agent.conversation.conversation_id == other.agent.conversation.conversation_id
            && self.agent.conversation.runtime_generation
                == other.agent.conversation.runtime_generation
            && self.sandbox.runtime == other.sandbox.runtime
            && self.sandbox.credential.current == other.sandbox.credential.current
            && self.sandbox.credential.reaches == other.sandbox.credential.reaches
            && self.sandbox.credential.service_grants == other.sandbox.credential.service_grants
    }
}

struct FencedTitleGenerator {
    worker: Arc<AgentTitleWorker>,
    authority: TitleAuthority,
}

#[async_trait::async_trait]
impl AgentTitleGenerator for FencedTitleGenerator {
    async fn generate(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        let refused = AgentTitleGenerationError::unavailable;
        if !self
            .worker
            .is_current(&self.authority, request.bearer())
            .await
        {
            return Err(refused());
        }
        let endpoint = self
            .worker
            .runtime
            .endpoint(&self.authority.sandbox)
            .await
            .ok_or_else(refused)?;
        if !self
            .worker
            .is_current(&self.authority, request.bearer())
            .await
        {
            return Err(refused());
        }
        let mut generator =
            HttpAgentTitleGenerator::new(&endpoint.endpoint, &self.worker.deployment_model)
                .ok_or_else(refused)?;
        if let Some(authorization) = endpoint.authorization {
            generator = generator
                .with_proxy_authorization(authorization.expose())
                .ok_or_else(refused)?;
        }
        let response = generator.generate(request.clone()).await?;
        if !self
            .worker
            .is_current(&self.authority, request.bearer())
            .await
        {
            return Err(refused());
        }
        Ok(response)
    }
}

#[cfg(test)]
#[path = "title_worker_tests.rs"]
mod tests;
