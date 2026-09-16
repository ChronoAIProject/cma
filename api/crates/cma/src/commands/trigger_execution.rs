use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, AgentActor, AgentSourceKind};

pub(super) fn is_trigger(command: &AdmittedAgentCommand) -> bool {
    matches!(command.receipt.envelope.actor, AgentActor::Trigger { .. })
        || command.receipt.envelope.source.kind == AgentSourceKind::Trigger
}
