//! Adapt S4's certified response interval without deriving facts in a reader.
use layer2_core::sandbox::{
    store::{indexed::AgentOutputTurn, SandboxStoreError},
    AgentLogTurnFacts,
};

pub(super) fn convert(facts: AgentLogTurnFacts) -> Result<AgentOutputTurn, SandboxStoreError> {
    if facts.response_usage.is_some() != facts.response_usage_sequence.is_some() {
        return Err(super::corrupt());
    }
    Ok(AgentOutputTurn {
        legacy_model_known: facts.legacy_model_known,
        usage_sequence: facts.response_usage_sequence,
        terminal_sequence: facts.end_sequence,
        prewrite_sequence: facts.prewrite_failure_sequence,
        facts,
    })
}
