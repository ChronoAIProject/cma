use super::{error, projection, ResponsesV2State, V2ResponseObject};
use crate::stream_telemetry::{self, CloseReason, Contract, ContractKind, Kind, Poll};
use axum::response::{
    sse::{Event, KeepAlive},
    IntoResponse, Response, Sse,
};
use futures_util::stream;
use layer2_core::sandbox::{
    agent_io::CommandReceiptView, AgentEventStreamAuthorization, AgentResponseObservation,
};
use serde_json::{json, Value};
use std::{collections::VecDeque, convert::Infallible, time::Duration};

pub(super) async fn snapshot(
    state: &ResponsesV2State,
    authorization: &AgentEventStreamAuthorization,
    receipt: &CommandReceiptView,
) -> Result<(V2ResponseObject, bool), Response> {
    let mut authorization = authorization.clone();
    let _observation = state
        .reads
        .observe_output_stream(&mut authorization)
        .map_err(error::read)?;
    snapshot_mode(state, &authorization, receipt, true, None).await
}

async fn snapshot_mode(
    state: &ResponsesV2State,
    authorization: &AgentEventStreamAuthorization,
    receipt: &CommandReceiptView,
    indexed_allowed: bool,
    evidence: Option<&Contract>,
) -> Result<(V2ResponseObject, bool), Response> {
    let id = receipt
        .response_id
        .as_ref()
        .ok_or_else(error::unavailable)?;
    let reading = state
        .commands
        .response_for_stream(authorization, id)
        .await
        .inspect_err(|err| {
            if let Some(evidence) = evidence {
                evidence.record(CloseReason::command(err));
            }
        })
        .map_err(error::admission)?;
    if reading.command.receipt != *receipt {
        return Err(error::unavailable());
    }
    let bound = reading.command.turn_sequence.is_some();
    if !bound {
        let history = state
            .reads
            .response_event_stream_snapshot(authorization, None)
            .await
            .inspect_err(|err| {
                if let Some(evidence) = evidence {
                    evidence.record(CloseReason::read(err));
                }
            })
            .map_err(error::read)?;
        return super::shared_projection::project(state, reading, &history)
            .map(|response| (response, false))
            .ok_or_else(error::unavailable);
    }
    let origin = state.agent_page_origin.clone();
    let indexed_reading = reading.clone();
    let observation = state
        .reads
        .with_response_observation(
            authorization,
            reading
                .command
                .turn_sequence
                .expect("bound response has a turn"),
            move |source| {
                Box::pin(async move {
                    if !indexed_allowed {
                        return Err(layer2_core::sandbox::AgentReadError::Unavailable(
                            "source format changed; reconnect at the same durable cursor".into(),
                        ));
                    }
                    projection::indexed::complete(source, &origin, indexed_reading).await
                })
            },
        )
        .await
        .inspect_err(|err| {
            if let Some(evidence) = evidence {
                evidence.record(CloseReason::read(err));
            }
        })
        .map_err(error::read)?;
    let history = match observation {
        AgentResponseObservation::Indexed(result) => {
            return result.map(|response| (response, bound)).ok_or_else(|| {
                if let Some(evidence) = evidence {
                    evidence.record(CloseReason::PaginationRequired);
                }
                pagination_required()
            });
        }
        AgentResponseObservation::Legacy(history) => history,
    };
    super::shared_projection::project(state, reading, &history)
        .map(|response| (response, bound))
        .ok_or_else(error::unavailable)
}

pub(super) async fn open(
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
    known_bound: Option<bool>,
) -> Response {
    // Queued responses are not bound to a runtime turn and cannot contain
    // output. Avoid probing the output format and scanning the Agent log just
    // to construct their response.created frame; snapshot_mode still reads
    // the current Agent and applies the final authority fence.
    if known_bound == Some(false)
        || (state.reads.shared_observation_enabled()
            && layer2_core::sandbox::agent_observation::observation_agent().is_some())
    {
        // The response may bind after admission. Start with the metadata-only
        // header path, then let stream_pages select indexed or legacy output
        // from the first bound observation instead of freezing legacy routing.
        return super::stream_pages::open(state, authorization, receipt, after).await;
    }
    let indexed = match state.reads.output_is_indexed(&authorization).await {
        Ok(indexed) => indexed,
        Err(value) => return error::read(value),
    };
    if state.paged_events || indexed {
        return super::stream_pages::open(state, authorization, receipt, after).await;
    }
    let (initial, bound) = match snapshot_mode(&state, &authorization, &receipt, false, None).await
    {
        Ok(value) => value,
        Err(error) => return error,
    };
    open_legacy(state, authorization, receipt, after, initial, bound)
}

fn open_legacy(
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
    initial: V2ResponseObject,
    bound: bool,
) -> Response {
    let events = projection::events(&initial, bound);
    if after.is_some_and(|after| after >= events.len() as u64) {
        return error::field(
            "starting_after",
            "invalid_replay_cursor",
            "The cursor is ahead of this response's durable events.",
        );
    }
    let response_id = initial.id.clone();
    let command_id = receipt.command_id.to_string();
    let evidence = Contract::new(ContractKind::ResponsesV2);
    let open = Open {
        evidence: evidence.clone(),
        state,
        authorization,
        receipt,
        next: after.map_or(0, |value| value + 1),
        pending: events.into(),
        terminal: initial.terminal(),
        failed: false,
    };
    let body = stream::unfold(open, |mut open| async move {
        loop {
            if let Some(event) = open.pending.pop_front() {
                let sequence = event["sequence_number"]
                    .as_u64()
                    .expect("event sequence is generated");
                if sequence < open.next {
                    continue;
                }
                // A buffered frame gets its own policy/deadline check after any
                // consumer backpressure; polling never renews the authority.
                if let Err(error) = open
                    .state
                    .reads
                    .validate_event_stream(&open.authorization)
                    .await
                {
                    open.evidence.record(CloseReason::read(&error));
                    return open.fail();
                }
                open.next = sequence + 1;
                return Some((Ok::<_, Infallible>(sse(event)), open));
            }
            if open.terminal || open.failed {
                if !open.failed {
                    open.evidence.record(CloseReason::NormalTerminal);
                }
                return None;
            }
            #[cfg(any(test, feature = "test-support"))]
            open.state.wait_for_replay_refresh(open.next).await;
            let wake = tokio::time::Instant::now() + Duration::from_millis(250);
            let wake = open
                .authorization
                .revalidation_deadline()
                .map_or(wake, |expires| expires.min(wake));
            tokio::time::sleep_until(wake).await;
            let poll = Poll::new(Kind::ResponsesV2);
            match snapshot_mode(
                &open.state,
                &open.authorization,
                &open.receipt,
                false,
                Some(&open.evidence),
            )
            .await
            {
                Ok((response, bound)) => {
                    open.terminal = response.terminal();
                    open.pending = projection::events(&response, bound).into();
                    // Projection sequences increase monotonically; inspect only the tail.
                    poll.complete(open.pending.back().is_some_and(|event| {
                        event["sequence_number"]
                            .as_u64()
                            .is_some_and(|seq| seq >= open.next)
                    }));
                }
                Err(_) => return open.fail(),
            }
        }
    });
    response(body, &response_id, &command_id, evidence)
}

pub(super) fn pagination_required() -> Response {
    error::field("output","response_pagination_required",
        "The complete response exceeds the result budget. Read its /output pages and /events replay; output was not truncated.")
}

pub(super) fn response(
    body: impl futures_util::Stream<Item = Result<Event, Infallible>> + Send + 'static,
    response_id: &str,
    command_id: &str,
    evidence: Contract,
) -> Response {
    let mut response = Sse::new(stream_telemetry::observe(body, Kind::ResponsesV2))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response.extensions_mut().insert(evidence);
    let headers = response.headers_mut();
    headers.insert("cache-control", "no-cache, no-store".parse().unwrap());
    headers.insert("x-accel-buffering", "no".parse().unwrap());
    headers.insert(
        "x-cma-response-id",
        response_id.parse().expect("server response identity"),
    );
    headers.insert(
        "x-cma-command-id",
        command_id.parse().expect("server command identity"),
    );
    headers.insert("link", "</api/v2>; rel=\"service-desc\"".parse().unwrap());
    response
}
struct Open {
    evidence: Contract,
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    next: u64,
    pending: VecDeque<Value>,
    terminal: bool,
    failed: bool,
}
impl Open {
    fn fail(mut self) -> Option<(Result<Event, Infallible>, Self)> {
        if self.failed {
            return None;
        }
        self.evidence.record(CloseReason::ServerError);
        self.failed = true;
        self.pending.clear();
        // Observation failure cannot change or fabricate the admitted command's
        // durable outcome. Reconnect/retrieve under fresh current authorization.
        let event = json!({"type":"error","sequence_number":self.next,"code":"response_observation_unavailable","message":"Response observation ended. Retrieve or reconnect with current authorization.","param":null});
        Some((Ok(sse(event)), self))
    }
}
pub(super) fn sse(event: Value) -> Event {
    let mut result = Event::default().event(event["type"].as_str().unwrap());
    // Only durable events carry replay IDs. Observation errors are connection
    // events and must never advance the reconnect cursor past future output.
    if event["type"] != "error" {
        result = result.id(event["sequence_number"].as_u64().unwrap().to_string());
    }
    result.json_data(event).expect("generated event is JSON")
}
