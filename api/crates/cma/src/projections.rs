//! Shared production composition for Agent-facing public projections.

use std::sync::Arc;

use axum::Router;
use layer1_public::rest::{
    AgentHistoryExportError, AgentHistoryExporter, AgentLifecycleOperationReader,
    AgentLifecycleProvisioner,
};
use layer2_core::profiles::{AgentProfileStore, ProfileUseRecorder, SandboxProfileStore};
use layer2_core::sandbox::store::{AgentReadStore, AgentServiceStore, SandboxSessionStore};
use layer2_core::sandbox::{
    AgentContextService, AgentOperationService, AgentOperationStore, AgentReadService,
    AgentRuntimeRegistry, AgentService, AgentUpdateRecorder, Sandbox, SandboxResourceReadError,
    SandboxResourceReader, SandboxResourceReading,
};
use layer2_core::RuntimeCapabilities;
use layer3_adapter::{AdapterError, RuntimeHandle, SandboxAdapter};
#[path = "projections/agent_v2.rs"]
mod agent_v2;
#[cfg(test)]
#[path = "projections/response_readers_tests.rs"]
mod response_readers_tests;
mod rest;
pub(crate) use rest::{rest_links, rest_router, REST_LINKS};

pub(crate) struct AgentProjectionServices {
    external_review_origin: String,
    #[cfg(feature = "qualification-hooks")]
    qualification: Option<Arc<cma_qualification::Registry>>,
    bounded_replay: bool,
    sandbox_profile_readability: bool,
    sandbox_profiles: Arc<dyn SandboxProfileStore>,
    suggestions: Option<Arc<dyn layer2_core::sandbox::suggestions::SuggestionStore>>,
    agent_page_origin: layer1_public::agent_page::AgentPageOrigin,
    message_feedback: Option<Arc<dyn layer2_core::sandbox::message_feedback::MessageFeedbackStore>>,
    skill_support: Option<Arc<dyn layer1_public::rest::AgentSkillRuntimeSupport>>,
    commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    agents: Arc<AgentService>,
    owner_agent_restart: Option<Arc<layer2_core::sandbox::AgentRestartService>>,
    operations: Arc<AgentOperationService>,
    reads: Arc<AgentReadService>,
    lifecycle: Option<Arc<dyn AgentLifecycleProvisioner>>,
    lifecycle_operations: Option<Arc<dyn AgentLifecycleOperationReader>>,
    history_exporter: Option<Arc<dyn AgentHistoryExporter>>,
}

impl AgentProjectionServices {
    pub(crate) fn with_observation(
        mut self,
        observation: Arc<layer2_core::sandbox::agent_observation::AgentObservationService>,
    ) -> Self {
        self.reads = self.reads.with_observation(observation);
        self
    }
    pub(crate) fn with_response_logs(
        mut self,
        reader: Option<Arc<dyn layer2_core::sandbox::store::AgentResponseLogStore>>,
    ) -> Self {
        // All response routes, including legacy streams, share this service.
        // Retained adopted histories need the reader even after admission closes.
        self.reads = self.reads.with_response_logs(reader);
        self
    }
    pub(crate) fn with_suggestions(
        mut self,
        store: Arc<dyn layer2_core::sandbox::suggestions::SuggestionStore>,
    ) -> Self {
        self.suggestions = Some(store);
        self
    }
    pub(crate) fn with_message_feedback(
        mut self,
        store: Arc<dyn layer2_core::sandbox::message_feedback::MessageFeedbackStore>,
    ) -> Self {
        self.message_feedback = Some(store);
        self
    }
    pub(crate) fn with_skill_support(
        mut self,
        support: Arc<dyn layer1_public::rest::AgentSkillRuntimeSupport>,
    ) -> Self {
        self.skill_support = Some(support);
        self
    }
    pub(crate) fn with_commands(
        mut self,
        commands: Arc<layer2_core::sandbox::agent_io::AgentCommandService>,
    ) -> Self {
        self.commands = Some(commands);
        self
    }

    pub(crate) fn with_owner_agent_restart(
        mut self,
        service: Arc<layer2_core::sandbox::AgentRestartService>,
    ) -> Self {
        self.owner_agent_restart = Some(service);
        self
    }

    pub(crate) fn with_lifecycle(mut self, lifecycle: Arc<dyn AgentLifecycleProvisioner>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub(crate) fn with_lifecycle_operations(
        mut self,
        reader: Arc<dyn AgentLifecycleOperationReader>,
    ) -> Self {
        self.lifecycle_operations = Some(reader);
        self
    }

    pub(crate) fn with_history_exporter(mut self, exporter: Arc<dyn AgentHistoryExporter>) -> Self {
        self.history_exporter = Some(exporter);
        self
    }

    pub(crate) fn with_resource_access(
        mut self,
        access: Arc<layer2_core::resource_access::ResourceAccessService>,
    ) -> Self {
        self.reads = self.reads.with_resource_access(access);
        self
    }

    pub(crate) fn with_sessions(mut self, sessions: Arc<dyn SandboxSessionStore>) -> Self {
        self.reads = self.reads.with_sessions(sessions);
        self
    }

    pub(crate) fn with_features(
        mut self,
        flags: crate::config::FeatureFlags,
        model_funding: Option<Arc<layer2_core::sandbox::model_credit::AgentTurnFunding>>,
    ) -> Self {
        self.operations = self
            .operations
            .with_skill_upgrades_enabled(flags.agent_skill_upgrade_enabled)
            .with_model_credit_required(flags.model_credit_required)
            .with_model_funding(model_funding);
        self.agents = self
            .agents
            .with_model_credit_required(flags.model_credit_required);
        self.bounded_replay = flags.agui_bounded_replay;
        self.sandbox_profile_readability = flags.sandbox_profile_readability;
        self
    }

    pub(crate) fn with_connections(
        mut self,
        connections: Arc<dyn layer2_core::sandbox::SandboxCredentialPrerequisite>,
    ) -> Self {
        self.agents = self.agents.with_connections(connections.clone());
        self.operations = self.operations.with_connections(connections);
        self
    }

    pub(crate) fn new<S, P, R, Q>(
        agent_page_origin: layer1_public::agent_page::AgentPageOrigin,
        store: Arc<S>,
        profiles: Arc<P>,
        runtimes: Arc<R>,
        resources: Arc<Q>,
        runtime_capabilities: RuntimeCapabilities,
    ) -> Self
    where
        S: AgentServiceStore + AgentReadStore + AgentOperationStore + 'static,
        P: AgentProfileStore + SandboxProfileStore + ProfileUseRecorder + 'static,
        R: AgentRuntimeRegistry + 'static,
        Q: SandboxResourceReader + 'static,
    {
        let agents = AgentService::new(
            store.clone(),
            profiles.clone(),
            runtimes.clone(),
            runtime_capabilities,
        );
        let reads = AgentReadService::new(store.clone(), resources);
        let operations = AgentOperationService::new(store, runtimes).with_instruction_policy(
            Arc::new(crate::agent_lifecycle::guidance::ManagedInstructionPolicy),
        );
        Self {
            #[cfg(feature = "qualification-hooks")]
            qualification: None,
            bounded_replay: false,
            external_review_origin: String::new(),
            sandbox_profile_readability: false,
            sandbox_profiles: profiles,
            suggestions: None,
            agent_page_origin,
            message_feedback: None,
            skill_support: None,
            commands: None,
            agents,
            owner_agent_restart: None,
            operations,
            reads,
            lifecycle: None,
            lifecycle_operations: None,
            history_exporter: None,
        }
    }

    pub(crate) fn agents(&self) -> Arc<AgentService> {
        Arc::clone(&self.agents)
    }

    pub(crate) fn operations(&self) -> Arc<AgentOperationService> {
        Arc::clone(&self.operations)
    }

    pub(crate) fn reads(&self) -> Arc<AgentReadService> {
        Arc::clone(&self.reads)
    }

    pub(crate) fn conversation_router(
        &self,
        context: Arc<AgentContextService>,
        updates: Arc<dyn AgentUpdateRecorder>,
        public_base: &str,
    ) -> Router {
        let state = layer1_public::AgentConversationState::new(
            self.agent_page_origin.clone(),
            self.reads.clone(),
            self.operations.clone(),
            context,
        )
        .with_updates(updates)
        .with_bounded_replay(self.bounded_replay);
        let state = match &self.commands {
            Some(commands) => state.with_commands(commands.clone()),
            None => state,
        };
        layer1_public::agui::router(state.clone())
            .merge(layer1_public::responses::router(state))
            .merge(layer1_public::mcp::router())
            .merge(layer1_public::a2a::router(public_base))
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct UnavailableSandboxResources;

#[cfg(test)]
#[async_trait::async_trait]
impl SandboxResourceReader for UnavailableSandboxResources {
    async fn sandbox_resources(
        &self,
        _sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        Err(SandboxResourceReadError::Unavailable(
            "the hosted Sandbox resource transport is not composed".into(),
        ))
    }
}

pub(crate) struct AdapterSandboxResources {
    adapter: Arc<dyn SandboxAdapter>,
}

pub(crate) struct AdapterAgentHistoryExporter {
    adapter: Arc<dyn SandboxAdapter>,
}

impl AdapterAgentHistoryExporter {
    pub(crate) fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self { adapter }
    }
}

#[async_trait::async_trait]
impl AgentHistoryExporter for AdapterAgentHistoryExporter {
    async fn export(
        &self,
        runtime_id: &str,
        agent_id: &str,
    ) -> Result<Vec<u8>, AgentHistoryExportError> {
        let bytes = self
            .adapter
            .export_agent_history(
                &RuntimeHandle::new(runtime_id),
                &layer3_adapter::AgentSlot::new(agent_id),
            )
            .await
            .map_err(|error| {
                let reason = match error {
                    AdapterError::RuntimeCapacity(_) => AgentHistoryExportError::Capacity,
                    AdapterError::Unsupported(_) => AgentHistoryExportError::Unsupported,
                    AdapterError::InputAuthorityUnavailable(_)
                    | AdapterError::PromptNotWritten(_)
                    | AdapterError::NativePoolDefinitionAbsent { .. }
                    | AdapterError::NativeClaimNotReady(_)
                    | AdapterError::RuntimeStartupHold(_)
                    | AdapterError::Unavailable(_)
                    | AdapterError::RuntimeLedgerHold(_)
                    | AdapterError::AtCapacity(_) => AgentHistoryExportError::Unavailable,
                    AdapterError::Rejected(_)
                    | AdapterError::AdmittedSandboxSpecDrift
                    | AdapterError::NativePoolConfigurationDrift
                    | AdapterError::DesktopFailed
                    | AdapterError::AccessRevoked
                    | AdapterError::SkillUpgradeRefused(_)
                    | AdapterError::OperationIdentityConflict(_)
                    | AdapterError::RuntimeContractMismatch(_) => AgentHistoryExportError::Rejected,
                };
                tracing::warn!(parent: None, reason = ?reason, "Agent raw history export failed");
                reason
            })?;
        Ok(bytes)
    }
}

impl AdapterSandboxResources {
    pub(crate) fn new(adapter: Arc<dyn SandboxAdapter>) -> Self {
        Self { adapter }
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for AdapterSandboxResources {
    async fn sandbox_resources(
        &self,
        sandbox: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        let runtime = sandbox.runtime.as_ref().ok_or_else(|| {
            SandboxResourceReadError::Unsupported("the sandbox has no active runtime".into())
        })?;
        if runtime.runtime_id.trim().is_empty() || runtime.runtime_id != runtime.runtime_id.trim() {
            return Err(SandboxResourceReadError::Corrupt(
                "the sandbox runtime identity is invalid".into(),
            ));
        }
        let reading = self
            .adapter
            .resources(&RuntimeHandle(runtime.runtime_id.clone()))
            .await
            .map_err(|error| resource_failure(&sandbox.id.to_string(), error))?;
        Ok(SandboxResourceReading {
            uptime_ms: reading.uptime_ms,
            cpu_usage_usec: reading.cpu_usage_usec,
            cpu_millis_limit: reading.cpu_millis_limit,
            memory_bytes_used: reading.memory_bytes_used,
            memory_bytes_limit: reading.memory_bytes_limit,
            memory_bytes_peak: reading.memory_bytes_peak,
            stall_permille: reading.stall_permille,
            ceiling_hits: reading.ceiling_hits,
            allocation_failures: reading.allocation_failures,
            kills: reading.kills,
            received_bytes: reading.received_bytes,
            sent_bytes: reading.sent_bytes,
        })
    }
}

fn resource_failure(sandbox_id: &str, error: AdapterError) -> SandboxResourceReadError {
    tracing::error!(sandbox_id, error = %error, "sandbox resource read failed");
    if let Some(capacity) = layer2_core::sandbox::RuntimeCapacityError::from_adapter(&error) {
        return SandboxResourceReadError::RuntimeCapacity(capacity);
    }
    match error {
        AdapterError::Unsupported(_) => SandboxResourceReadError::Unsupported(
            "the runtime does not support resource readings".into(),
        ),
        AdapterError::Rejected(_)
        | AdapterError::AdmittedSandboxSpecDrift
        | AdapterError::NativePoolConfigurationDrift
        | AdapterError::DesktopFailed
        | AdapterError::AccessRevoked
        | AdapterError::SkillUpgradeRefused(_)
        | AdapterError::OperationIdentityConflict(_)
        | AdapterError::RuntimeContractMismatch(_) => SandboxResourceReadError::Corrupt(
            "the runtime resource authority or reply is invalid".into(),
        ),
        AdapterError::InputAuthorityUnavailable(_)
        | AdapterError::PromptNotWritten(_)
        | AdapterError::NativePoolDefinitionAbsent { .. }
        | AdapterError::NativeClaimNotReady(_)
        | AdapterError::RuntimeStartupHold(_)
        | AdapterError::Unavailable(_)
        | AdapterError::RuntimeLedgerHold(_)
        | AdapterError::RuntimeCapacity(_)
        | AdapterError::AtCapacity(_) => SandboxResourceReadError::Unavailable(
            "the runtime resource transport is unavailable".into(),
        ),
    }
}

pub(crate) struct AdapterAgentSkillSupport(pub Arc<dyn SandboxAdapter>);
#[async_trait::async_trait]
impl layer1_public::rest::AgentSkillRuntimeSupport for AdapterAgentSkillSupport {
    async fn supported(&self, sandbox: &Sandbox) -> bool {
        self.observe(sandbox, false).await.unwrap_or(false)
    }
    async fn upgrade_supported(&self, sandbox: &Sandbox) -> bool {
        self.observe(sandbox, true).await.unwrap_or(false)
    }
    async fn checked_supported(
        &self,
        sandbox: &Sandbox,
        upgrade: bool,
    ) -> Result<bool, layer2_core::sandbox::RuntimeCapacityError> {
        self.observe(sandbox, upgrade).await
    }
}
impl AdapterAgentSkillSupport {
    async fn observe(
        &self,
        sandbox: &Sandbox,
        upgrade: bool,
    ) -> Result<bool, layer2_core::sandbox::RuntimeCapacityError> {
        let Some(binding) = &sandbox.runtime else {
            return Ok(false);
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.0
                .runtime_status(&RuntimeHandle::new(&binding.runtime_id)),
        )
        .await;
        Ok(match result {
            Ok(Ok(status)) => {
                status.agent_skill_install
                    && (!upgrade || status.agent_skill_upgrade)
                    && status.installation_ready
                    && status.credential_ready
                    && sandbox.runtime_process_matches(&status).unwrap_or(false)
            }
            Ok(Err(error)) => {
                if let Some(capacity) =
                    layer2_core::sandbox::RuntimeCapacityError::from_adapter(&error)
                {
                    return Err(capacity);
                }
                false
            }
            _ => false,
        })
    }
}

pub(crate) struct AdapterAgentModelSupport(pub Arc<dyn SandboxAdapter>);
#[async_trait::async_trait]
impl layer1_public::rest::AgentModelRuntimeSupport for AdapterAgentModelSupport {
    async fn supported(&self, sandbox: &Sandbox) -> bool {
        self.checked_supported(sandbox).await.unwrap_or(false)
    }
    async fn checked_supported(
        &self,
        sandbox: &Sandbox,
    ) -> Result<bool, layer2_core::sandbox::RuntimeCapacityError> {
        let Some(binding) = &sandbox.runtime else {
            return Ok(false);
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.0
                .runtime_status(&RuntimeHandle::new(&binding.runtime_id)),
        )
        .await;
        Ok(match result {
            Ok(Ok(status)) => {
                status.agent_model_catalog
                    && status.installation_ready
                    && status.credential_ready
                    && sandbox.runtime_process_matches(&status).unwrap_or(false)
            }
            Ok(Err(error)) => {
                if let Some(capacity) =
                    layer2_core::sandbox::RuntimeCapacityError::from_adapter(&error)
                {
                    return Err(capacity);
                }
                false
            }
            _ => false,
        })
    }
}
