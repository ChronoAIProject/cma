//! Bounded, process-local receipt state; durable checkpoint version 1 stays unchanged.
use std::time::Duration;

use layer3_adapter::RuntimeOutputSource;
use tokio::time::Instant;

use crate::sandbox::{AgentConversationError, AgentMessageAssembly};

/// Immutable rollout policy injected by the application, never read from the environment here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentOutputCoalescePolicy {
    pub(super) max_age: Duration,
    pub(super) max_frames: usize,
}

impl AgentOutputCoalescePolicy {
    pub fn new(max_age: Duration, max_frames: usize) -> Result<Self, AgentConversationError> {
        // Keep pending receipt metadata below 1 MiB, far below the runtime journal limits.
        if !(Duration::from_millis(1)..=Duration::from_millis(1000)).contains(&max_age)
            || !(1..=128).contains(&max_frames)
        {
            return Err(AgentConversationError::InvalidState(
                "quiet output bounds require 1..=1000 milliseconds and 1..=128 frames",
            ));
        }
        Ok(Self {
            max_age,
            max_frames,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuietOutputBuffer {
    pub sources: Vec<RuntimeOutputSource>,
    pub generation: u64,
    pub deadline: Instant,
}

impl AgentMessageAssembly {
    pub fn output_flush_deadline(&self) -> Option<Instant> {
        self.quiet_output.as_ref().map(|buffer| buffer.deadline)
    }

    pub fn buffered_output(&self) -> Option<&RuntimeOutputSource> {
        self.quiet_output
            .as_ref()
            .and_then(|buffer| buffer.sources.last())
    }

    pub(super) fn retain_quiet(
        &mut self,
        previous: &Self,
        source: &RuntimeOutputSource,
        generation: u64,
        policy: AgentOutputCoalescePolicy,
    ) -> bool {
        let now = Instant::now();
        let mut buffer = previous
            .quiet_output
            .clone()
            .unwrap_or_else(|| QuietOutputBuffer {
                sources: Vec::new(),
                generation,
                // An idle gap does not reset the time since the last durable cursor.
                deadline: previous.output_committed_at.unwrap_or(now) + policy.max_age,
            });
        buffer.sources.push(source.clone());
        if buffer.sources.len() >= policy.max_frames || now >= buffer.deadline {
            return false;
        }
        self.quiet_output = Some(buffer);
        self.output_committed_at = previous.output_committed_at;
        true
    }

    pub(super) fn durable_copy(&self) -> Self {
        let mut durable = self.clone();
        durable.quiet_output = None;
        durable.output_committed_at = None;
        durable.pending_update = None;
        durable
    }
}

pub(super) enum RecordInput<'a> {
    Update(&'a layer3_adapter::ConversationUpdate, bool),
    Flush(&'a RuntimeOutputSource),
}

impl RecordInput<'_> {
    pub fn parts(
        &self,
    ) -> (
        Option<&RuntimeOutputSource>,
        Option<&layer3_adapter::ConversationUpdate>,
        bool,
    ) {
        match self {
            Self::Update(update, _) => super::output::update_parts(update),
            Self::Flush(source) => (Some(source), None, false),
        }
    }
    pub fn coalesce(&self) -> bool {
        matches!(self, Self::Update(_, true))
    }
    pub fn flush(&self) -> bool {
        matches!(self, Self::Flush(_))
    }
}

/// A buffered replay is distinct from a durable replay and never authorizes ACK.
pub(super) fn replay_state(
    source: &RuntimeOutputSource,
    current: &crate::sandbox::Agent,
    previous: Option<&crate::sandbox::store::AgentMessageCheckpoint>,
    assembly: &AgentMessageAssembly,
) -> Result<(bool, bool), AgentConversationError> {
    let durable = previous
        .filter(|checkpoint| {
            checkpoint.native_conversation_id == source.native_conversation_id
                && Some(checkpoint.runtime_generation) == current.conversation.runtime_generation
        })
        .and_then(|checkpoint| checkpoint.output.as_ref());
    let Some(buffer) = &assembly.quiet_output else {
        return super::output::output_replayed(source, current, previous)
            .map(|replay| (replay, false));
    };
    if Some(buffer.generation) != current.conversation.runtime_generation {
        return Err(AgentConversationError::StaleGeneration);
    }
    if buffer
        .sources
        .iter()
        .any(|position| position.cursor == source.cursor && position != source)
    {
        return Err(AgentConversationError::Corrupt(
            "buffered output receipt hash conflict".into(),
        ));
    }
    if durable.is_some_and(|position| source.cursor <= position.cursor) {
        return super::output::output_replayed(source, current, previous)
            .map(|replay| (replay, false));
    }
    let last = buffer.sources.last().expect("quiet buffers are nonempty");
    let replay = super::output::output_position_replayed(source, current, Some(last))?;
    if replay && !buffer.sources.iter().any(|position| position == source) {
        return Err(AgentConversationError::Corrupt(
            "buffered output receipt hash conflict".into(),
        ));
    }
    Ok((false, replay))
}

pub(super) fn accepted(
    mut recovered: AgentMessageAssembly,
    previous: &AgentMessageAssembly,
    committed: Option<&RuntimeOutputSource>,
    enabled: bool,
) -> AgentMessageAssembly {
    // Replaying an older durable receipt must preserve newer uncommitted receipts.
    if previous.buffered_output().is_some_and(|last| {
        committed.is_some_and(|position| {
            position.stream_id == last.stream_id && position.cursor < last.cursor
        })
    }) {
        recovered.quiet_output = previous.quiet_output.clone();
    }
    recovered.output_committed_at = enabled.then(Instant::now);
    recovered
}
