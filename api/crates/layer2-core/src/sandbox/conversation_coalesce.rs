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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutputState {
    buffer: Option<QuietOutputBuffer>,
    committed_at: Option<Instant>,
}

impl AgentMessageAssembly {
    pub fn output_flush_deadline(&self) -> Option<Instant> {
        self.output_state
            .as_ref()?
            .buffer
            .as_ref()
            .map(|buffer| buffer.deadline)
    }

    pub fn buffered_output(&self) -> Option<&RuntimeOutputSource> {
        self.output_state.as_ref()?.buffer.as_ref()?.sources.last()
    }

    pub(super) fn retain_quiet(
        &mut self,
        previous: &Self,
        source: &RuntimeOutputSource,
        generation: u64,
        policy: AgentOutputCoalescePolicy,
    ) -> bool {
        let now = Instant::now();
        let committed_at = previous
            .output_state
            .as_ref()
            .and_then(|state| state.committed_at);
        let mut buffer = previous
            .output_state
            .as_ref()
            .and_then(|state| state.buffer.clone())
            .unwrap_or_else(|| QuietOutputBuffer {
                sources: Vec::new(),
                generation,
                // An idle gap does not reset the time since the last durable cursor.
                deadline: committed_at.unwrap_or(now) + policy.max_age,
            });
        buffer.sources.push(source.clone());
        if buffer.sources.len() >= policy.max_frames || now >= buffer.deadline {
            return false;
        }
        self.output_state = Some(Box::new(OutputState {
            buffer: Some(buffer),
            committed_at,
        }));
        true
    }

    pub(super) fn durable_copy(&self) -> Self {
        let mut durable = self.clone();
        durable.output_state = None;
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
    reopen: bool,
) -> Result<(bool, bool), AgentConversationError> {
    let durable = previous
        .filter(|checkpoint| {
            checkpoint.native_conversation_id == source.native_conversation_id
                && Some(checkpoint.runtime_generation) == current.conversation.runtime_generation
        })
        .and_then(|checkpoint| checkpoint.output.as_ref());
    let Some(buffer) = assembly
        .output_state
        .as_ref()
        .and_then(|state| state.buffer.as_ref())
    else {
        return super::output::output_replayed(source, current, previous, reopen)
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
        return super::output::output_replayed(source, current, previous, false)
            .map(|replay| (replay, false));
    }
    let last = buffer.sources.last().expect("quiet buffers are nonempty");
    let replay = super::output::output_position_replayed(source, current, Some(last), || false)?;
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
        recovered.output_state = previous.output_state.clone();
    } else {
        recovered.output_state = enabled.then(|| {
            Box::new(OutputState {
                buffer: None,
                committed_at: Some(Instant::now()),
            })
        });
    }
    recovered
}
