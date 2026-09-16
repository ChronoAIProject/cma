use crate::ids::AgentResponseId;
use crate::resource_access::{
    AccessPurpose, CallerContext, ResourceAccessError, ResourceCapability, ResourceTarget,
};
use crate::{AgentId, RuntimeCapabilities, RuntimeCapability};

use super::super::admission_service::bounded;
use super::super::{
    AgentCommand, AgentCommandService, CommandAdmissionError, CommandLedgerError, CommandStatusView,
};
use super::TargetedResponse;

/// Runtime support only. This observation grants no operation authority and
/// does not claim that an approval is pending or the target remains active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseControlRuntimeSupport {
    pub cancel: bool,
    pub steer: bool,
    pub resolve_approval: bool,
}

impl ResponseControlRuntimeSupport {
    pub(super) fn from_runtime(runtime: &RuntimeCapabilities) -> Self {
        let conversation = runtime.supports(RuntimeCapability::Conversation);
        Self {
            cancel: conversation && runtime.supports(RuntimeCapability::Interrupt),
            steer: conversation && runtime.supports(RuntimeCapability::Steer),
            resolve_approval: conversation,
        }
    }

    pub(super) fn supports(&self, command: &AgentCommand) -> bool {
        match command {
            AgentCommand::Interrupt => self.cancel,
            AgentCommand::Steer { .. } => self.steer,
            AgentCommand::AnswerApproval { .. } => self.resolve_approval,
            AgentCommand::Message { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseControlObservation {
    pub response: CommandStatusView,
    pub target: Option<TargetedResponse>,
    pub agent_revision: u64,
    pub target_active: bool,
    pub runtime_support: ResponseControlRuntimeSupport,
}

impl AgentCommandService {
    /// Safe history read scoped to the exact current Agent and owner. Queued
    /// responses have no target until a durable dispatch binds a turn.
    pub async fn observe_response_controls(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        agent_id: &AgentId,
        response_id: &AgentResponseId,
    ) -> Result<ResponseControlObservation, CommandAdmissionError> {
        let resource = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        let capability = ResourceCapability::ViewHistory;
        let authorized = bounded(self.access.authorize(
            context,
            credential,
            &resource,
            capability,
            AccessPurpose::Read,
        ))
        .await??;
        let result = self
            .with_authority_bound(
                &authorized,
                capability,
                self.store.get_response_command(response_id),
            )
            .await;
        self.fence(&authorized, &resource, capability).await?;
        let record = result??.ok_or(CommandLedgerError::NotFound)?;
        record.validate()?;
        let agent = authorized
            .snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(CommandLedgerError::Corrupt)?;
        let owner = authorized
            .snapshot
            .scope(&resource)
            .map_err(ResourceAccessError::from)?
            .owner;
        let parse = |value: &str| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                .map_err(|_| CommandLedgerError::Corrupt)
        };
        if &record.receipt.envelope.agent_id != agent_id
            || record.receipt.envelope.owner != owner
            || record.receipt.response_id.as_ref() != Some(response_id)
            || !matches!(
                record.receipt.envelope.command,
                AgentCommand::Message { .. }
            )
            || parse(&record.receipt.admitted_at)? < parse(&agent.created_at)?
        {
            return Err(CommandLedgerError::NotFound.into());
        }
        let response = CommandStatusView::from_record(&record);
        let target = response
            .turn_sequence
            .map(|turn_sequence| TargetedResponse {
                response_id: response_id.clone(),
                turn_sequence,
            });
        Ok(ResponseControlObservation {
            target_active: !response.status.is_terminal()
                && target.as_ref().is_some_and(|target| {
                    agent.conversation.active_turn == Some(target.turn_sequence)
                }),
            response,
            target,
            agent_revision: agent.revision,
            runtime_support: ResponseControlRuntimeSupport::from_runtime(&agent.capabilities),
        })
    }
}
