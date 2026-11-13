//! One bounded private operation followed by one release fence.
use super::*;
use crate::diagnostic_context::DiagnosticContext;
use crate::sandbox::agent_io::{AgentCommandService, AgentEventKind};

/// The transport chooses coordinates only after seeing the current header.
/// Opaque cursor validation stays outside core without reopening authority.
pub enum AgentBufferedEventRequest {
    Page {
        after: u64,
        through: u64,
        limit: u16,
        message_bindings: bool,
    },
    Head,
}

impl AgentReadService {
    /// No intermediate value leaves this operation. Selector errors, private
    /// dependency failures and timeouts all pass the same final release fence.
    pub async fn buffered_event_window<E: From<AgentReadError> + Send>(
        &self,
        authorization: &AgentEventStreamAuthorization,
        commands: &AgentCommandService,
        select: impl FnOnce(&AgentEventStreamHeader) -> Result<AgentBufferedEventRequest, E> + Send,
    ) -> Result<AgentReplayWindow, E> {
        let context = DiagnosticContext::current();
        let result = authorization.bounded(async {
            let snapshot = context.clone().with_phase("history_capture")
                .scope(self.current_event_stream_snapshot(authorization)).await?;
            let agent = snapshot.lineage.agent
                .ok_or_else(|| corrupt("authorized event snapshot has no Agent"))?;
            let header = window::header(&agent)?;
            let request = match select(&header) {
                Ok(request) => request,
                Err(error) => return Ok(Err(error)),
            };
            let (request, messages, active_turn) = match request {
                AgentBufferedEventRequest::Head => {
                    let Some(turn) = header.active_turn else {
                        return Ok(Ok(AgentReplayWindow {
                            through: header.head, header, entries: Vec::new(),
                        }));
                    };
                    if turn == 0 || turn > header.head {
                        return Err(corrupt("active turn is outside the captured history"));
                    }
                    (window::WindowRead {
                        after: turn - 1, through: Some(turn), limit: 1,
                        suffix: false, shared_tail_proof: true,
                    }, false, Some(turn))
                }
                AgentBufferedEventRequest::Page { after, through, limit, message_bindings } => {
                    if !(1..=200).contains(&limit) {
                        return Err(AgentReadError::InvalidPage);
                    }
                    (window::WindowRead {
                        after, through: Some(through), limit,
                        suffix: false, shared_tail_proof: true,
                    }, message_bindings, None)
                }
            };
            let window = context.clone().with_phase("history_page")
                .scope(self.project_event_window(authorization, &agent, request)).await?;
            if let Some(turn) = active_turn {
                if window.entries.len() != 1
                    || window.entries[0].sequence != turn
                    || !matches!(window.entries[0].kind, Some(AgentEventKind::TurnStarted { turn_sequence }) if turn_sequence == turn)
                {
                    return Err(corrupt("active turn does not identify a turn start"));
                }
            }
            let window = context.clone().with_phase("history_bindings").scope(async {
                if window.entries.is_empty() { return Ok(window); }
                let window = commands.bind_replay_window_unfenced(authorization, window).await?;
                if messages {
                    commands.bind_message_window_unfenced(self, authorization, window).await
                } else {
                    Ok(window)
                }
            }).await?;
            Ok(Ok(window))
        }).await;
        // Keep this outside the inner result: neither ? nor timeout can bypass
        // the nonrenewing authority check or reveal a private dependency error.
        context
            .with_phase("final_fence")
            .scope(self.validate_event_stream(authorization))
            .await
            .map_err(E::from)?;
        result.map_err(E::from)?
    }
}
