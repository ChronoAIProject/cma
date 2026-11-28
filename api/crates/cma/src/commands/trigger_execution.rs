use layer2_core::sandbox::agent_io::AdmittedAgentCommand;

pub(super) fn is_first_message(command: &AdmittedAgentCommand) -> bool {
    command.receipt.envelope.is_first_message()
}
