//! Bind durable title claims to the current Sandbox runtime and scoped credential.

use super::title_budget::TitleBudget;
use layer2_core::sandbox::PreparedAgentTitle;
mod prepared;

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{AgentLifecycleStore, Page, PageRequest};
use layer2_core::sandbox::{
    Agent, AgentStatus, AgentTitleGenerationContext, AgentTitleGenerationError,
    AgentTitleGenerationRequest, AgentTitleGenerationResponse, AgentTitleGenerator,
    AgentTitleOutcome, AgentTitleService, AgentTitleServiceError, Sandbox, SandboxStatus,
    AGENT_TITLE_CALL_TIMEOUT_SECONDS,
};
use layer2_core::{seconds_from_rfc3339, AgentId};
use layer3_adapter::provider::ProviderEndpoint;

use super::credential::{CredentialMaterial, SandboxCredentialController};
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

/// Resolve the scoped model service independently of the Sandbox hosting provider.
/// Native homes have no legacy provider port endpoint; title generation only
/// needs the same scoped upstream and exact credential used by model discovery.
pub(crate) struct PlatformTitleEndpoint {
    pub platform_base_url: String,
}

#[async_trait::async_trait]
impl TitleRuntimeEndpoint for PlatformTitleEndpoint {
    async fn endpoint(&self, sandbox: &Sandbox) -> Option<TitleProxy> {
        let reaches = sandbox.credential.effective_reaches();
        let service = ["chrono-llm-public", "chrono-llm"]
            .into_iter()
            .find(|name| reaches.iter().any(|scope| scope.as_str() == *name))?;
        let mut endpoint = reqwest::Url::parse(&self.platform_base_url).ok()?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return None;
        }
        endpoint
            .path_segments_mut()
            .ok()?
            .extend(["api", "v1", "proxy", "s", service]);
        Some(TitleProxy {
            endpoint: ProviderEndpoint {
                endpoint: endpoint.into(),
            },
            authorization: None,
        })
    }
}

pub(crate) struct AgentTitleWorker {
    service: Arc<AgentTitleService>,
    store: Arc<dyn AgentLifecycleStore>,
    credentials: Arc<dyn SandboxCredentialController>,
    runtime: Arc<dyn TitleRuntimeEndpoint>,
    deployment_model: String,
    budget: Arc<dyn TitleBudget>,
    request_budget: Option<Arc<dyn layer2_core::budget::RequestBudget>>,
}

impl AgentTitleWorker {
    pub(super) fn new(
        service: Arc<AgentTitleService>,
        store: Arc<dyn AgentLifecycleStore>,
        credentials: Arc<dyn SandboxCredentialController>,
        runtime: Arc<dyn TitleRuntimeEndpoint>,
        deployment_model: String,
        budget: Arc<dyn TitleBudget>,
    ) -> Arc<Self> {
        Arc::new(Self {
            service,
            store,
            credentials,
            runtime,
            deployment_model,
            budget,
            request_budget: None,
        })
    }

    pub(super) fn with_request_budget(
        self: Arc<Self>,
        request_budget: Arc<dyn layer2_core::budget::RequestBudget>,
    ) -> Arc<Self> {
        Arc::new(Self {
            service: self.service.clone(),
            store: self.store.clone(),
            credentials: self.credentials.clone(),
            runtime: self.runtime.clone(),
            deployment_model: self.deployment_model.clone(),
            budget: self.budget.clone(),
            request_budget: Some(request_budget),
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
        if !self.budget.admit().await? {
            return Ok(AgentTitleOutcome::Skipped);
        }
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
        if self
            .service
            .title(id)
            .await
            .ok()?
            .is_some_and(|title| title.source != layer2_core::sandbox::AgentTitleSource::Fallback)
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

#[derive(Clone)]
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
        self.prepare(request).await?.dispatch().await
    }
    async fn prepare(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<Box<dyn PreparedAgentTitle>, AgentTitleGenerationError> {
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
            .ok_or_else(AgentTitleGenerationError::permanent)?;
        if !self
            .worker
            .is_current(&self.authority, request.bearer())
            .await
        {
            return Err(refused());
        }
        let mut generator =
            HttpAgentTitleGenerator::new(&endpoint.endpoint, &self.worker.deployment_model)
                .ok_or_else(AgentTitleGenerationError::permanent)?;
        if let Some(authorization) = endpoint.authorization {
            generator = generator
                .with_proxy_authorization(authorization.expose())
                .ok_or_else(refused)?;
        }
        if let Some(budget) = &self.worker.request_budget {
            generator =
                generator.with_request_budget(super::model_request_budget::RequestBudgetContext {
                    budget: budget.clone(),
                    scope: layer2_core::budget::RequestScope {
                        agent_id: self.authority.agent.id.clone(),
                        turn_sequence: request.turn_sequence(),
                        request_id: format!("title:{}", uuid::Uuid::new_v4()),
                    },
                });
        }
        let inner = generator.prepare(request.clone()).await?;
        if !self
            .worker
            .is_current(&self.authority, request.bearer())
            .await
        {
            return Err(refused());
        }
        Ok(Box::new(prepared::FencedPrepared {
            inner,
            worker: self.worker.clone(),
            authority: self.authority.clone(),
            request,
        }))
    }
}

#[cfg(test)]
#[path = "title_worker_tests.rs"]
pub(super) mod tests;
#[cfg(test)]
pub(crate) use tests::fixtures::{scheduled_title_fixture, start_title_scheduler};
