use super::*;
use crate::bridge::channel_control::ChannelControlCommand;
use crate::sandbox::{AgentOperationService, AgentRuntimeTarget};
use crate::RuntimeCapability;

/// Advisory local availability after source validation. This is neither an
/// ownership grant nor provider evidence, and never writes or sends input.
pub(super) async fn require_local(
    operations: &AgentOperationService,
    proof: &BridgeInvokeProof,
    snapshot: &ChannelControlExecutionSnapshot,
    deadline: tokio::time::Instant,
) -> Result<(), Error> {
    proof.require_current().map_err(invoke_error)?;
    let intent = &snapshot.admitted.sidecar.intent;
    let agent = proof
        .resources()
        .lineage
        .agent
        .as_ref()
        .ok_or(Error::SourceUnavailable)?;
    let target = AgentRuntimeTarget {
        agent_id: agent.id.clone(),
        conversation_id: intent.runtime_conversation_id.clone(),
        runtime_generation: intent.runtime_generation,
    };
    let capability = match intent.request.command {
        ChannelControlCommand::Cancel => RuntimeCapability::Interrupt,
        ChannelControlCommand::Steer { .. } => RuntimeCapability::Steer,
    };
    let deadline = deadline.min(proof.deadline());
    if tokio::time::Instant::now() >= deadline {
        return Err(Error::Unavailable);
    }
    let connection = tokio::time::timeout_at(deadline, operations.runtimes.resolve(&target))
        .await
        .map_err(|_| Error::Unavailable)?
        .map_err(|_| Error::Unavailable)?;
    // A replica's attachment capabilities are availability, not canonical
    // source policy. Their absence must never permanently refuse the control.
    connection
        .capabilities
        .require([RuntimeCapability::Conversation, capability])
        .map_err(|_| Error::Unavailable)?;
    proof.require_current().map_err(invoke_error)?;
    if tokio::time::Instant::now() >= deadline {
        return Err(Error::Unavailable);
    }
    Ok(())
}
