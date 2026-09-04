use std::{collections::HashMap, collections::VecDeque, convert::Infallible, sync::Arc};

use axum::{
    http::{header, HeaderValue},
    response::{sse::Event as SseEvent, sse::KeepAlive, IntoResponse, Response, Sse},
};
use futures_util::{stream, Stream};
use layer2_core::{Caller, Event, EventKind, LiveFrame, Session, SessionId, SessionService};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::retrieve;
use super::{message_id, reasoning_id, response_id, ResponseError, ResponseObject, ResponseStatus};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

/// Decoded generic event shape used by contract fixtures and SDK parity tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsesConnectionEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub sequence_number: u64,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

struct ToolState {
    kind: String,
    target: String,
    started_at: Option<u64>,
    arguments: Option<layer2_core::ToolBody>,
}

struct Open {
    service: Arc<SessionService>,
    caller: Caller,
    session: Session,
    id: SessionId,
    turn: u64,
    cursor: u64,
    sequence: u64,
    pending: VecDeque<Value>,
    watching: Option<tokio::sync::broadcast::Receiver<LiveFrame>>,
    announced: bool,
    reasoning_started: bool,
    reasoning_text: String,
    message_started: bool,
    message_text: String,
    committed_text: Option<(u64, String)>,
    history: Vec<Event>,
    tools: HashMap<String, ToolState>,
    ended: bool,
}

pub(super) fn open(
    service: Arc<SessionService>,
    caller: Caller,
    session: Session,
    turn: u64,
    watching: Option<tokio::sync::broadcast::Receiver<LiveFrame>>,
) -> Response {
    let id = session.id.clone();
    sse_response(events(Open {
        service,
        caller,
        session,
        id,
        turn,
        cursor: turn.saturating_sub(1),
        sequence: 0,
        pending: VecDeque::new(),
        watching,
        announced: false,
        reasoning_started: false,
        reasoning_text: String::new(),
        message_started: false,
        message_text: String::new(),
        committed_text: None,
        history: Vec::new(),
        tools: HashMap::new(),
        ended: false,
    }))
}

pub(super) fn provisioning_timeout(id: SessionId, session: Session) -> Response {
    let response = failed_response(
        session,
        format!("resp_{}_0", id.as_str()),
        "provisioning_timeout",
        "The conversation did not become reachable before the readiness deadline.",
    );
    let value = with_envelope("response.failed", 0, json!({ "response": response }));
    sse_response(stream::iter([Ok::<_, Infallible>(sse_event(value))]))
}

fn sse_response<S>(events: S) -> Response
where
    S: Stream<Item = Result<SseEvent, Infallible>> + Send + 'static,
{
    let mut response = Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(KEEPALIVE_INTERVAL)
                .text("keepalive"),
        )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn events(open: Open) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    stream::unfold(open, |mut open| async move {
        loop {
            if let Some(value) = open.pending.pop_front() {
                return Some((Ok(sse_event(value)), open));
            }
            if open.ended {
                return None;
            }

            let live = next_live(&mut open.watching).await;
            let durable = match open
                .service
                .events_since(&open.caller, &open.id, Some(open.cursor))
                .await
            {
                Ok(events) => events,
                Err(error) => {
                    let mut response = failed_response(
                        open.session.clone(),
                        response_id(&open.id, open.turn),
                        error.code(),
                        &error.to_string(),
                    );
                    response.created_at = 0;
                    open.push("response.failed", json!({ "response": response }));
                    open.ended = true;
                    continue;
                }
            };
            if !open.announced {
                let Some(mut response) = retrieve::project(&open.session, &durable, open.turn)
                else {
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                };
                response.status = ResponseStatus::InProgress;
                response.error = None;
                response.incomplete_details = None;
                response.output.clear();
                response.usage = None;
                open.push("response.created", json!({ "response": response.clone() }));
                open.push("response.in_progress", json!({ "response": response }));
                open.announced = true;
            }

            let terminal = open.fold_durable(&durable);
            if !terminal {
                if let Some(frame) = live {
                    open.fold_live(frame);
                }
            }
        }
    })
}

async fn next_live(
    watching: &mut Option<tokio::sync::broadcast::Receiver<LiveFrame>>,
) -> Option<LiveFrame> {
    let Some(receiver) = watching.as_mut() else {
        tokio::time::sleep(POLL_INTERVAL).await;
        return None;
    };
    match tokio::time::timeout(POLL_INTERVAL, receiver.recv()).await {
        Ok(Ok(frame)) => Some(frame),
        Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => None,
        Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
            *watching = None;
            None
        }
        Err(_) => None,
    }
}

impl Open {
    fn push(&mut self, kind: &str, body: Value) {
        let sequence = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        self.pending.push_back(with_envelope(kind, sequence, body));
    }

    fn fold_live(&mut self, frame: LiveFrame) {
        match frame {
            LiveFrame::ThoughtDelta(delta) => {
                let item_id = reasoning_id(&self.id, self.turn);
                if !self.reasoning_started {
                    self.push(
                        "response.reasoning_summary_part.added",
                        json!({
                            "item_id": item_id,
                            "output_index": 0,
                            "summary_index": 0,
                            "part": { "type": "summary_text", "text": "" }
                        }),
                    );
                    self.reasoning_started = true;
                }
                self.reasoning_text.push_str(&delta);
                self.push(
                    "response.reasoning_summary_text.delta",
                    json!({
                        "item_id": item_id,
                        "output_index": 0,
                        "summary_index": 0,
                        "delta": delta
                    }),
                );
            }
            LiveFrame::MessageDelta(delta) => {
                self.ensure_message_started();
                self.message_text.push_str(&delta);
                self.push(
                    "response.output_text.delta",
                    json!({
                        "item_id": message_id(&self.id, self.turn),
                        "output_index": 0,
                        "content_index": 0,
                        "delta": delta,
                        "logprobs": []
                    }),
                );
            }
            LiveFrame::TerminalOutput(_)
            | LiveFrame::AvailableCommands(_)
            | LiveFrame::Exited { .. } => {}
        }
    }

    fn fold_durable(&mut self, events: &[Event]) -> bool {
        let mut completed = false;
        for event in events {
            if matches!(event.kind, EventKind::TurnStarted) && event.seq != self.turn {
                break;
            }
            self.cursor = self.cursor.max(event.seq);
            self.history.push(event.clone());
            match &event.kind {
                EventKind::MessageCommitted { content } => {
                    if let Some((seq, previous)) =
                        self.committed_text.replace((event.seq, content.clone()))
                    {
                        self.push(
                            "response.cma_chat.intermediate_message",
                            json!({
                                "response_id": response_id(&self.id, self.turn),
                                "item": {
                                    "id": format!("workmsg_{}_{}", self.id.as_str(), seq),
                                    "type": "cma_chat.intermediate_message",
                                    "text": previous
                                }
                            }),
                        );
                        // The next live message is a new piece of work rather
                        // than a continuation of the one just retained.
                        self.message_text.clear();
                    }
                }
                EventKind::ReasoningCommitted { content } => {
                    let item_id = reasoning_id(&self.id, self.turn);
                    if !self.reasoning_started {
                        self.push(
                            "response.reasoning_summary_part.added",
                            json!({
                                "item_id": item_id,
                                "output_index": 0,
                                "summary_index": 0,
                                "part": { "type": "summary_text", "text": "" }
                            }),
                        );
                        self.reasoning_started = true;
                    }
                    if let Some(suffix) = content.strip_prefix(&self.reasoning_text) {
                        if !suffix.is_empty() {
                            self.push(
                                "response.reasoning_summary_text.delta",
                                json!({
                                    "item_id": item_id,
                                    "output_index": 0,
                                    "summary_index": 0,
                                    "delta": suffix
                                }),
                            );
                        }
                    }
                    self.reasoning_text.clone_from(content);
                }
                EventKind::ToolCallStarted {
                    call_id,
                    title,
                    kind,
                    arguments,
                } => {
                    self.tools.insert(
                        call_id.clone(),
                        ToolState {
                            kind: kind.clone(),
                            target: title.clone(),
                            started_at: layer2_core::seconds_from_rfc3339(&event.appended_at),
                            arguments: arguments.clone(),
                        },
                    );
                    self.push(
                        "response.cma_chat.tool_activity",
                        json!({
                            "response_id": response_id(&self.id, self.turn),
                            "item": {
                                "id": format!("tool_{}_{}", self.id.as_str(), event.seq),
                                "type": "cma_chat.tool_activity",
                                "call_id": call_id,
                                "kind": kind,
                                "target": title,
                                "status": "started",
                                "duration_ms": null,
                                "arguments": arguments
                            }
                        }),
                    );
                }
                EventKind::ToolCallUpdated {
                    call_id,
                    status,
                    result,
                } => {
                    let tool = self.tools.get(call_id);
                    let duration = tool.and_then(|tool| {
                        tool.started_at.and_then(|started| {
                            layer2_core::seconds_from_rfc3339(&event.appended_at)
                                .map(|ended| ended.saturating_sub(started).saturating_mul(1_000))
                        })
                    });
                    let kind = tool
                        .map(|tool| tool.kind.clone())
                        .unwrap_or_else(|| "unknown".into());
                    let target = tool
                        .map(|tool| tool.target.clone())
                        .unwrap_or_else(|| "unknown".into());
                    let arguments = tool.and_then(|tool| tool.arguments.clone());
                    self.push(
                        "response.cma_chat.tool_activity",
                        json!({
                            "response_id": response_id(&self.id, self.turn),
                            "item": {
                                "id": format!("tool_{}_{}", self.id.as_str(), event.seq),
                                "type": "cma_chat.tool_activity",
                                "call_id": call_id,
                                "kind": kind,
                                "target": target,
                                "status": status,
                                "duration_ms": duration,
                                "arguments": arguments,
                                "result": result
                            }
                        }),
                    );
                }
                EventKind::TurnCompleted => completed = true,
                _ => {}
            }
        }
        if !completed {
            return false;
        }
        let Some(final_response) = retrieve::project(&self.session, &self.history, self.turn)
        else {
            return false;
        };
        self.finish(final_response);
        true
    }

    fn ensure_message_started(&mut self) {
        if self.message_started {
            return;
        }
        let item_id = message_id(&self.id, self.turn);
        self.push(
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {
                    "id": item_id,
                    "type": "message",
                    "status": "in_progress",
                    "role": "assistant",
                    "content": []
                }
            }),
        );
        self.push(
            "response.content_part.added",
            json!({
                "item_id": item_id,
                "output_index": 0,
                "content_index": 0,
                "part": {
                    "type": "output_text",
                    "text": "",
                    "annotations": [],
                    "logprobs": []
                }
            }),
        );
        self.message_started = true;
    }

    fn finish(&mut self, response: ResponseObject) {
        if self.reasoning_started {
            let item_id = reasoning_id(&self.id, self.turn);
            let reasoning = self.reasoning_text.clone();
            self.push(
                "response.reasoning_summary_text.done",
                json!({
                    "item_id": item_id,
                    "output_index": 0,
                    "summary_index": 0,
                    "text": reasoning.clone()
                }),
            );
            self.push(
                "response.reasoning_summary_part.done",
                json!({
                    "item_id": item_id,
                    "output_index": 0,
                    "summary_index": 0,
                    "part": { "type": "summary_text", "text": reasoning }
                }),
            );
        }
        if let Some((_, text)) = self.committed_text.clone() {
            self.ensure_message_started();
            if let Some(suffix) = text.strip_prefix(&self.message_text) {
                if !suffix.is_empty() {
                    self.push(
                        "response.output_text.delta",
                        json!({
                            "item_id": message_id(&self.id, self.turn),
                            "output_index": 0,
                            "content_index": 0,
                            "delta": suffix,
                            "logprobs": []
                        }),
                    );
                }
            }
            let item_id = message_id(&self.id, self.turn);
            self.push(
                "response.output_text.done",
                json!({
                    "item_id": item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "text": text,
                    "logprobs": []
                }),
            );
            self.push(
                "response.content_part.done",
                json!({
                    "item_id": item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "part": {
                        "type": "output_text",
                        "text": text,
                        "annotations": [],
                        "logprobs": []
                    }
                }),
            );
            self.push(
                "response.output_item.done",
                json!({
                    "output_index": 0,
                    "item": response
                        .output
                        .first()
                        .expect("committed text projects one output message")
                }),
            );
        }
        let kind = match response.status {
            ResponseStatus::Failed => "response.failed",
            ResponseStatus::Incomplete => "response.incomplete",
            ResponseStatus::Completed | ResponseStatus::InProgress => "response.completed",
        };
        self.push(kind, json!({ "response": response }));
        self.ended = true;
    }
}

fn with_envelope(kind: &str, sequence: u64, body: Value) -> Value {
    let mut object = match body {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    object.insert("type".into(), kind.into());
    object.insert("sequence_number".into(), sequence.into());
    Value::Object(object)
}

fn sse_event(value: Value) -> SseEvent {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("response.failed");
    SseEvent::default()
        .event(kind)
        .json_data(value)
        .expect("Responses events contain only serializable values")
}

fn failed_response(mut session: Session, id: String, code: &str, message: &str) -> ResponseObject {
    let model = session
        .options
        .model
        .take()
        .unwrap_or_else(|| super::DEFAULT_MODEL_NAME.into());
    ResponseObject {
        id,
        object: super::OBJECT_RESPONSE,
        created_at: 0,
        status: ResponseStatus::Failed,
        background: false,
        error: Some(ResponseError {
            code: code.into(),
            message: message.into(),
        }),
        incomplete_details: None,
        instructions: None,
        max_output_tokens: None,
        model,
        output: Vec::new(),
        parallel_tool_calls: false,
        previous_response_id: None,
        reasoning: json!({ "effort": null, "summary": null }),
        store: true,
        temperature: None,
        text: json!({ "format": { "type": "text" }, "verbosity": "medium" }),
        tool_choice: "none",
        tools: Vec::new(),
        top_p: None,
        truncation: "disabled",
        usage: None,
        metadata: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn absent_live_receiver_waits_for_the_durable_poll_interval() {
        let mut watching = None;
        let started = tokio::time::Instant::now();
        assert!(next_live(&mut watching).await.is_none());
        assert!(started.elapsed() >= POLL_INTERVAL);
    }
}
