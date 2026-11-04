//! Shared service composition for Agent conversation projections.

use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::sandbox::{
    agent_io::AgentCommandService, AgentContextError, AgentContextService, AgentLiveSubscription,
    AgentOperationService, AgentReadError, AgentReadService, AgentUpdateRecorder,
};

use crate::rest::instance_problem::InstanceProblem;
use crate::rest::{problem_document, problem_response};

#[cfg(test)]
#[path = "conversation_checkpoint_tests.rs"]
mod checkpoint_tests;

pub(crate) mod commands;

#[derive(serde::Deserialize, utoipa::ToSchema)]
#[allow(dead_code)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnswerContextRequest {
    pub turn_sequence: u64,
    pub answer_sequence: u64,
}

#[cfg(test)]
pub(crate) mod command_test_support;

#[cfg(test)]
#[path = "conversation/commands_tests.rs"]
mod commands_tests;

#[derive(Clone)]
pub struct AgentConversationState {
    pub(crate) agent_page_origin: crate::agent_page::AgentPageOrigin,
    pub(crate) reads: Arc<AgentReadService>,
    #[cfg(test)]
    pub(crate) operations: Arc<AgentOperationService>,
    pub(crate) context: Arc<AgentContextService>,
    pub(crate) commands: Option<Arc<AgentCommandService>>,
    updates: Option<Arc<dyn AgentUpdateRecorder>>,
}

impl AgentConversationState {
    pub fn new(
        agent_page_origin: crate::agent_page::AgentPageOrigin,
        reads: Arc<AgentReadService>,
        _operations: Arc<AgentOperationService>,
        context: Arc<AgentContextService>,
    ) -> Self {
        Self {
            agent_page_origin,
            reads,
            #[cfg(test)]
            operations: _operations,
            context,
            updates: None,
            commands: None,
        }
    }

    pub fn with_commands(mut self, commands: Arc<AgentCommandService>) -> Self {
        self.commands = Some(commands);
        self
    }

    pub fn with_updates(mut self, updates: Arc<dyn AgentUpdateRecorder>) -> Self {
        self.updates = Some(updates);
        self
    }

    pub(crate) fn watch(&self, id: &layer2_core::AgentId) -> Option<AgentLiveSubscription> {
        self.updates.as_ref()?.subscribe_live(id)
    }

    pub(crate) fn has_live_updates(&self) -> bool {
        self.updates.is_some()
    }
}

pub(crate) fn failure(status: StatusCode, code: &str, detail: &str) -> Response {
    problem_response(
        status,
        problem_document(status, &code.replace('_', "-"), detail, code),
    )
}

pub(crate) fn read_failure(error: AgentReadError, missing: &str) -> Response {
    match error {
        AgentReadError::NotFound => failure(
            StatusCode::NOT_FOUND,
            missing,
            "The Agent is not available to this caller.",
        ),
        other => InstanceProblem::from(other).into_response(),
    }
}

pub(crate) fn context_failure(error: AgentContextError) -> Response {
    match error {
        AgentContextError::Forbidden => InstanceProblem::Forbidden.into_response(),
        AgentContextError::SandboxRequired => failure(
            StatusCode::FORBIDDEN,
            "sandbox_required",
            "An owned Sandbox is required; this caller cannot create a default Sandbox.",
        ),
        AgentContextError::InvalidOverride => failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_context_override",
            "Context overrides must name usable resources owned by this caller.",
        ),
        AgentContextError::AdmissionTimeout => failure(
            StatusCode::GATEWAY_TIMEOUT,
            "provisioning_timeout",
            "The context did not become ready within the ten-minute admission bound.",
        ),
        AgentContextError::NotReady => InstanceProblem::AgentNotReady.into_response(),
        AgentContextError::CheckpointInProgress => {
            InstanceProblem::CheckpointInProgress.into_response()
        }
        AgentContextError::Agent(error) => InstanceProblem::from_create(error).into_response(),
        AgentContextError::Sandbox(error) => {
            crate::rest::instances_problem::HierarchyProblem::sandbox(error).into_response()
        }
        AgentContextError::Workspace(error) => {
            crate::rest::instances_problem::HierarchyProblem::workspace(error).into_response()
        }
        AgentContextError::Lifecycle(error) => {
            crate::rest::instances_problem::HierarchyProblem::sandbox_lifecycle(error)
                .into_response()
        }
        AgentContextError::Unavailable => InstanceProblem::Unavailable.into_response(),
    }
}
