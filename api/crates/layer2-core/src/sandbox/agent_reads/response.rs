//! Response-specific reads retain the stream's nonrenewing release authority.
use super::*;
use crate::sandbox::{store::AgentResponseLogWindow, AgentLogKind};

#[path = "response_indexed.rs"]
pub mod indexed;

/// A supported indexed result never falls back, even when its projection
/// requires pagination. Legacy snapshots retain complete source validation.
pub enum AgentResponseObservation<T> {
    Indexed(T),
    Legacy(AgentEventStreamSnapshot),
}

pub enum AgentResponseObservationError {
    Authorization(AgentReadError),
    Source(AgentReadError),
    Release(AgentReadError),
    SourceAndRelease {
        source: AgentReadError,
        release: AgentReadError,
    },
}

impl AgentResponseObservationError {
    fn public_error(self) -> AgentReadError {
        match self {
            Self::Authorization(error) | Self::Source(error) | Self::Release(error) => error,
            Self::SourceAndRelease { release, .. } => release,
        }
    }

    fn fence<T>(
        source: Result<T, AgentReadError>,
        release: Result<(), AgentReadError>,
    ) -> Result<T, Self> {
        match (source, release) {
            (source, Ok(())) => source.map_err(Self::Source),
            (Ok(_), Err(error)) => Err(Self::Release(error)),
            (Err(source), Err(release)) => Err(Self::SourceAndRelease { source, release }),
        }
    }
}

impl AgentReadService {
    pub fn with_response_logs(
        self: &Arc<Self>,
        response_logs: Option<Arc<dyn super::super::store::AgentResponseLogStore>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            observation: self.observation.clone(),
            store: self.store.clone(),
            sessions: self.sessions.clone(),
            resources: self.resources.clone(),
            access: self.access.clone(),
            response_logs,
        })
    }

    /// The command service supplies its already-authorized immutable turn
    /// binding. A bounded window is a complete input for the existing response
    /// projection, never a truncated substitute for complete output.
    pub async fn response_event_stream_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
        turn_sequence: Option<u64>,
    ) -> Result<AgentEventStreamSnapshot, AgentReadError> {
        // An unbound response has no output turn. Its public projection only
        // needs the current Agent identity/model and the command receipt;
        // scanning the response source here adds Mongo work without adding
        // evidence. Keep the same current snapshot and release fence used by
        // every other response observation.
        if turn_sequence.is_none() {
            return self
                .read_unbound_response_snapshot(authorization)
                .await
                .map_err(AgentResponseObservationError::public_error);
        }
        if self.response_logs.is_none() {
            return self.event_stream_snapshot(authorization).await;
        }
        let result = authorization
            .bounded(async {
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let agent = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized response has no Agent"))?;
                self.read_response_window(&agent, turn_sequence, authorization.resource().2)
                    .await
            })
            .await;
        // Also fence dependency errors and fallback decisions before releasing
        // anything; a failed private read cannot extend expired authority.
        self.validate_event_stream(authorization).await?;
        match result? {
            Some(snapshot) => Ok(snapshot),
            None => self.event_stream_snapshot(authorization).await,
        }
    }

    pub async fn read_unbound_response_snapshot(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<AgentEventStreamSnapshot, AgentResponseObservationError> {
        authorization
            .bounded(async {
                let snapshot = self.current_event_stream_snapshot(authorization).await?;
                let result = snapshot
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized response has no Agent"))
                    .map(|agent| AgentEventStreamSnapshot {
                        source_generation: None,
                        model: agent.selected_model(),
                        id: agent.id,
                        created_at: agent.created_at,
                        initial_model: agent.profile.content.model,
                        runtime_generation: agent.conversation.runtime_generation,
                        active_turn: agent.conversation.active_turn,
                        head: agent.conversation.last_seq,
                        history: vec![],
                    });
                let release = self.validate_event_stream(authorization).await;
                Ok(AgentResponseObservationError::fence(result, release))
            })
            .await
            .map_err(AgentResponseObservationError::Authorization)?
    }

    /// Select one command-bound source under a single nonrenewing IO budget.
    /// The final authority check also gates private source/projection errors.
    pub async fn with_response_observation<T: Send, F>(
        &self,
        authorization: &AgentEventStreamAuthorization,
        turn: u64,
        operation: F,
    ) -> Result<AgentResponseObservation<T>, AgentReadError>
    where
        F: Send
            + for<'a, 'b> FnOnce(
                &'a mut indexed::AgentOutputObservation<'b>,
            ) -> indexed::OutputReadFuture<'a, T>,
    {
        self.read_response_observation(authorization, turn, None, operation)
            .await
            .map_err(AgentResponseObservationError::public_error)
    }

    pub async fn read_response_observation<T: Send, F>(
        &self,
        authorization: &AgentEventStreamAuthorization,
        turn: u64,
        operation_key: Option<[u8; 32]>,
        operation: F,
    ) -> Result<AgentResponseObservation<T>, AgentResponseObservationError>
    where
        F: Send
            + for<'a, 'b> FnOnce(
                &'a mut indexed::AgentOutputObservation<'b>,
            ) -> indexed::OutputReadFuture<'a, T>,
    {
        authorization
            .bounded(async {
                let current = self.current_event_stream_snapshot(authorization).await?;
                let agent = current
                    .lineage
                    .agent
                    .ok_or_else(|| corrupt("authorized response has no Agent"))?;
                let result = async {
                    let personal = authorization.resource().2;
                    if let Some(result) = self
                        .read_output_observation(
                            &agent,
                            Some(turn),
                            personal,
                            operation_key,
                            operation,
                        )
                        .await?
                    {
                        return Ok(AgentResponseObservation::Indexed(result));
                    }
                    let snapshot = match self
                        .read_response_window(&agent, Some(turn), personal)
                        .await?
                    {
                        Some(snapshot) => snapshot,
                        None => self.complete_event_stream_snapshot(agent, personal).await?,
                    };
                    Ok(AgentResponseObservation::Legacy(snapshot))
                }
                .await;
                let release = self
                    .current_event_stream_snapshot(authorization)
                    .await
                    .map(|_| ());
                Ok(AgentResponseObservationError::fence(result, release))
            })
            .await
            .map_err(AgentResponseObservationError::Authorization)?
    }

    async fn read_response_window(
        &self,
        agent: &Agent,
        turn_sequence: Option<u64>,
        personal: bool,
    ) -> Result<Option<AgentEventStreamSnapshot>, AgentReadError> {
        let Some(reader) = &self.response_logs else {
            return Ok(None);
        };
        let flight = match &self.observation {
            Some(service) => service
                .output_flight(agent, turn_sequence)
                .await
                .map_err(log_store_error)?,
            None => None,
        };
        if let Some(window) = flight
            .as_ref()
            .and_then(|flight| flight.get::<AgentResponseLogWindow>(b"window"))
        {
            let mut snapshot = project_window(agent, turn_sequence, (*window).clone(), personal)?;
            snapshot.source_generation = flight.as_ref().map(|flight| flight.identity());
            return Ok(Some(snapshot));
        }
        let previous = flight.as_ref().and_then(|flight| flight.previous_window());
        let suffix = match previous.as_ref() {
            Some(previous) if previous.through < agent.conversation.last_seq => reader
                .agent_response_log_suffix(agent, previous)
                .await
                .map_err(capture_open_error)?,
            _ => None,
        };
        let window = match suffix {
            Some(window) => Some(window),
            None => reader
                .agent_response_log_window(agent, turn_sequence)
                .await
                .map_err(capture_open_error)?,
        };
        window
            .map(|window| {
                let mut snapshot = project_window(agent, turn_sequence, window.clone(), personal)?;
                snapshot.source_generation = flight.as_ref().map(|flight| flight.identity());
                if let Some(flight) = &flight {
                    flight.remember_window(window.clone());
                    flight.insert(b"window", window);
                }
                Ok(snapshot)
            })
            .transpose()
    }
}

fn project_window(
    agent: &Agent,
    turn: Option<u64>,
    window: AgentResponseLogWindow,
    personal: bool,
) -> Result<AgentEventStreamSnapshot, AgentReadError> {
    if window.through != agent.conversation.last_seq || window.turn_sequence != turn {
        return Err(corrupt("response source does not match its captured head"));
    }
    match turn {
        None if !window.entries.is_empty() => {
            return Err(corrupt("unbound response unexpectedly contains a turn"));
        }
        Some(turn) => {
            if window.entries.len() > 200
                || !window.entries.first().is_some_and(|entry| {
                    entry.seq == turn
                        && matches!(&entry.kind, AgentLogKind::TurnStarted {
                        turn_sequence, model: Some(_), ..
                    } if *turn_sequence == turn)
                })
            {
                return Err(corrupt("response turn lacks its retained model fence"));
            }
            for (offset, entry) in window.entries.iter().enumerate() {
                if entry.agent_id != agent.id
                    || turn.checked_add(offset as u64) != Some(entry.seq)
                    || entry.seq > window.through
                {
                    return Err(corrupt("response source interval is not consecutive"));
                }
            }
            let last = window.entries.last().expect("turn fence is present");
            if last.seq != window.through
                && !(last.seq > turn
                    && matches!(last.kind,
                    AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == last.seq))
            {
                return Err(corrupt("response window has no complete turn boundary"));
            }
        }
        None => {}
    }
    Ok(AgentEventStreamSnapshot {
        source_generation: None,
        id: agent.id.clone(),
        created_at: agent.created_at.clone(),
        model: agent.selected_model(),
        initial_model: agent.profile.content.model.clone(),
        runtime_generation: agent.conversation.runtime_generation,
        active_turn: agent.conversation.active_turn,
        head: window.through,
        history: window
            .entries
            .into_iter()
            .map(|entry| AgentVisibleLogEntry::from_entry(entry, personal))
            .collect(),
    })
}
