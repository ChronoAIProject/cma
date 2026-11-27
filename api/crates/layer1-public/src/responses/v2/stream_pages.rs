//! Response observation defers source work until after the HTTP headers.
use super::{error, projection, stream, ResponsesV2State};
use crate::stream_telemetry::{CloseReason, Contract, ContractKind, Kind, Poll};
use axum::response::{sse::Event, Response};
use futures_util::stream::unfold;
use layer2_core::sandbox::{
    agent_io::{AgentResponseReading, CommandReceiptView, ObservedResponseReadError},
    AgentEventStreamAuthorization, AgentEventStreamHeader,
};
use serde_json::{json, Value};
use std::{collections::VecDeque, convert::Infallible, time::Duration};

#[path = "indexed_stream.rs"]
pub(super) mod indexed;

pub(super) async fn open(
    state: ResponsesV2State,
    mut authorization: AgentEventStreamAuthorization,
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
    let observation = match state.reads.observe_output_stream(&mut authorization) {
        Ok(value) => value,
        Err(err) => return error::read(err),
    };
    if let Err(err) = state.reads.validate_event_stream(&authorization).await {
        return error::read(err);
    }
    let command_id = receipt.command_id.to_string();
    let evidence = Contract::new(ContractKind::ResponsesV2);
    let open = Open {
        evidence: evidence.clone(),
        observation,
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
        more: false,
        last: None,
        pagination_required: false,
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
                    if let Err(error) = open
                        .state
                        .reads
                        .validate_event_stream(&open.authorization)
                        .await
                    {
                        open.evidence.record(CloseReason::read(&error));
                        return open.fail("response_observation_unavailable");
                    }
                    open.next = sequence + 1;
                    return Some((Ok(stream::sse(event)), open));
                }
                if open.pagination_required {
                    return open.fail("response_pagination_required");
                }
                if open.terminal || open.failed {
                    if !open.failed {
                        open.evidence.record(CloseReason::NormalTerminal);
                    }
                    return None;
                }
                if !open.first && !open.more {
                    let wake = tokio::time::Instant::now() + Duration::from_millis(250);
                    let wake = open
                        .authorization
                        .revalidation_deadline()
                        .map_or(wake, |expires| expires.min(wake));
                    if let Some(observation) = &mut open.observation {
                        // Every notice checks current command state even if its
                        // durable log coordinate is unchanged.
                        if let Err(error) = observation.changed().await {
                            open.evidence.record(CloseReason::read(&error));
                            return open.fail("response_observation_unavailable");
                        }
                    } else {
                        tokio::time::sleep_until(wake).await;
                    }
                }
                let poll = (!open.first).then(|| Poll::new(Kind::ResponsesV2));
                let first = open.first;
                open.first = false;
                match open.refresh().await {
                    Ok(changed) => {
                        if first
                            && open
                                .after
                                .is_some_and(|after| open.last.is_none_or(|last| after > last))
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
                    Err(code) => return open.fail(code),
                }
            }
        }),
        &response_id,
        &command_id,
        evidence,
    )
}

#[derive(PartialEq, Eq)]
struct Seen {
    command: AgentResponseReading,
    header: AgentEventStreamHeader,
}

struct Open {
    evidence: Contract,
    observation: Option<layer2_core::sandbox::agent_observation::AgentObservationSubscription>,
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
    more: bool,
    last: Option<u64>,
    pagination_required: bool,
}

impl Open {
    async fn refresh(&mut self) -> Result<bool, &'static str> {
        let id = self
            .receipt
            .response_id
            .as_ref()
            .ok_or("response_observation_unavailable")?;
        // Always re-read current command state and authority, including on quiet
        // heads: queued rejection and revocation need not append Agent events.
        let (command, observed_header) = if self.observation.is_some() {
            self.state
                .commands
                .response_for_observed_stream(&self.state.reads, &self.authorization, id)
                .await
                .map(|(command, header)| (command, Some(header)))
                .inspect_err(|error| {
                    let reason = match error {
                        ObservedResponseReadError::Command(error) => CloseReason::command(error),
                        ObservedResponseReadError::Header(error) => CloseReason::read(error),
                    };
                    self.evidence.record(reason);
                })
                .map_err(|_| "response_observation_unavailable")?
        } else {
            self.state
                .commands
                .response_for_stream(&self.authorization, id)
                .await
                .map(|command| (command, None))
                .inspect_err(|error| self.evidence.record(CloseReason::command(error)))
                .map_err(|_| "response_observation_unavailable")?
        };
        if command.command.receipt != self.receipt {
            return Err("response_observation_unavailable");
        }
        let header = match observed_header {
            Some(header) => header,
            None => self
                .state
                .reads
                .event_stream_header(&self.authorization)
                .await
                .inspect_err(|error| self.evidence.record(CloseReason::read(error)))
                .map_err(|_| "response_observation_unavailable")?,
        };
        let current = Seen { command, header };
        if self.seen.as_ref() == Some(&current) {
            return Ok(false);
        }
        if let Some(turn) = current.command.command.turn_sequence {
            let origin = self.state.agent_page_origin.clone();
            let reading = current.command.clone();
            let next = self.next;
            let page = self
                .state
                .reads
                .with_output_observation(&self.authorization, Some(turn), move |source| {
                    Box::pin(async move { indexed::read(source, &origin, reading, next).await })
                })
                .await
                .inspect_err(|error| self.evidence.record(CloseReason::read(error)))
                .map_err(|error| match error {
                    layer2_core::sandbox::AgentReadError::InvalidPage => "invalid_replay_cursor",
                    _ => "response_observation_unavailable",
                })?;
            if let Some(page) = page {
                self.terminal = page.terminal;
                self.more = page.more;
                self.last = Some(page.last);
                self.pagination_required = page.pagination_required;
                self.pending = page.events.into();
                self.seen = (!page.more && page.through == current.header.head).then_some(current);
                return Ok(true);
            }
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
            .inspect_err(|error| self.evidence.record(CloseReason::read(error)))
            .map_err(|_| "response_observation_unavailable")?;
        let response =
            super::shared_projection::project(&self.state, current.command.clone(), &history)
                .ok_or("response_observation_unavailable")?;
        self.terminal = response.terminal();
        self.pending = projection::events(&response, bound).into();
        self.last = self
            .pending
            .back()
            .and_then(|v| v["sequence_number"].as_u64());
        self.more = false;
        if self.observation.is_some() {
            let all = std::mem::take(&mut self.pending);
            let mut bytes = 0;
            for event in all.into_iter().filter(|event| {
                event["sequence_number"]
                    .as_u64()
                    .is_some_and(|seq| seq >= self.next)
            }) {
                let size = serde_json::to_vec(&event)
                    .map_err(|_| "response_observation_unavailable")?
                    .len();
                if size > layer2_core::sandbox::agent_observation::MAX_PAGE_BYTES {
                    self.pagination_required = true;
                    self.terminal = false;
                    break;
                }
                if self.pending.len() == 200
                    || bytes + size > layer2_core::sandbox::agent_observation::MAX_PAGE_BYTES
                {
                    self.more = true;
                    self.terminal = false;
                    break;
                }
                bytes += size;
                self.pending.push_back(event);
            }
        }
        // An append between the cheap head read and the source read forces one
        // fresh observation; it cannot cache a mismatched source coordinate.
        self.seen = (!self.more && history.head == current.header.head).then_some(current);
        Ok(true)
    }

    fn fail(mut self, code: &'static str) -> Option<(Result<Event, Infallible>, Self)> {
        self.evidence
            .record(if code == "response_pagination_required" {
                CloseReason::PaginationRequired
            } else {
                CloseReason::ServerError
            });
        self.failed = true;
        self.pending.clear();
        let event = json!({"type":"error","sequence_number":self.next,"code":code,
            "message":if code=="response_pagination_required" {"The response exceeds the result budget. Read its /output pages; output was not truncated."} else {"Response observation ended. Retrieve or reconnect with current authorization."},"param":null});
        // No durable id and no cursor advancement on dependency failures.
        Some((Ok(stream::sse(event)), self))
    }
}
