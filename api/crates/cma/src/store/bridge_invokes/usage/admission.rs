use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, ChannelCommandReservation};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedUsage {
    schema_version: u16,
    event_id: String,
    event_digest: String,
    binding: ChannelBindingReference,
}

impl MongoBridgeInvokeStore {
    pub(in crate::store::bridge_invokes) async fn record_accepted_usage(
        &self,
        request: &ChannelCommandReservation,
        command: &AdmittedAgentCommand,
        created: bool,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        request
            .require_current()
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        request
            .require_result(command)
            .map_err(|_| BridgeInvokeError::TargetMismatch)?;
        let id = &command.receipt.envelope.command_id;
        let document = self
            .resources
            .agent_commands
            .find_one(doc! {"_id":id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let prior: Option<AcceptedUsage> = document
            .get("channel_usage_event")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| BridgeInvokeError::Corrupt)
            })
            .transpose()?;
        if !created && prior.is_none() {
            // Retained pre-integration commands have no proven usage append.
            // Receipt recovery must not invent an old use or refresh its time.
            return Ok(());
        }
        if created && prior.is_some() {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let actor = FencedUsageActor::Accepted {
            reservation: request,
            command,
        };
        let (binding, historical) = match &prior {
            Some(prior) => {
                prior
                    .binding
                    .validate()
                    .map_err(|_| BridgeInvokeError::Corrupt)?;
                if prior.schema_version != 1
                    || prior.binding.bridge_id != request.allocation().identity.bridge_id.as_str()
                    || prior.binding.partition_id != request.allocation().identity.key.hash()
                {
                    return Err(BridgeInvokeError::TargetMismatch.into());
                }
                let row = self
                    .resources
                    .channel_usage_events
                    .find_one(doc! {"_id":&prior.event_id})
                    .session(&mut *session)
                    .await?
                    .ok_or(BridgeInvokeError::Corrupt)?;
                let event = agent_channel_usage::read_usage_event(&row)
                    .map_err(|_| BridgeInvokeError::Corrupt)?;
                if event.digest() != prior.event_digest {
                    return Err(BridgeInvokeError::Corrupt.into());
                }
                let historical = event.predecessor.as_ref() == Some(&prior.binding);
                (prior.binding.clone(), historical)
            }
            None => {
                self.ensure_usage_link(
                    request.proof().grant(),
                    request.allocation(),
                    actor,
                    id.as_str(),
                    session,
                )
                .await?
            }
        };
        let event = if historical {
            agent_channel_usage::bridge_historical::append_historical_use(
                &self.resources,
                &command.receipt.envelope.agent_id,
                actor,
                digest(&("cma.bridge.usage.command.v1", id))?,
                &binding,
                session,
            )
            .await?
        } else {
            append_fenced_change(
                &self.resources,
                &command.receipt.envelope.agent_id,
                actor,
                digest(&("cma.bridge.usage.command.v1", id))?,
                ChannelUsageChange::Use {
                    binding_id: binding.binding_id.clone(),
                },
                ChannelBindingChangeReason::AcceptedCommand,
                session,
            )
            .await?
        };
        if let Some(prior) = prior {
            if event.operation_id != prior.event_id || event.digest() != prior.event_digest {
                return Err(BridgeInvokeError::Corrupt.into());
            }
        } else {
            let retained = AcceptedUsage {
                schema_version: 1,
                event_id: event.operation_id.clone(),
                event_digest: event.digest(),
                binding,
            };
            let changed = self.resources.agent_commands.update_one(
                doc! {"_id":id.as_str(),"revision":command.revision as i64,"channel_usage_event":{"$exists":false}},
                doc! {"$set":{"channel_usage_event":record_bson(&retained).map_err(|_|BridgeInvokeError::Corrupt)?}},
            ).session(session).await?;
            if changed.matched_count != 1 {
                return Err(BridgeInvokeError::Conflict.into());
            }
        }
        request
            .require_current()
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        Ok(())
    }
}
