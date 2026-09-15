use layer2_core::bridge::allocation::AllocationRecord;
use layer2_core::bridge::ingress::StoredInbox;
use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, ChannelCommandSidecar};
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
use layer2_core::sandbox::AgentCreationProvenance;
use layer2_core::AgentId;

use super::*;

mod grant;

pub(super) struct TurnSnapshot {
    resources: AgentProvenanceSnapshot,
    provenance: AgentCreationProvenance,
    pub bridge: BridgeRecord,
    command: AdmittedAgentCommand,
    sidecar: ChannelCommandSidecar,
    inbox: StoredInbox,
    allocation: AllocationRecord,
}
impl TurnSnapshot {
    pub fn observation<'a>(
        &'a self,
        incarnation: &'a str,
        deadline: tokio::time::Instant,
    ) -> Result<ChannelReplyTurnObservation<'a>, OutboxError> {
        let source = OutboxSource::capture(
            &self.bridge,
            &self.inbox,
            &self.allocation,
            &self.command.receipt,
            &self.sidecar,
        )?;
        Ok(ChannelReplyTurnObservation {
            source,
            command: &self.command,
            sidecar: &self.sidecar,
            allocation: &self.allocation,
            agent: self
                .resources
                .resource
                .lineage
                .agent
                .as_ref()
                .ok_or(OutboxError::ToolUnavailable)?,
            sandbox: &self.resources.resource.lineage.sandbox,
            provenance: &self.provenance,
            bridge: &self.bridge,
            capability_incarnation: incarnation,
            deadline,
        })
    }
}

impl MongoBridgeReplyToolStore {
    pub(crate) async fn current_context(
        &self,
        capability: &ChannelReplySessionAuthorization,
    ) -> Result<Option<String>, OutboxError> {
        match self.read_current(capability, None).await {
            Ok(scope) => Ok(Some(scope.context_reference().into())),
            Err(OutboxError::ToolUnavailable) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn current_turn(
        &self,
        capability: &ChannelReplySessionAuthorization,
        context: &str,
    ) -> Result<ChannelReplyTurnAuthorization, OutboxError> {
        self.read_current(capability, Some(context)).await
    }
    async fn read_current(
        &self,
        capability: &ChannelReplySessionAuthorization,
        context: Option<&str>,
    ) -> Result<ChannelReplyTurnAuthorization, OutboxError> {
        capability.require_current()?;
        let mut session = self
            .invokes
            .bridges
            .client
            .start_session()
            .await
            .map_err(|_| OutboxError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| OutboxError::Unavailable)?;
        let result = async {
            let snapshot = self.session_snapshot(capability, &mut session).await?;
            let Some(agent) = &snapshot.resource.lineage.agent else {
                return Err(OutboxError::ToolUnavailable.into());
            };
            if agent.conversation.active_turn.is_none() {
                return Err(OutboxError::ToolUnavailable.into());
            }
            let mut cursor = self
                .invokes
                .resources
                .agent_commands
                .find(doc! {"agent_id":capability.agent_id().as_str(),"status":"in_progress"})
                .limit(2)
                .session(&mut session)
                .await?;
            let mut documents = Vec::new();
            while cursor.advance(&mut session).await? {
                documents.push(cursor.deserialize_current()?);
            }
            if documents.len() != 1 {
                return Err(OutboxError::ToolUnavailable.into());
            }
            let command = crate::store::agent_commands::read_command(&documents[0])
                .map_err(|_| OutboxError::SourceMismatch)?;
            let snapshot = self
                .turn_snapshot(
                    capability.agent_id(),
                    command.receipt.envelope.command_id.as_str(),
                    &mut session,
                )
                .await?;
            let scope = ChannelReplyTurnAuthorization::from_current(
                snapshot.observation(capability.capability_incarnation(), capability.deadline())?,
            )?;
            if scope.namespace() != capability.namespace()
                || scope.runtime_generation() != capability.runtime_generation()
                || context.is_some_and(|context| context != scope.context_reference())
            {
                return Err(OutboxError::ToolUnavailable.into());
            }
            Ok::<_, MutationError>(scope)
        }
        .await;
        let _ = session.abort_transaction().await;
        result.map_err(MutationError::into_core)
    }

    pub(super) async fn session_snapshot(
        &self,
        capability: &ChannelReplySessionAuthorization,
        session: &mut ClientSession,
    ) -> Result<AgentProvenanceSnapshot, MutationError> {
        capability.require_current()?;
        let snapshot = crate::store::agent_provenance::read_snapshot(
            &self.invokes.resources,
            capability.agent_id(),
            session,
        )
        .await?;
        let provenance = snapshot
            .creation_provenance(capability.agent_id())
            .map_err(|_| OutboxError::ToolUnavailable)?;
        capability.require_snapshot(ChannelReplySessionObservation {
            agent: snapshot
                .resource
                .lineage
                .agent
                .as_ref()
                .ok_or(OutboxError::ToolUnavailable)?,
            sandbox: &snapshot.resource.lineage.sandbox,
            provenance: &provenance,
            capability_incarnation: capability.capability_incarnation(),
            runtime_generation: capability.runtime_generation(),
            deadline: capability.deadline(),
        })?;
        Ok(snapshot)
    }

    pub(super) async fn turn_snapshot(
        &self,
        agent_id: &AgentId,
        command_id: &str,
        session: &mut ClientSession,
    ) -> Result<TurnSnapshot, MutationError> {
        let resources = crate::store::agent_provenance::read_snapshot(
            &self.invokes.resources,
            agent_id,
            session,
        )
        .await?;
        let provenance = resources
            .creation_provenance(agent_id)
            .map_err(|_| OutboxError::ToolUnavailable)?;
        let command_document = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id":command_id})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::SourceMismatch)?;
        let command = crate::store::agent_commands::read_command(&command_document)
            .map_err(|_| OutboxError::SourceMismatch)?;
        let sidecar: ChannelCommandSidecar = mongodb::bson::deserialize_from_bson(
            command_document
                .get("channel_admission")
                .ok_or(OutboxError::ToolUnavailable)?
                .clone(),
        )
        .map_err(|_| OutboxError::SourceMismatch)?;
        let inbox =
            super::super::channel_source::read(&self.invokes, &command_document, &sidecar, session)
                .await
                .map_err(source_error)?;
        let allocation_document = self
            .allocations
            .find_one(doc! {"_id":&sidecar.allocation_id})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::SourceMismatch)?;
        let allocation = crate::store::bridge_allocations::decode_record(&allocation_document)
            .map_err(|_| OutboxError::SourceMismatch)?;
        let bridge_document = self
            .invokes
            .bridges
            .definitions
            .find_one(doc! {"_id":inbox.pin.bridge_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(OutboxError::SourceMismatch)?;
        let bridge = crate::store::bridge_definitions::decode_bridge(&bridge_document)
            .map_err(|_| OutboxError::SourceMismatch)?;
        let observed = TurnSnapshot {
            resources,
            provenance,
            bridge,
            command,
            sidecar,
            inbox,
            allocation,
        };
        self.require_turn_grant(&observed, false, session).await?;
        Ok(observed)
    }

    pub(super) async fn fence_turn(
        &self,
        scope: &ChannelReplyTurnAuthorization,
        validation: &BridgeValidation,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let observed = self
            .turn_snapshot(
                &scope.source().target().agent_id,
                scope.source().command_id().as_str(),
                session,
            )
            .await?;
        scope.require_snapshot(
            observed.observation(scope.capability_incarnation(), scope.deadline())?,
        )?;
        let capability =
            ChannelReplySessionAuthorization::from_current(ChannelReplySessionObservation {
                agent: observed
                    .resources
                    .resource
                    .lineage
                    .agent
                    .as_ref()
                    .ok_or(OutboxError::ToolUnavailable)?,
                sandbox: &observed.resources.resource.lineage.sandbox,
                provenance: &observed.provenance,
                capability_incarnation: scope.capability_incarnation(),
                runtime_generation: scope.runtime_generation(),
                deadline: scope.deadline(),
            })?;
        crate::store::channel_tools::validate_and_fence_session(
            &self.invokes.resources,
            &capability,
            session,
        )
        .await?;
        scope.selected_key(validation)?;
        self.require_turn_grant(&observed, true, session).await?;
        self.fence_delivery(&observed.bridge, scope.source(), validation, session)
            .await?;
        crate::store::resource_access::fence_lineage(
            &self.invokes.resources,
            &observed.resources.resource,
            session,
        )
        .await?;
        let provenance = record_bson(&observed.provenance).map_err(|_| OutboxError::Corrupt)?;
        let changed = self
            .invokes
            .resources
            .agent_creation_provenance
            .update_one(
                doc! {"_id":observed.provenance.agent_id.as_str(),"record":provenance},
                doc! {"$inc":{"outbox_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(OutboxError::Conflict.into());
        }
        super::super::channel_source::fence(
            &self.invokes,
            &observed.command,
            &observed.sidecar,
            &observed.inbox,
            session,
        )
        .await
        .map_err(source_error)?;
        for (collection, filter) in [
            (
                &self.allocations,
                doc! {"_id":&observed.allocation.id,"revision":observed.allocation.revision as i64},
            ),
            (
                &self.invokes.resources.agent_commands,
                doc! {"_id":observed.command.receipt.envelope.command_id.as_str(),"revision":observed.command.revision as i64},
            ),
        ] {
            let changed = collection
                .update_one(filter, doc! {"$inc":{"outbox_fence":1_i64}})
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(OutboxError::Conflict.into());
            }
        }
        scope.require_current()?;
        Ok(())
    }

    pub(super) async fn fence_delivery(
        &self,
        bridge: &BridgeRecord,
        source: &OutboxSource,
        validation: &BridgeValidation,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        source.require_bridge(bridge)?;
        self.invokes
            .bridges
            .credentials
            .require_retained_binding(bridge, source.config(), validation.custody_pin(), session)
            .await
            .map_err(|_| OutboxError::AuthorityUnavailable)?;
        let changed = self
            .invokes
            .bridges
            .definitions
            .update_one(
                doc! {"_id":bridge.id.as_str(),"revision":bridge.revision as i64},
                doc! {"$inc":{"outbox_fence":1_i64}},
            )
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(OutboxError::Conflict.into());
        }
        Ok(())
    }
}

fn source_error(error: super::super::channel::MutationError) -> MutationError {
    match error {
        super::super::channel::MutationError::Driver(error) => MutationError::Driver(error),
        super::super::channel::MutationError::Core(_) => {
            MutationError::Core(OutboxError::SourceMismatch)
        }
    }
}
