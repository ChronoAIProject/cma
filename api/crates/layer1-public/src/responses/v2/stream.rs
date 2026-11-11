use super::{error, projection, ResponsesV2State, V2ResponseObject};
use crate::stream_telemetry::{self, Kind, Poll};
use axum::response::{
    sse::{Event, KeepAlive},
    IntoResponse, Response, Sse,
};
use futures_util::stream;
use layer2_core::sandbox::{agent_io::CommandReceiptView, AgentEventStreamAuthorization};
use serde_json::{json, Value};
use std::{collections::VecDeque, convert::Infallible, time::Duration};

pub(super) async fn snapshot(
    state: &ResponsesV2State,
    authorization: &AgentEventStreamAuthorization,
    receipt: &CommandReceiptView,
) -> Result<(V2ResponseObject, bool), Response> {
    let id = receipt
        .response_id
        .as_ref()
        .ok_or_else(error::unavailable)?;
    let reading = state
        .commands
        .response_for_stream(authorization, id)
        .await
        .map_err(error::admission)?;
    if reading.command.receipt != *receipt {
        return Err(error::unavailable());
    }
    let bound = reading.command.turn_sequence.is_some();
    let origin = state.agent_page_origin.clone();
    let indexed_reading = reading.clone();
    if let Some(result) = state
        .reads
        .with_output_observation(
            authorization,
            reading.command.turn_sequence,
            move |source| {
                Box::pin(async move {
                    projection::indexed::complete(source, &origin, indexed_reading).await
                })
            },
        )
        .await
        .map_err(error::read)?
    {
        return result
            .map(|response| (response, bound))
            .ok_or_else(pagination_required);
    }
    let history = state
        .reads
        .response_event_stream_snapshot(authorization, reading.command.turn_sequence)
        .await
        .map_err(error::read)?;
    projection::project(&state.agent_page_origin, reading, &history)
        .map(|response| (response, bound))
        .ok_or_else(error::unavailable)
}

pub(super) async fn open(
    state: ResponsesV2State,
    authorization: AgentEventStreamAuthorization,
    receipt: CommandReceiptView,
    after: Option<u64>,
) -> Response {
    let indexed = match state.reads.output_is_indexed(&authorization).await {
        Ok(indexed) => indexed,
        Err(value) => return error::read(value),
    };
    if state.paged_events || indexed {
        return super::stream_pages::open(state, authorization, receipt, after).await;
    }
    let (initial, bound) = match snapshot(&state, &authorization, &receipt).await {
        Ok(value) => value,
        Err(error) => return error,
    };
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
    let open = Open {
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
                if open
                    .state
                    .reads
                    .validate_event_stream(&open.authorization)
                    .await
                    .is_err()
                {
                    return open.fail();
                }
                open.next = sequence + 1;
                return Some((Ok::<_, Infallible>(sse(event)), open));
            }
            if open.terminal || open.failed {
                return None;
            }
            let wake = tokio::time::Instant::now() + Duration::from_millis(250);
            let wake = open
                .authorization
                .revalidation_deadline()
                .map_or(wake, |expires| expires.min(wake));
            tokio::time::sleep_until(wake).await;
            let poll = Poll::new(Kind::ResponsesV2);
            match snapshot(&open.state, &open.authorization, &open.receipt).await {
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
    response(body, &response_id, &command_id)
}

pub(super) fn pagination_required() -> Response {
    error::field("output","response_pagination_required",
        "The complete response exceeds the result budget. Read its /output pages and /events replay; output was not truncated.")
}

pub(super) fn response(
    body: impl futures_util::Stream<Item = Result<Event, Infallible>> + Send + 'static,
    response_id: &str,
    command_id: &str,
) -> Response {
    let mut response = Sse::new(stream_telemetry::observe(body, Kind::ResponsesV2))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
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
