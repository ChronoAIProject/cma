use layer2_core::sandbox::agent_io::{
    AgentActor, AgentCommand, AgentOwner, AgentSourceKind, ChannelCommandAdmissionError as Error,
    ChannelExecutionFence,
};
use layer2_core::sandbox::store::AgentConversationMutation;
use layer2_core::sandbox::{Agent, AgentOperationDelivery};
use mongodb::{bson::doc, ClientSession};

use super::super::MongoBridgeInvokeStore;
use super::MutationError;
use crate::store::agent_commands::read_command;

impl MongoBridgeInvokeStore {
    /// Shares the Agent/log transaction. The replacement message remains an
    /// ordinary queued command, and one receipt binds it to one original turn.
    pub(crate) async fn fence_channel_supersession(
        &self,
        fence: &ChannelExecutionFence,
        current: &Agent,
        mutation: &AgentConversationMutation,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        fence
            .validate_supersession(current, mutation)
            .map_err(|_| Error::SourceMismatch)?;
        let command = fence.superseding_command().ok_or(Error::SourceMismatch)?;
        self.fence_channel_execution(fence, command, session)
            .await?;
        let operation = mutation
            .agent
            .conversation
            .operation
            .as_ref()
            .ok_or(Error::SourceMismatch)?;
        let original_document = self
            .resources
            .agent_commands
            .find_one(doc! {
                "agent_id": current.id.as_str(),
                "record.work.dispatch.turn_sequence": operation.turn_sequence as i64,
                "record.work.dispatch.runtime_generation": operation.runtime_generation as i64,
                "record.receipt.envelope.command.type": "message",
            })
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceMismatch)?;
        let original = read_command(&original_document).map_err(Error::from)?;
        let original_dispatch = original
            .work
            .dispatch
            .as_ref()
            .ok_or(Error::SourceMismatch)?;
        if !matches!(
            original.receipt.envelope.command,
            AgentCommand::Message { .. }
        ) || original.receipt.agent_order >= command.receipt.agent_order
            || command.receipt.admitted_at < original_dispatch.fenced_at
            || original.receipt.envelope.owner != command.receipt.envelope.owner
            || original.receipt.envelope.agent_id != command.receipt.envelope.agent_id
        {
            return Err(Error::SourceMismatch.into());
        }
        // Owner website turns share this channel-created Agent, but carry no
        // channel source themselves. Retain the latest preceding channel input
        // as the sender/chat anchor; an owner actor alone cannot widen the scope.
        let source_command = match (
            &original.receipt.envelope.actor,
            original.receipt.envelope.source.kind,
        ) {
            (AgentActor::Channel { .. }, AgentSourceKind::Channel) => original.clone(),
            (AgentActor::Human { subject }, AgentSourceKind::UserInterface)
            | (
                AgentActor::Api {
                    subject,
                    delegation_id: None,
                },
                AgentSourceKind::Api,
            ) if original.receipt.envelope.owner
                == (AgentOwner::Person {
                    subject: subject.clone(),
                })
                && subject == &current.owner
                && matches!(
                    fence.source().inbox().envelope.platform.as_str(),
                    "telegram" | "telegram_new"
                ) =>
            {
                let prior = self
                    .resources
                    .agent_commands
                    .find_one(doc! {
                        "agent_id": current.id.as_str(),
                        "agent_order": {"$lt": original.receipt.agent_order as i64},
                        "record.receipt.envelope.command.type": "message",
                        "record.receipt.envelope.actor.kind": "channel",
                        "record.receipt.envelope.source.kind": "channel",
                    })
                    .sort(doc! {"agent_order": -1_i32})
                    .session(&mut *session)
                    .await?
                    .ok_or(Error::SourceMismatch)?;
                read_command(&prior).map_err(Error::from)?
            }
            _ => return Err(Error::SourceMismatch.into()),
        };
        if source_command.receipt.envelope.actor != command.receipt.envelope.actor
            || source_command.receipt.envelope.owner != command.receipt.envelope.owner
            || source_command.receipt.envelope.agent_id != command.receipt.envelope.agent_id
        {
            return Err(Error::SourceMismatch.into());
        }
        let source = self.read_channel_source(&source_command, session).await?;
        if source.allocation.id != fence.source().allocation().id
            || source.inbox.envelope.conversation_type != "private"
            || fence.source().inbox().envelope.conversation_type != "private"
            || source.allocation.identity != fence.source().allocation().identity
            || source.inbox.pin != fence.source().inbox().pin
            || source.inbox.accepted_at > fence.source().inbox().accepted_at
            || layer2_core::seconds_from_rfc3339(&original_dispatch.fenced_at)
                .is_none_or(|at| fence.source().inbox().accepted_at < at as i64)
        {
            return Err(Error::SourceMismatch.into());
        }
        let document = self
            .resources
            .agent_commands
            .find_one(doc! {
                "_id": command.receipt.envelope.command_id.as_str(),
            })
            .session(&mut *session)
            .await?
            .ok_or(Error::SourceMismatch)?;
        if let Ok(previous) = document.get_document("channel_supersession") {
            let retry = current.conversation.operation.as_ref().is_some_and(|old| {
                old.state == AgentOperationDelivery::PreWriteFailed
                    && previous.get_i64("operation_sequence").ok() == Some(old.sequence as i64)
                    && previous.get_i64("turn_sequence").ok()
                        == Some(operation.turn_sequence as i64)
                    && previous.get_i64("runtime_generation").ok()
                        == Some(operation.runtime_generation as i64)
            });
            if !retry {
                return Err(Error::SourceMismatch.into());
            }
        } else if document.contains_key("channel_supersession") {
            return Err(Error::SourceMismatch.into());
        }
        let changed = self
            .resources
            .agent_commands
            .update_one(
                doc! {
                    "_id": command.receipt.envelope.command_id.as_str(),
                    "revision": command.revision as i64,
                },
                doc! {"$set": {"channel_supersession": {
                    "source_command_id": original.receipt.envelope.command_id.as_str(),
                    "channel_source_command_id": source_command.receipt.envelope.command_id.as_str(),
                    "allocation_id": &source.allocation.id,
                    "operation_sequence": operation.sequence as i64,
                    "turn_sequence": operation.turn_sequence as i64,
                    "runtime_generation": operation.runtime_generation as i64,
                    "at": &mutation.appended_at,
                }}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(Error::SourceMismatch.into());
        }
        fence.require_current()?;
        Ok(())
    }
}
