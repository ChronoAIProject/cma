use std::sync::Arc;

use super::super::agent_history::AgentHistoryExporter;
use super::super::agent_lifecycle::{AgentLifecycleOperationReader, AgentLifecycleProvisioner};
use layer2_core::profiles::SandboxProfileStore;
use layer2_core::sandbox::{AgentOperationService, AgentReadService, AgentService};
use layer2_core::OperationStore;

pub(super) async fn list_private_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        "private, no-store".parse().unwrap(),
    );
    response
}

#[derive(Clone)]
pub struct AgentRoutesState {
    pub(in crate::rest) suggestions:
        Option<Arc<layer2_core::sandbox::suggestions::SuggestionService>>,
    pub(crate) agent_page_origin: crate::agent_page::AgentPageOrigin,
    pub(in crate::rest) message_feedback:
        Option<Arc<layer2_core::sandbox::message_feedback::MessageFeedbackService>>,
    pub(in crate::rest) skill_support:
        Option<Arc<dyn super::super::agent_skills::AgentSkillRuntimeSupport>>,
    pub(in crate::rest) commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    pub(in crate::rest) agents: Arc<AgentService>,
    pub(in crate::rest) owner_agent_restart:
        Option<Arc<dyn layer2_core::sandbox::AgentRestartControl>>,
    pub(in crate::rest) reads: Arc<AgentReadService>,
    pub(in crate::rest) agent_operations: Arc<AgentOperationService>,
    pub(in crate::rest) operation_records: Arc<dyn OperationStore>,
    pub(in crate::rest) lifecycle: Option<Arc<dyn AgentLifecycleProvisioner>>,
    pub(in crate::rest) lifecycle_operations: Option<Arc<dyn AgentLifecycleOperationReader>>,
    pub(in crate::rest) history_exporter: Option<Arc<dyn AgentHistoryExporter>>,
    #[cfg(feature = "qualification-hooks")]
    control_checkpoint: Option<Arc<dyn layer2_core::qualification::ControlCheckpoint>>,
    pub(super) sandbox_profile_readability: Option<Arc<dyn SandboxProfileStore>>,
}

impl AgentRoutesState {
    pub fn with_owner_agent_restart(
        mut self,
        service: Arc<dyn layer2_core::sandbox::AgentRestartControl>,
    ) -> Self {
        self.owner_agent_restart = Some(service);
        self
    }

    pub fn with_suggestions(
        mut self,
        store: Arc<dyn layer2_core::sandbox::suggestions::SuggestionStore>,
    ) -> Self {
        self.suggestions = Some(layer2_core::sandbox::suggestions::SuggestionService::new(
            self.reads.clone(),
            store,
        ));
        self
    }
    pub fn with_message_feedback(
        mut self,
        store: Arc<dyn layer2_core::sandbox::message_feedback::MessageFeedbackStore>,
    ) -> Self {
        self.message_feedback = Some(
            layer2_core::sandbox::message_feedback::MessageFeedbackService::new(
                self.reads.clone(),
                store,
            ),
        );
        self
    }

    pub fn with_skill_support(
        mut self,
        support: Arc<dyn super::super::agent_skills::AgentSkillRuntimeSupport>,
    ) -> Self {
        self.skill_support = Some(support);
        self
    }

    pub fn new(
        agent_page_origin: crate::agent_page::AgentPageOrigin,
        agents: Arc<AgentService>,
        reads: Arc<AgentReadService>,
        agent_operations: Arc<AgentOperationService>,
        operation_records: Arc<dyn OperationStore>,
    ) -> Self {
        Self {
            suggestions: None,
            agent_page_origin,
            message_feedback: None,
            skill_support: None,
            commands: None,
            agents,
            owner_agent_restart: None,
            reads,
            agent_operations,
            operation_records,
            lifecycle: None,
            lifecycle_operations: None,
            history_exporter: None,
            #[cfg(feature = "qualification-hooks")]
            control_checkpoint: None,
            sandbox_profile_readability: None,
        }
    }

    /// Attach the simplified inline lifecycle facade without changing the
    /// retained hierarchy routes used by compatibility clients and tests.
    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn AgentLifecycleProvisioner>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub fn with_lifecycle_operations(
        mut self,
        reader: Arc<dyn AgentLifecycleOperationReader>,
    ) -> Self {
        self.lifecycle_operations = Some(reader);
        self
    }

    pub fn with_history_exporter(mut self, exporter: Arc<dyn AgentHistoryExporter>) -> Self {
        self.history_exporter = Some(exporter);
        self
    }

    pub fn with_sandbox_profile_readability(
        mut self,
        profiles: Arc<dyn SandboxProfileStore>,
    ) -> Self {
        self.sandbox_profile_readability = Some(profiles);
        self
    }

    /// Production adapters share the durable command ledger. Older service
    /// compositions keep their established operation behavior until migrated.
    pub fn with_commands(
        mut self,
        commands: Arc<layer2_core::sandbox::agent_io::AgentCommandService>,
    ) -> Self {
        self.commands = Some(commands);
        self
    }
}

#[cfg(feature = "qualification-hooks")]
impl AgentRoutesState {
    pub fn with_control_checkpoint(
        mut self,
        checkpoint: Arc<dyn layer2_core::qualification::ControlCheckpoint>,
    ) -> Self {
        self.control_checkpoint = Some(checkpoint);
        self
    }

    pub(in crate::rest) fn qualification_for(
        &self,
        guard: &super::super::idempotency::Idempotency,
        operation: layer2_core::qualification::ControlOperation,
        agent: &layer2_core::AgentId,
    ) -> Option<(
        Arc<dyn layer2_core::qualification::ControlCheckpoint>,
        layer2_core::qualification::ControlContext,
    )> {
        let super::super::idempotency::Idempotency::Acquired { claim, .. } = guard else {
            return None;
        };
        let checkpoint = self.control_checkpoint.as_ref()?;
        let context =
            layer2_core::qualification::ControlContext::from_claim(operation, agent, claim)?;
        Some((Arc::clone(checkpoint), context))
    }
}
