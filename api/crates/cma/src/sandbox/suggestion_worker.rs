use super::{
    credential::SandboxCredentialController, suggestions::HttpSuggestionGenerator,
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
}

impl SuggestionWorker {
    pub(super) fn new(
        store: Arc<crate::store::MongoSandboxStore>,
        credentials: Arc<dyn SandboxCredentialController>,
        runtime: Arc<dyn TitleRuntimeEndpoint>,
        model: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            service: SuggestionGenerationService::new(store.clone()),
            suggestions: store.clone(),
            writer_policy: store.writer_policy.clone(),
            store,
            credentials,
            runtime,
            model,
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
        {
            return None;
        }
        Some((agent, sandbox))
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
        let (_, sandbox) = self.authority(work).await.ok_or_else(refused)?;
        let authority = sandbox.credential.current.as_ref().ok_or_else(refused)?;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| refused())?
            .as_secs();
        if seconds_from_rfc3339(&authority.expires_at)
            .is_none_or(|expiry| expiry <= now + GENERATION_TIMEOUT_SECONDS)
        {
            return Err(refused());
        }
        let material = self
            .credentials
            .material(&sandbox.id, authority.generation)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.authority != *authority
        {
            return Err(refused());
        }
        let endpoint = self.runtime.endpoint(&sandbox).await.ok_or_else(refused)?;
        self.authority(work).await.ok_or_else(refused)?;
        let current = self
            .suggestions
            .read_suggestions(&work.owner, &work.agent_id, work.scope)
            .await
            .map_err(|_| refused())?;
        if current.status != SuggestionStatus::Running {
            return Err(refused());
        }
        let generator = HttpSuggestionGenerator::new(
            &endpoint.endpoint.endpoint,
            &self.model,
            material.value,
            endpoint.authorization,
        )
        .ok_or_else(refused)?;
        generator.generate(work).await
    }
}
