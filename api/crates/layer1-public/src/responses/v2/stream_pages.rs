//! Response observation defers source work until after the HTTP headers.
use super::{
    error, projection,
    read_cycle::{indexed_read_key, read_category, read_cycle, FirstFailure, ReadFailure},
    stream, ResponsesV2State,
};
use crate::stream_telemetry::{CloseReason, Contract, ContractKind, Kind, Poll};
use axum::response::{sse::Event, Response};
use futures_util::stream::unfold;
use layer2_core::sandbox::{
    agent_io::{AgentResponseReading, CommandReceiptView},
    agent_observation::{durable_read_scope, AgentObservationSubscription, OBSERVATION_INTERVAL},
    AgentEventStreamAuthorization, AgentEventStreamHeader, AgentReadError,
    AgentResponseObservation,
};
use serde_json::{json, Value};
use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[path = "indexed_stream.rs"]
pub(super) mod indexed;

pub(super) async fn open(
    state: ResponsesV2State,
    mut authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
    post: bool,
) -> Response {
    let mut first_failure = FirstFailure::new(&receipt, post);
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
    let mut observation = match state.reads.observe_output_stream(&mut authorization) {
        Ok(value) => value,
        Err(err) => {
            first_failure.record(
                "paged",
                next,
                ("subscription_open", read_category(&err)),
                false,
                "none",
            );
            return error::read(err);
        }
    };
    let mut durable_recovery = None;
    if let Err(err) = state.reads.validate_event_stream(&authorization).await {
        durable_recovery = acquire_durable_recovery(
            &state.durable_recovery_slots,
            &mut authorization,
            observation.as_ref(),
            &err,
        );
        first_failure.record(
            "paged",
            next,
            ("header_read", read_category(&err)),
            observation.is_some(),
            if durable_recovery.is_some() {
                "durable"
            } else {
                "none"
            },
        );
        if durable_recovery.is_none() {
            return error::read(err);
        }
        observation = None;
        if let Err(err) = state.reads.validate_event_stream(&authorization).await {
            return error::read(err);
        }
    }
    let command_id = receipt.command_id.to_string();
    let evidence = Contract::new(ContractKind::ResponsesV2);
    let open = Open {
        evidence: evidence.clone(),
        first_failure,
        observation,
        durable_recovery,
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
                if open.failed {
                    return None;
                }
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
                        if open.recover_after("frame_authorization", &error) {
                            continue;
                        }
                        open.evidence.record(CloseReason::read(&error));
                        return open.fail("response_observation_unavailable");
                    }
                    open.next = sequence + 1;
                    return Some((Ok(stream::sse(event)), open));
                }
                if open.pagination_required {
                    return open.fail("response_pagination_required");
                }
                if open.terminal {
                    open.evidence.record(CloseReason::NormalTerminal);
                    return None;
                }
                if !open.first && !open.more {
                    #[cfg(any(test, feature = "test-support"))]
                    open.state.wait_for_replay_refresh(open.next).await;
                    let interval = if open.durable_recovery.is_some() {
                        OBSERVATION_INTERVAL
                    } else {
                        Duration::from_millis(250)
                    };
                    let wake = tokio::time::Instant::now() + interval;
                    let wake = open
                        .authorization
                        .revalidation_deadline()
                        .map_or(wake, |expires| expires.min(wake));
                    if let Some(observation) = &mut open.observation {
                        // Every notice checks current command state even if its
                        // durable log coordinate is unchanged.
                        if let Err(error) = observation.changed().await {
                            if open.recover_after("subscription_notice", &error) {
                                continue;
                            }
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
    first_failure: FirstFailure,
    observation: Option<AgentObservationSubscription>,
    durable_recovery: Option<OwnedSemaphorePermit>,
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
    fn resume_from_durable_history(&mut self, error: &AgentReadError) -> bool {
        let Some(permit) = acquire_durable_recovery(
            &self.state.durable_recovery_slots,
            &mut self.authorization,
            self.observation.as_ref(),
            error,
        ) else {
            return false;
        };
        self.observation = None;
        self.durable_recovery = Some(permit);
        // Rebuild unreleased frames from current durable history. The next
        // sequence is the first frame this connection has not yet released.
        self.pending.clear();
        self.seen = None;
        self.terminal = false;
        self.more = false;
        self.pagination_required = false;
        true
    }

    fn recover_after(&mut self, stage: &'static str, error: &AgentReadError) -> bool {
        let custody = self.observation.is_some();
        let recovery = self.resume_from_durable_history(error);
        self.first_failure.record(
            "paged",
            self.next,
            (stage, read_category(error)),
            custody,
            if recovery { "durable" } else { "none" },
        );
        recovery
    }

    async fn refresh(&mut self) -> Result<bool, &'static str> {
        let deadline = self.authorization.read_deadline();
        let grant_deadline = self.authorization.revalidation_deadline();
        let page = read_cycle(
            self,
            deadline,
            grant_deadline,
            |open| {
                Box::pin(async {
                    if open.durable_recovery.is_some() {
                        durable_read_scope(open.refresh_once()).await
                    } else {
                        open.refresh_once().await
                    }
                })
            },
            |open, failure, available| {
                let custody = open.observation.is_some();
                let retry = available
                    && match failure {
                        ReadFailure::Header(error) | ReadFailure::Release(error) => {
                            open.resume_from_durable_history(error)
                        }
                        _ => failure.advanced(),
                    };
                open.first_failure.record(
                    "paged",
                    open.next,
                    failure.diagnostic(),
                    custody,
                    match (retry, failure.advanced()) {
                        (true, true) => "recapture",
                        (true, false) => "durable",
                        _ => "none",
                    },
                );
                retry
            },
        )
        .await
        .map_err(|failure| {
            self.evidence.record(failure.close_reason());
            if matches!(
                failure,
                ReadFailure::Header(AgentReadError::InvalidPage)
                    | ReadFailure::Source(AgentReadError::InvalidPage)
            ) {
                "invalid_replay_cursor"
            } else {
                "response_observation_unavailable"
            }
        })?;
        let Some(page) = page else { return Ok(false) };
        self.pending = page.pending;
        self.terminal = page.terminal;
        self.more = page.more;
        self.last = page.last;
        self.pagination_required = page.pagination_required;
        self.seen = page.seen;
        Ok(true)
    }

    async fn refresh_once(&self) -> Result<Option<PreparedRefresh>, ReadFailure> {
        let id = self
            .receipt
            .response_id
            .as_ref()
            .ok_or(ReadFailure::Receipt)?;
        // Always re-read current command state and authority, including on quiet
        // heads: queued rejection and revocation need not append Agent events.
        let (command, header) = if self.observation.is_some() {
            self.state
                .commands
                .response_for_observed_stream(&self.state.reads, &self.authorization, id)
                .await
                .map_err(ReadFailure::from)?
        } else {
            let command = self
                .state
                .commands
                .response_for_stream(&self.authorization, id)
                .await
                .map_err(ReadFailure::Command)?;
            let header = self
                .state
                .reads
                .event_stream_header(&self.authorization)
                .await
                .map_err(ReadFailure::Header)?;
            (command, header)
        };
        if command.command.receipt != self.receipt {
            return Err(ReadFailure::Receipt);
        }
        let current = Seen { command, header };
        if self.seen.as_ref() == Some(&current) {
            return Ok(None);
        }
        let history = if let Some(turn) = current.command.command.turn_sequence {
            let origin = self.state.agent_page_origin.clone();
            let reading = current.command.clone();
            let next = self.next;
            let key =
                indexed_read_key(&reading, Some(next), self.authorization.is_personal_owner())?;
            let observation = self
                .state
                .reads
                .read_response_observation(&self.authorization, turn, Some(key), move |source| {
                    Box::pin(async move { indexed::read(source, &origin, reading, next).await })
                })
                .await
                .map_err(ReadFailure::from)?;
            match observation {
                AgentResponseObservation::Indexed(page) => {
                    return Ok(Some(PreparedRefresh {
                        terminal: page.terminal,
                        more: page.more,
                        last: Some(page.last),
                        pagination_required: page.pagination_required,
                        pending: page.events.into(),
                        seen: (!page.more && page.through == current.header.head)
                            .then_some(current),
                    }));
                }
                AgentResponseObservation::Legacy(history) => history,
            }
        } else {
            self.state
                .reads
                .read_unbound_response_snapshot(&self.authorization)
                .await
                .map_err(ReadFailure::from)?
        };
        let bound = current.command.command.turn_sequence.is_some();
        let response =
            super::shared_projection::project(&self.state, current.command.clone(), &history)
                .ok_or(ReadFailure::Projection)?;
        let events = projection::events(&response, bound);
        let last = events.last().and_then(|v| v["sequence_number"].as_u64());
        let mut prepared = PreparedRefresh {
            pending: VecDeque::new(),
            terminal: response.terminal(),
            more: false,
            last,
            pagination_required: false,
            seen: None,
        };
        if self.observation.is_some() || self.durable_recovery.is_some() {
            let mut bytes = 0;
            for event in events.into_iter().filter(|event| {
                event["sequence_number"]
                    .as_u64()
                    .is_some_and(|seq| seq >= self.next)
            }) {
                let size = serde_json::to_vec(&event)
                    .map_err(|_| ReadFailure::Projection)?
                    .len();
                if size > layer2_core::sandbox::agent_observation::MAX_PAGE_BYTES {
                    prepared.pagination_required = true;
                    prepared.terminal = false;
                    break;
                }
                if prepared.pending.len() == 200
                    || bytes + size > layer2_core::sandbox::agent_observation::MAX_PAGE_BYTES
                {
                    prepared.more = true;
                    prepared.terminal = false;
                    break;
                }
                bytes += size;
                prepared.pending.push_back(event);
            }
        } else {
            prepared.pending = events.into();
        }
        self.state
            .reads
            .validate_event_stream(&self.authorization)
            .await
            .map_err(ReadFailure::Release)?;
        prepared.seen = (!prepared.more && history.head == current.header.head).then_some(current);
        Ok(Some(prepared))
    }

    fn fail(mut self, code: &'static str) -> Option<(Result<Event, Infallible>, Self)> {
        self.evidence
            .record(if code == "response_pagination_required" {
                CloseReason::PaginationRequired
            } else {
                CloseReason::ServerError
            });
        self.failed = true;
        self.observation = None;
        self.durable_recovery = None;
        self.pending.clear();
        let event = json!({"type":"error","sequence_number":self.next,"code":code,
            "message":if code=="response_pagination_required" {"The response exceeds the result budget. Read its /output pages; output was not truncated."} else {"Response observation ended. Retrieve or reconnect with current authorization."},"param":null});
        // No durable id and no cursor advancement on dependency failures.
        Some((Ok(stream::sse(event)), self))
    }
}

struct PreparedRefresh {
    pending: VecDeque<Value>,
    terminal: bool,
    more: bool,
    last: Option<u64>,
    pagination_required: bool,
    seen: Option<Seen>,
}

fn acquire_durable_recovery(
    slots: &Arc<Semaphore>,
    authorization: &mut AgentEventStreamAuthorization,
    observation: Option<&AgentObservationSubscription>,
    error: &AgentReadError,
) -> Option<OwnedSemaphorePermit> {
    if !matches!(error, AgentReadError::Unavailable(_))
        || !observation.is_some_and(AgentObservationSubscription::custody_lost)
    {
        return None;
    }
    let permit = slots.clone().try_acquire_owned().ok()?;
    authorization.use_personal_durable_reads().then_some(permit)
}
