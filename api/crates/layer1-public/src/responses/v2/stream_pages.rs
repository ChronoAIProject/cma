//! Response observation defers source work until after the HTTP headers.
use super::{error, projection, stream, ResponsesV2State};
use crate::stream_telemetry::{Kind, Poll};
use axum::response::{sse::Event, Response};
use futures_util::stream::unfold;
use layer2_core::sandbox::{
    agent_io::{AgentResponseReading, CommandReceiptView},
    AgentEventStreamAuthorization, AgentEventStreamHeader,
};
use serde_json::{json, Value};
use std::{collections::VecDeque, convert::Infallible, time::Duration};

pub(super) async fn open(
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
) -> Response {
    let Some(response_id) = receipt.response_id.as_ref().map(ToString::to_string) else {
        return error::unavailable();
    };
    let Some(next) = after.map_or(Some(0), |value| value.checked_add(1)) else {
        return error::field(
            "starting_after",
            "invalid_replay_cursor",
            "The replay cursor is out of range.",
        );
    };
    if let Err(err) = state.reads.validate_event_stream(&authorization).await {
        return error::read(err);
    }
    let command_id = receipt.command_id.to_string();
    let open = Open {
        state,
        authorization,
        receipt,
        next,
        after,
        seen: None,
        pending: VecDeque::new(),
        terminal: false,
        failed: false,
        first: true,
    };
    stream::response(
        unfold(open, |mut open| async move {
            loop {
                if let Some(event) = open.pending.pop_front() {
                    let sequence = event["sequence_number"]
                        .as_u64()
                        .expect("generated sequence");
                    if sequence < open.next {
                        continue;
                    }
                    // Backpressure cannot extend or renew a captured grant.
                    if open
                        .state
                        .reads
                        .validate_event_stream(&open.authorization)
                        .await
                        .is_err()
                    {
                        return open.fail("response_observation_unavailable");
                    }
                    open.next = sequence + 1;
                    return Some((Ok(stream::sse(event)), open));
                }
                if open.terminal || open.failed {
                    return None;
                }
                if !open.first {
                    let wake = tokio::time::Instant::now() + Duration::from_millis(250);
                    let wake = open
                        .authorization
                        .revalidation_deadline()
                        .map_or(wake, |expires| expires.min(wake));
                    tokio::time::sleep_until(wake).await;
                }
                let poll = (!open.first).then(|| Poll::new(Kind::ResponsesV2));
                let first = open.first;
                open.first = false;
                match open.refresh().await {
                    Ok(changed) => {
                        if first
                            && open.after.is_some_and(|after| {
                                open.pending.back().is_none_or(|last| {
                                    last["sequence_number"]
                                        .as_u64()
                                        .is_none_or(|last| after > last)
                                })
                            })
                        {
                            return open.fail("invalid_replay_cursor");
                        }
                        if let Some(poll) = poll {
                            poll.complete(
                                changed
                                    && open.pending.back().is_some_and(|last| {
                                        last["sequence_number"]
                                            .as_u64()
                                            .is_some_and(|last| last >= open.next)
                                    }),
                            );
                        }
                    }
                    Err(()) => return open.fail("response_observation_unavailable"),
                }
            }
        }),
        &response_id,
        &command_id,
    )
}

#[derive(PartialEq, Eq)]
struct Seen {
    command: AgentResponseReading,
    header: AgentEventStreamHeader,
}

struct Open {
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    next: u64,
    after: Option<u64>,
    seen: Option<Seen>,
    pending: VecDeque<Value>,
    terminal: bool,
    failed: bool,
    first: bool,
}

impl Open {
    async fn refresh(&mut self) -> Result<bool, ()> {
        let id = self.receipt.response_id.as_ref().ok_or(())?;
        // Always re-read current command state and authority, including on quiet
        // heads: queued rejection and revocation need not append Agent events.
        let command = self
            .state
            .commands
            .response_for_stream(&self.authorization, id)
            .await
            .map_err(|_| ())?;
        if command.command.receipt != self.receipt {
            return Err(());
        }
        let header = self
            .state
            .reads
            .event_stream_header(&self.authorization)
            .await
            .map_err(|_| ())?;
        let current = Seen { command, header };
        if self.seen.as_ref() == Some(&current) {
            return Ok(false);
        }
        let bound = current.command.command.turn_sequence.is_some();
        let history = self
            .state
            .reads
            .response_event_stream_snapshot(
                &self.authorization,
                current.command.command.turn_sequence,
            )
            .await
            .map_err(|_| ())?;
        let response = projection::project(
            &self.state.agent_page_origin,
            current.command.clone(),
            &history,
        )
        .ok_or(())?;
        self.terminal = response.terminal();
        self.pending = projection::events(&response, bound).into();
        // An append between the cheap head read and the source read forces one
        // fresh observation; it cannot cache a mismatched source coordinate.
        self.seen = (history.head == current.header.head).then_some(current);
        Ok(true)
    }

    fn fail(mut self, code: &'static str) -> Option<(Result<Event, Infallible>, Self)> {
        self.failed = true;
        self.pending.clear();
        let event = json!({"type":"error","sequence_number":self.next,"code":code,
            "message":"Response observation ended. Retrieve or reconnect with current authorization.","param":null});
        // No durable id and no cursor advancement on dependency failures.
        Some((Ok(stream::sse(event)), self))
    }
}
