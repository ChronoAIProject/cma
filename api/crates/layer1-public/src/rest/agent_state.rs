use std::sync::Arc;

use super::super::agent_history::AgentHistoryExporter;
use super::super::agent_lifecycle::{AgentLifecycleOperationReader, AgentLifecycleProvisioner};
use layer2_core::sandbox::{AgentOperationService, AgentReadService, AgentService};
use layer2_core::OperationStore;

#[derive(Clone)]
pub struct AgentRoutesState {
    pub(in crate::rest) message_feedback:
        Option<Arc<layer2_core::sandbox::message_feedback::MessageFeedbackService>>,
    pub(in crate::rest) skill_support:
        Option<Arc<dyn super::super::agent_skills::AgentSkillRuntimeSupport>>,
    pub(in crate::rest) commands: Option<Arc<layer2_core::sandbox::agent_io::AgentCommandService>>,
    pub(in crate::rest) agents: Arc<AgentService>,
    pub(in crate::rest) reads: Arc<AgentReadService>,
    pub(in crate::rest) agent_operations: Arc<AgentOperationService>,
    pub(in crate::rest) operation_records: Arc<dyn OperationStore>,
    pub(in crate::rest) lifecycle: Option<Arc<dyn AgentLifecycleProvisioner>>,
    pub(in crate::rest) lifecycle_operations: Option<Arc<dyn AgentLifecycleOperationReader>>,
    pub(in crate::rest) history_exporter: Option<Arc<dyn AgentHistoryExporter>>,
}

impl AgentRoutesState {
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
        agents: Arc<AgentService>,
        reads: Arc<AgentReadService>,
        agent_operations: Arc<AgentOperationService>,
        operation_records: Arc<dyn OperationStore>,
    ) -> Self {
        Self {
            message_feedback: None,
            skill_support: None,
            commands: None,
            agents,
            reads,
            agent_operations,
            operation_records,
            lifecycle: None,
            lifecycle_operations: None,
            history_exporter: None,
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
