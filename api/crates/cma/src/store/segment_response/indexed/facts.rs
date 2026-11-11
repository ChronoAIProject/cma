//! S4 owns the certified interval facts. Keep the pending handoff isolated here.
use layer2_core::sandbox::{
    store::{indexed::AgentOutputTurn, SandboxStoreError},
    AgentLogTurnFacts,
};

pub(super) fn convert(_facts: AgentLogTurnFacts) -> Result<AgentOutputTurn, SandboxStoreError> {
    // Historical model knowledge and post-terminal legacy usage cannot be
    // inferred from the current aggregate or today's incomplete turn facts.
    Err(SandboxStoreError::Unavailable(
        "certified output interval facts are not yet available".into(),
    ))
}
