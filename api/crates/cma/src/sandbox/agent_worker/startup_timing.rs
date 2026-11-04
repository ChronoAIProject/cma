//! Bounded identity and duration only; never retain runtime payloads or credentials.
use layer2_core::sandbox::{Agent, AgentWorkStep};
use layer2_core::AgentId;
use std::time::Instant;

pub(super) struct StartupTiming {
    agent_id: AgentId,
    step: AgentWorkStep,
    last: Instant,
}

impl StartupTiming {
    pub(super) fn new(agent: &Agent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            step: agent.work.step,
            last: Instant::now(),
        }
    }

    pub(super) fn completed(&mut self, phase: &'static str) {
        let now = Instant::now();
        tracing::info!(
            agent_id = %self.agent_id,
            step = ?self.step,
            phase,
            elapsed_ms = now.duration_since(self.last).as_millis() as u64,
            "Agent startup phase completed"
        );
        self.last = now;
    }
}
