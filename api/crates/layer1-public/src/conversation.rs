//! Shared service composition for Agent conversation projections.

use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use layer2_core::sandbox::{
    AgentContextError, AgentContextService, AgentLiveSubscription, AgentOperationService,
    AgentReadError, AgentReadService, AgentUpdateRecorder,
};

use crate::rest::instance_problem::InstanceProblem;
use crate::rest::{problem_document, problem_response};

#[derive(Clone)]
pub struct AgentConversationState {
    pub(crate) reads: Arc<AgentReadService>,
    pub(crate) operations: Arc<AgentOperationService>,
    pub(crate) context: Arc<AgentContextService>,
    updates: Option<Arc<dyn AgentUpdateRecorder>>,
}

impl AgentConversationState {
    pub fn new(
        reads: Arc<AgentReadService>,
        operations: Arc<AgentOperationService>,
        context: Arc<AgentContextService>,
    ) -> Self {
        Self {
            reads,
            operations,
            context,
            updates: None,
        }
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
