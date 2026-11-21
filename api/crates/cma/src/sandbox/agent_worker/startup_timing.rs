//! Bounded identity and duration only; never retain runtime payloads or credentials.
use layer2_core::sandbox::{Agent, AgentWorkStep};
use layer2_core::AgentId;
use std::time::Instant;

#[derive(Clone, Copy, Default)]
pub(super) struct StartupPhaseDurations {
    pub(super) open_conversation_ms: Option<u64>,
    pub(super) open_context_ms: Option<u64>,
    pub(super) open_intent_ms: Option<u64>,
    pub(super) open_runtime_ms: Option<u64>,
}

pub(super) struct StartupTiming {
    agent_id: AgentId,
    step: AgentWorkStep,
    started: Instant,
    last: Instant,
    durations: StartupPhaseDurations,
}

impl StartupTiming {
    pub(super) fn new(agent: &Agent) -> Self {
        Self {
            agent_id: agent.id.clone(),
            step: agent.work.step,
            started: Instant::now(),
            last: Instant::now(),
            durations: StartupPhaseDurations::default(),
        }
    }

    pub(super) fn completed(&mut self, phase: &'static str) {
        let now = Instant::now();
        let elapsed_ms = now.duration_since(self.last).as_millis() as u64;
        match phase {
            "open_context" => self.durations.open_context_ms = Some(elapsed_ms),
            "open_intent" => self.durations.open_intent_ms = Some(elapsed_ms),
            "open_runtime" => self.durations.open_runtime_ms = Some(elapsed_ms),
            _ => {}
        }
        tracing::info!(
            agent_id = %self.agent_id,
            step = ?self.step,
            phase,
            elapsed_ms,
            "Agent startup phase completed"
        );
        self.last = now;
    }

    pub(super) fn finish(mut self) -> StartupPhaseDurations {
        self.durations.open_conversation_ms = Some(self.started.elapsed().as_millis() as u64);
        self.durations
    }
}
