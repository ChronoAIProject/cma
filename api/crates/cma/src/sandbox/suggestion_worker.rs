use super::{
    credential::{CredentialMaterial, SandboxCredentialController},
    suggestions::HttpSuggestionGenerator,
    title_worker::TitleRuntimeEndpoint,
};
use layer2_core::profiles::ModelAccess;
use layer2_core::{
    sandbox::{store::AgentLifecycleStore, suggestions::*, Agent, Sandbox, SandboxStatus},
    seconds_from_rfc3339,
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

pub(crate) struct SuggestionWorker {
    service: Arc<SuggestionGenerationService>,
    store: Arc<dyn AgentLifecycleStore>,
    suggestions: Arc<dyn SuggestionStore>,
    credentials: Arc<dyn SandboxCredentialController>,
    runtime: Arc<dyn TitleRuntimeEndpoint>,
    model: String,
    writer_policy: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    request_budget: Option<Arc<dyn layer2_core::budget::RequestBudget>>,
}

impl SuggestionWorker {
    pub(super) fn new(
        store: Arc<crate::store::MongoSandboxStore>,
        credentials: Arc<dyn SandboxCredentialController>,
        runtime: Arc<dyn TitleRuntimeEndpoint>,
        model: String,
    ) -> Arc<Self> {
        let mut worker = Self::with_dependencies(
            store.clone(),
            store.clone(),
            credentials,
            runtime,
            model,
            store.writer_policy.clone(),
        );
        Arc::get_mut(&mut worker)
            .expect("new suggestion worker is exclusively owned")
            .request_budget = Some(store);
        worker
    }

    pub(super) fn with_dependencies(
        store: Arc<dyn AgentLifecycleStore>,
        suggestions: Arc<dyn SuggestionStore>,
        credentials: Arc<dyn SandboxCredentialController>,
        runtime: Arc<dyn TitleRuntimeEndpoint>,
        model: String,
        writer_policy: Arc<layer2_core::sandbox::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self> {
        Arc::new(Self {
            service: SuggestionGenerationService::new(suggestions.clone()),
            suggestions,
            writer_policy,
            store,
            credentials,
            runtime,
            model,
            request_budget: None,
        })
    }

    async fn authority(&self, work: &SuggestionWork) -> Option<(Agent, Sandbox)> {
        if !self.writer_policy.allows_writer(
            &work.owner,
            layer2_core::sandbox::task_experience::TaskExperienceWriter::Suggestions,
        ) {
            return None;
        }
        let agent = self.store.get_agent(&work.agent_id).await.ok()??;
        let sandbox = self.store.get_sandbox(&agent.sandbox_id).await.ok()??;
        if agent.owner != work.owner
            || agent.owner != sandbox.owner
            || authority_digest(&agent, &sandbox) != work.authority
            || sandbox.status != SandboxStatus::Active
            || sandbox.lifecycle.operation.is_some()
            || sandbox.profile.content.model_access == ModelAccess::OwnCredentials
            || sandbox.runtime.as_ref().is_none_or(|binding| {
                binding.provision_generation.is_none()
                    || binding.provision_generation != agent.conversation.runtime_generation
            })
            || !sandbox.credential.validate()
            || !sandbox.credential.delivered
            || sandbox.needs_service_access_credential()
        {
            return None;
        }
        Some((agent, sandbox))
    }

    async fn material(&self, work: &SuggestionWork) -> Option<(Sandbox, CredentialMaterial)> {
        let (_, sandbox) = self.authority(work).await?;
        let authority = sandbox.credential.current.as_ref()?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs();
        if seconds_from_rfc3339(&authority.expires_at)
            .is_none_or(|expiry| expiry <= now + GENERATION_TIMEOUT_SECONDS)
        {
            return None;
        }
        let material = self
            .credentials
            .material(&sandbox.id, authority.generation)
            .await
            .ok()??;
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.authority != *authority
        {
            return None;
        }
        Some((sandbox, material))
    }

    async fn running(&self, work: &SuggestionWork) -> bool {
        self.suggestions
            .read_suggestions(&work.owner, &work.agent_id, work.scope)
            .await
            .is_ok_and(|current| current.status == SuggestionStatus::Running)
    }

    pub(super) fn spawn(
        self: Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { break; } }
                    _ = tick.tick(), if tasks.len() < 2 => {
                        let worker = self.clone();
                        tasks.spawn(async move {
                            if !matches!(tokio::time::timeout(Duration::from_secs(80), worker.service.run_next(worker.as_ref())).await, Ok(Ok(_))) {
                                tracing::warn!("contextual suggestion work could not be confirmed");
                            }
                        });
                    }
                    _ = tasks.join_next(), if !tasks.is_empty() => {}
                }
            }
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        })
    }
}

#[async_trait::async_trait]
impl SuggestionGenerator for SuggestionWorker {
    async fn generate(
        &self,
        work: &SuggestionWork,
    ) -> Result<SuggestionGeneration, SuggestionFailure> {
        let refused = || SuggestionFailure::Unavailable;
        let (sandbox, material) = self.material(work).await.ok_or_else(refused)?;
        let endpoint = self.runtime.endpoint(&sandbox).await.ok_or_else(refused)?;
        self.authority(work).await.ok_or_else(refused)?;
        if !self.running(work).await {
            return Err(refused());
        }
        let mut generator = HttpSuggestionGenerator::new(
            &endpoint.endpoint.endpoint,
            &self.model,
            material.value.clone(),
            endpoint.authorization,
        )
        .ok_or_else(refused)?;
        if let Some(budget) = &self.request_budget {
            generator =
                generator.with_request_budget(super::model_request_budget::RequestBudgetContext {
                    budget: budget.clone(),
                    scope: layer2_core::budget::RequestScope {
                        agent_id: work.agent_id.clone(),
                        turn_sequence: work.scope.turn_sequence,
                        request_id: format!("suggestion:{}", work.id),
                    },
                });
        }
        let prepared = generator.prepare(work).await?;
        let (_, current) = self.material(work).await.ok_or_else(refused)?;
        if current.value != material.value || !self.running(work).await {
            return Err(refused());
        }
        self.authority(work).await.ok_or_else(refused)?;
        prepared.dispatch().await
    }
}

#[cfg(test)]
#[path = "suggestion_worker_tests.rs"]
mod tests;
