//! Transaction primitive for production Bridge usage. The enclosing Bridge
//! transaction must first validate/fence the exact binding action. This helper
//! owns no transaction and derives attribution only from typed current evidence.
use layer2_core::{
    bridge::{BridgeId, BridgeManagementLease},
    sandbox::agent_io::{AdmittedAgentCommand, ChannelCommandReservation},
};

use super::*;

/// Accepted input always means a durable common command, never callback receipt
/// alone. A management lease is for an already fenced attach/detach/reset action.
#[derive(Clone, Copy)]
pub(in crate::store) enum FencedUsageActor<'a> {
    Management(&'a BridgeManagementLease),
    Accepted {
        reservation: &'a ChannelCommandReservation,
        command: &'a AdmittedAgentCommand,
    },
}

impl FencedUsageActor<'_> {
    fn identity(
        &self,
        snapshot: &AgentChannelUsageSnapshot,
    ) -> Result<(layer2_core::resource_access::ResourceActor, BridgeId), UsageError> {
        let creation = &snapshot.state.creation;
        match self {
            Self::Management(lease) => {
                lease.require_current().map_err(|_| conflict())?;
                if lease.record().owner != creation.owner {
                    return Err(conflict());
                }
                Ok((lease.actor().clone(), lease.record().id.clone()))
            }
            Self::Accepted {
                reservation,
                command,
            } => {
                reservation.require_current().map_err(|_| conflict())?;
                reservation
                    .require_result(command)
                    .map_err(|_| conflict())?;
                let proof = reservation.proof();
                if proof.grant().owner != creation.owner
                    || proof.grant().target.ids.agent_id != creation.agent_id
                    || proof.resources().fence() != snapshot.provenance.resource.fence()
                {
                    return Err(conflict());
                }
                Ok((
                    command.receipt.envelope.actor.clone(),
                    proof.grant().pin.bridge_id.clone(),
                ))
            }
        }
    }
}

/// Append in the caller's transaction with the same permanent operation key.
/// On recovery, use the event's original expected revision and timestamp: later
/// usage must not turn an identical retry into a new event or a false conflict.
///
/// `key_sha256` names the authoritative action, such as the immutable grant
/// receipt/action or accepted command ID, with its own schema/domain prefix.
/// The caller must prove the exact retained binding link separately; these
/// history records never confer grant, allocation or dispatch authority.
pub(in crate::store) async fn append_fenced_change(
    store: &MongoSandboxStore,
    id: &AgentId,
    authority: FencedUsageActor<'_>,
    key_sha256: String,
    change: ChannelUsageChange,
    reason: ChannelBindingChangeReason,
    session: &mut ClientSession,
) -> Result<ChannelUsageEvent, UsageError> {
    let snapshot = read_snapshot(store, id, session).await?;
    let (actor, bridge_id) = authority.identity(&snapshot)?;
    if matches!(&authority, FencedUsageActor::Accepted { .. })
        && !matches!(
            (&change, reason),
            (
                ChannelUsageChange::Use { .. },
                ChannelBindingChangeReason::AcceptedCommand
            ) | (
                ChannelUsageChange::Attach { .. },
                ChannelBindingChangeReason::Attachment
            )
        )
    {
        return Err(conflict());
    }
    if let FencedUsageActor::Accepted { reservation, .. } = &authority {
        if change.introduced_binding().is_some_and(|binding| {
            binding.partition_id != reservation.allocation().identity.key.hash()
        }) {
            return Err(conflict());
        }
    }
    if matches!(&authority, FencedUsageActor::Management(_))
        && matches!(change, ChannelUsageChange::Use { .. })
    {
        return Err(conflict());
    }
    let mut request = ChannelUsageMutation {
        creation: snapshot.state.creation.clone(),
        policy_fence: snapshot.provenance.resource.fence(),
        expected_revision: snapshot.state.revision,
        key_sha256,
        actor,
        change,
        reason,
        at: layer2_core::now_rfc3339(),
    };
    request.validate()?;
    let prior = store
        .channel_usage_events
        .find_one(doc! {"_id": request.operation_id()})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read_event)
        .transpose()?;
    let before = match &prior {
        Some(event) => {
            event.validate()?;
            request.expected_revision = event.before.revision;
            request.at = event.at.clone();
            // Check every original fact, including actor and change, before
            // letting historical state supply any replay identity.
            request.replay(event.clone())?;
            &event.before
        }
        None => &snapshot.state,
    };
    require_bridge(&request.change, before, &bridge_id)?;
    let (event, _) = apply(store, &request, session).await?;
    // Expiry during the store writes aborts with the enclosing transaction.
    authority.identity(&snapshot)?;
    Ok(event)
}

fn require_bridge(
    change: &ChannelUsageChange,
    before: &AgentChannelUsageState,
    bridge_id: &BridgeId,
) -> Result<(), UsageError> {
    let introduced = change.introduced_binding();
    if introduced.is_some_and(|binding| binding.bridge_id != bridge_id.as_str()) {
        return Err(conflict());
    }
    let outgoing = match change {
        ChannelUsageChange::Use { binding_id } | ChannelUsageChange::Detach { binding_id } => {
            Some(binding_id)
        }
        ChannelUsageChange::Replace {
            before_binding_id, ..
        } => Some(before_binding_id),
        ChannelUsageChange::Attach { .. } => None,
    };
    if outgoing.is_some_and(|id| {
        before.active_bindings.iter().all(|active| {
            &active.reference.binding_id != id || active.reference.bridge_id != bridge_id.as_str()
        })
    }) {
        return Err(conflict());
    }
    Ok(())
}

fn conflict() -> UsageError {
    UsageError::Domain(SandboxStoreError::Conflict)
}
