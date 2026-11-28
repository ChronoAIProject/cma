use super::{
    error, projection,
    read_cycle::{indexed_read_key, read_category, read_cycle, FirstFailure, ReadFailure},
    ResponsesV2State, V2ResponseObject,
};
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
    snapshot_mode(state, &authorization, receipt, true)
        .await
        .map_err(http_error)
}

async fn snapshot_mode(
    state: &ResponsesV2State,
    authorization: &AgentEventStreamAuthorization,
    receipt: &CommandReceiptView,
    indexed_allowed: bool,
) -> Result<(V2ResponseObject, bool), ReadFailure> {
    let id = receipt.response_id.as_ref().ok_or(ReadFailure::Receipt)?;
    let reading = state
        .commands
        .response_for_stream(authorization, id)
        .await
        .map_err(ReadFailure::Command)?;
    if reading.command.receipt != *receipt {
        return Err(ReadFailure::Receipt);
    }
    let history = if let Some(turn) = reading.command.turn_sequence {
        let origin = state.agent_page_origin.clone();
        let indexed_reading = reading.clone();
        let key = indexed_read_key(&reading, None, authorization.is_personal_owner())?;
        let observation = state
            .reads
            .read_response_observation(authorization, turn, Some(key), move |source| {
                Box::pin(async move {
                    if !indexed_allowed {
                        return Err(layer2_core::sandbox::AgentReadError::Unavailable(
                            "source format changed; reconnect at the same durable cursor".into(),
                        ));
                    }
                    projection::indexed::complete(source, &origin, indexed_reading).await
                })
            })
            .await
            .map_err(ReadFailure::from)?;
        match observation {
            AgentResponseObservation::Indexed(result) => {
                return result
                    .map(|response| (response, true))
                    .ok_or(ReadFailure::Pagination)
            }
            AgentResponseObservation::Legacy(history) => history,
        }
    } else {
        state
            .reads
            .read_unbound_response_snapshot(authorization)
            .await
            .map_err(ReadFailure::from)?
    };
    let bound = reading.command.turn_sequence.is_some();
    let response = super::shared_projection::project(state, reading, &history)
        .ok_or(ReadFailure::Projection)?;
    state
        .reads
        .validate_event_stream(authorization)
        .await
        .map_err(ReadFailure::Release)?;
    Ok((response, bound))
}

fn http_error(failure: ReadFailure) -> Response {
    match failure {
        ReadFailure::Command(error) => error::admission(error),
        ReadFailure::Header(error)
        | ReadFailure::Source(error)
        | ReadFailure::Release(error)
        | ReadFailure::SourceAndRelease { release: error, .. } => error::read(error),
        ReadFailure::Pagination => pagination_required(),
        _ => error::unavailable(),
    }
}

async fn legacy_refresh(
    state: &ResponsesV2State,
    authorization: &AgentEventStreamAuthorization,
    receipt: &CommandReceiptView,
    next: u64,
    first_failure: &mut FirstFailure,
) -> Result<PreparedLegacy, ReadFailure> {
    struct Context<'a> {
        state: &'a ResponsesV2State,
        authorization: &'a AgentEventStreamAuthorization,
        receipt: &'a CommandReceiptView,
        first_failure: &'a mut FirstFailure,
    }
    read_cycle(
        &mut Context {
            state,
            authorization,
            receipt,
            first_failure,
        },
        authorization.read_deadline(),
        authorization.revalidation_deadline(),
        |context| {
            Box::pin(async {
                let (response, bound) =
                    snapshot_mode(context.state, context.authorization, context.receipt, false)
                        .await?;
                Ok(PreparedLegacy {
                    pending: projection::events(&response, bound).into(),
                    terminal: response.terminal(),
                    response_id: response.id,
                })
            })
        },
        |context, failure, available| {
            let retry = available && failure.advanced();
            context.first_failure.record(
                "legacy",
                next,
                failure.diagnostic(),
                false,
                if retry { "recapture" } else { "none" },
            );
            retry
        },
    )
    .await
}

struct PreparedLegacy {
    pending: VecDeque<Value>,
    terminal: bool,
    response_id: String,
}

pub(super) async fn open(
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
    known_bound: Option<bool>,
    post: bool,
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
        return super::stream_pages::open(state, authorization, receipt, after, post).await;
    }
    let mut first_failure = FirstFailure::new(&receipt, post);
    let next = after.map_or(0, |after| after.saturating_add(1));
    let indexed = match state.reads.output_is_indexed(&authorization).await {
        Ok(indexed) => indexed,
        Err(value) => {
            first_failure.record(
                "format",
                next,
                ("source_format", read_category(&value)),
                false,
                "none",
            );
            return error::read(value);
        }
    };
    if state.paged_events || indexed {
        return super::stream_pages::open(state, authorization, receipt, after, post).await;
    }
    let initial =
        match legacy_refresh(&state, &authorization, &receipt, next, &mut first_failure).await {
            Ok(value) => value,
            Err(error) => return http_error(error),
        };
    open_legacy(state, authorization, receipt, after, initial, first_failure)
}

fn open_legacy(
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
    initial: PreparedLegacy,
    first_failure: FirstFailure,
) -> Response {
    if after.is_some_and(|after| after >= initial.pending.len() as u64) {
        return error::field(
            "starting_after",
            "invalid_replay_cursor",
            "The cursor is ahead of this response's durable events.",
        );
    }
    let response_id = initial.response_id;
    let command_id = receipt.command_id.to_string();
    let evidence = Contract::new(ContractKind::ResponsesV2);
    let open = Open {
        evidence: evidence.clone(),
        first_failure,
        state,
        authorization,
        receipt,
        next: after.map_or(0, |value| value + 1),
        pending: initial.pending,
        terminal: initial.terminal,
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
                    open.first_failure.record(
                        "legacy",
                        open.next,
                        ("frame_authorization", read_category(&error)),
                        false,
                        "none",
                    );
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
            match legacy_refresh(
                &open.state,
                &open.authorization,
                &open.receipt,
                open.next,
                &mut open.first_failure,
            )
            .await
            {
                Ok(page) => {
                    open.terminal = page.terminal;
                    open.pending = page.pending;
                    // Projection sequences increase monotonically; inspect only the tail.
                    poll.complete(open.pending.back().is_some_and(|event| {
                        event["sequence_number"]
                            .as_u64()
                            .is_some_and(|seq| seq >= open.next)
                    }));
                }
                Err(failure) => {
                    open.evidence.record(failure.close_reason());
                    return open.fail();
                }
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
    first_failure: FirstFailure,
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
