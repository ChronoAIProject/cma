//! Resolved intents enter the current owner; observation stays with the caller.
use std::{sync::Arc, time::Duration};

use crate::{resource_access::CallerContext, AgentId};

use super::*;

mod admission;
mod evidence;
mod execution;
pub use evidence::OwnerCommandRoutingEvidence;
mod preflight;
mod validation;

/// Intent only: no captured resource snapshot or execution authority crosses
/// this port. The receiver authenticates the original credential independently.
#[derive(Debug, Clone)]
pub enum OwnerCommandMutation {
    Admit {
        submission: CommandSubmission,
        /// None: ordinary admission; Some(None): observed absent policy;
        /// Some(Some(revision)): exact policy revision observed by the caller.
        binding_policy: Option<Option<u64>>,
    },
    TargetedControl(TargetedControlSubmission),
    DirectTargetedControl(TargetedControlSubmission),
    InterruptingMessage(InterruptingMessageSubmission),
    QueuedInput(QueuedInputSubmission),
    QueuedCancellation(QueuedCancellationSubmission),
}

/// Preserve business refusals separately from an uncertain routing outcome.
#[derive(Debug)]
pub enum OwnerCommandReply {
    Admit(Result<CommandStatusView, CommandAdmissionError>),
    TargetedControl(Result<TargetedControlStatusView, TargetedControlError>),
    DirectTargetedControl(Result<TargetedControlStatusView, CommandAdmissionError>),
    InterruptingMessage(Result<InterruptingMessageStatusView, TargetedControlError>),
    QueuedInput(Result<QueuedInputStatusView, QueuedCancellationError>),
    QueuedCancellation(Result<QueuedCancellationStatusView, QueuedCancellationError>),
}

#[async_trait::async_trait]
pub trait AgentCommandOwnerRouting: Send + Sync {
    /// The implementation retains original credential custody, validates the
    /// live destination, and makes at most one remote attempt. The supplied
    /// context is local trusted input, never serialized as receiver authority.
    /// A missing request origin must fail closed unless the adapter independently
    /// proves its noninteractive origin. No outcome permits local fallback.
    async fn route(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<OwnerCommandReply, CommandAdmissionError>;
}

impl AgentCommandService {
    pub fn with_owner_routing(
        self: Arc<Self>,
        routing: Arc<dyn AgentCommandOwnerRouting>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            access: self.access.clone(),
            authority: self.authority.clone(),
            task_experience: self.task_experience.clone(),
            owner_routing: Some(routing),
        })
    }

    /// None is possible only when ownership routing was not composed. A failed
    /// route, mismatched reply or expired permission never falls back locally.
    pub(super) async fn route_mutation(
        &self,
        context: &CallerContext,
        credential: &str,
        source: CommandSubmissionSource,
        mutation: OwnerCommandMutation,
    ) -> Result<Option<OwnerCommandReply>, CommandAdmissionError> {
        let Some(routing) = &self.owner_routing else {
            return Ok(None);
        };
        self.prepare_owner_mutation(context, credential, source, &mutation)
            .await?;
        let reply = tokio::time::timeout(
            Duration::from_secs(60),
            routing.route(context, credential, source, mutation.clone()),
        )
        .await
        .map_err(|_| CommandLedgerError::Unavailable)?;
        // The owner may have durably admitted work even if this final fence
        // refuses the reply. Keep the original key; never submit again here.
        self.authorize_owner_mutation(context, credential, &mutation)
            .await?;
        let reply = reply?;
        mutation.validate_reply(&reply)?;
        Ok(Some(reply))
    }
}

impl OwnerCommandMutation {
    pub fn agent_id(&self) -> &AgentId {
        match self {
            Self::Admit { submission, .. } => &submission.agent_id,
            Self::TargetedControl(value) | Self::DirectTargetedControl(value) => &value.agent_id,
            Self::InterruptingMessage(value) => &value.agent_id,
            Self::QueuedInput(value) => &value.agent_id,
            Self::QueuedCancellation(value) => &value.agent_id,
        }
    }
}
