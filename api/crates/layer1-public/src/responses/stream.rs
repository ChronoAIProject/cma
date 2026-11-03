use std::{collections::VecDeque, convert::Infallible, time::Duration};

use axum::response::{
    sse::{Event, KeepAlive},
    IntoResponse, Response, Sse,
};
use futures_util::{stream, Stream};
use layer2_core::{
    sandbox::{Agent, AgentEventStreamAuthorization, AgentLogKind},
    Caller,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{retrieve, ResponseObject, ResponseStatus};
use crate::AgentConversationState;

#[path = "stream_live.rs"]
mod live;

#[path = "stream_authorized.rs"]
mod authorized;
pub(super) use authorized::open as open_authorized;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsesConnectionEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub sequence_number: u64,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

#[cfg(test)]
pub(super) fn open(
    state: AgentConversationState,
    caller: Caller,
    agent: Agent,
    turn: u64,
    watching: Option<layer2_core::sandbox::AgentLiveSubscription>,
) -> Response {
    open_inner(state, caller, agent, turn, watching, None)
}

fn open_inner(
    state: AgentConversationState,
    caller: Caller,
    agent: Agent,
    turn: u64,
    watching: Option<layer2_core::sandbox::AgentLiveSubscription>,
    authorization: Option<AgentEventStreamAuthorization>,
) -> Response {
    let open = Open {
        state,
        caller,
        agent,
        turn,
        cursor: turn.saturating_sub(1),
        sequence: 0,
        pending: VecDeque::new(),
        announced: false,
        output_count: 0,
        next_output_index: 0,
        ended: false,
        live: live::LiveState::new(watching),
        authorization,
    };
    sse_response(stream::unfold(open, |mut open| async move {
        loop {
            if let Some(value) = open.pending.pop_front() {
                if let Some(authorization) = &open.authorization {
                    if open
                        .state
                        .reads
                        .validate_event_stream(authorization)
                        .await
                        .is_err()
                    {
                        return None;
                    }
                }
                return Some((Ok::<_, Infallible>(sse_event(value)), open));
            }
            if open.ended {
                return None;
            }
            let (agent, history) = match open.read().await {
                Ok(snapshot) => snapshot,
                // Observation loss ends this connection. It cannot manufacture
                // a durable turn outcome or release buffered private frames.
                Err(_) => return None,
            };
            let Some(response) = retrieve::project(&agent, &history, open.turn) else {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            open.announce(&response);
            let mut message_count = 0;
            let turn = open.turn;
            for entry in history.iter().filter(|entry| entry.seq >= turn) {
                if matches!(entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence != open.turn)
                {
                    break;
                }
                if matches!(entry.kind, AgentLogKind::AgentMessage { .. }) {
                    message_count += 1;
                }
                if entry.seq <= open.cursor {
                    continue;
                }
                open.cursor = entry.seq;
                match &entry.kind {
                    AgentLogKind::AgentMessage { .. } => open.append_output(&response, message_count),
                    AgentLogKind::ToolCallStarted { call_id, title, kind, status } => open.push("response.cma_chat.tool_activity", json!({
                        "response_id": response.id,
                        "item": {"id": format!("tool_{}_{}", agent.id, entry.seq), "type": "cma_chat.tool_activity",
                            "call_id": call_id, "kind": kind, "target": title, "status": status, "duration_ms": null}
                    })),
                    AgentLogKind::ToolOutputReported { call_id, output } => open.push("response.cma_chat.tool_activity", json!({
                        "call_id": call_id, "output": output,
                    })),
                    AgentLogKind::ToolCallUpdated { call_id, title, status, output } => open.push("response.cma_chat.tool_activity", json!({
                        "response_id": response.id,
                        "item": {"id": format!("tool_{}_{}", agent.id, entry.seq), "type": "cma_chat.tool_activity",
                            "call_id": call_id, "kind": "unknown", "target": title, "status": status, "duration_ms": null, "output": output}
                    })),
                    _ => {}
                }
            }
            if response.status != ResponseStatus::InProgress {
                open.finish_reasoning();
                let kind = match response.status {
                    ResponseStatus::Completed => "response.completed",
                    ResponseStatus::Failed => "response.failed",
                    _ => "response.incomplete",
                };
                open.push(kind, json!({"response": response}));
                open.ended = true;
            } else {
                let committed = history
                    .iter()
                    .rev()
                    .find(|entry| matches!(entry.kind, AgentLogKind::AgentMessage { .. }))
                    .map_or(0, |entry| entry.seq);
                open.drain_live(&agent, committed);
            }
            if open.pending.is_empty() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }))
}

struct Open {
    state: AgentConversationState,
    caller: Caller,
    agent: Agent,
    turn: u64,
    cursor: u64,
    sequence: u64,
    pending: VecDeque<Value>,
    announced: bool,
    output_count: usize,
    next_output_index: usize,
    ended: bool,
    live: live::LiveState,
    authorization: Option<AgentEventStreamAuthorization>,
}

impl Open {
    fn announce(&mut self, response: &ResponseObject) {
        if self.announced {
            return;
        }
        let mut opening = response.clone();
        opening.status = ResponseStatus::InProgress;
        opening.error = None;
        opening.incomplete_details = None;
        opening.output.clear();
        opening.usage = None;
        self.push("response.created", json!({"response": opening}));
        self.push("response.in_progress", json!({"response": opening}));
        self.announced = true;
    }

    fn push(&mut self, kind: &str, body: Value) {
        self.pending.push_back(envelope(kind, self.sequence, body));
        self.sequence = self.sequence.saturating_add(1);
    }

    fn append_output(&mut self, response: &ResponseObject, count: usize) {
        for (ordinal, message) in response
            .output
            .iter()
            .enumerate()
            .take(count)
            .skip(self.output_count)
        {
            let announced = self.live.message_announced && ordinal == self.output_count;
            let index = if announced {
                self.live.message_index
            } else {
                let index = self.next_output_index;
                self.next_output_index += 1;
                index
            };
            if !announced {
                let mut opening = message.clone();
                opening.status = "in_progress";
                opening.content.clear();
                self.push(
                    "response.output_item.added",
                    json!({"output_index": index, "item": opening}),
                );
            }
            for (content_index, content) in message.content.iter().enumerate() {
                let mut empty = content.clone();
                empty.text.clear();
                if !announced {
                    self.push(
                        "response.content_part.added",
                        json!({"output_index": index, "item_id": message.id,
                        "content_index": content_index, "part": empty}),
                    );
                }
                let delta = if announced {
                    content.text.strip_prefix(&self.live.message_text)
                } else {
                    Some(content.text.as_str())
                };
                if let Some(delta) = delta.filter(|delta| !delta.is_empty()) {
                    self.push(
                        "response.output_text.delta",
                        json!({"output_index": index, "item_id": message.id,
                        "content_index": content_index, "delta": delta, "logprobs": []}),
                    );
                }
                self.push(
                    "response.output_text.done",
                    json!({"output_index": index, "item_id": message.id,
                    "content_index": content_index, "text": content.text, "logprobs": []}),
                );
                self.push(
                    "response.content_part.done",
                    json!({"output_index": index, "item_id": message.id,
                    "content_index": content_index, "part": content}),
                );
            }
            self.push(
                "response.output_item.done",
                json!({"output_index": index, "item": message}),
            );
            self.live.message_announced = false;
            self.live.message_text.clear();
        }
        self.output_count = count;
    }
}

fn envelope(kind: &str, sequence: u64, mut body: Value) -> Value {
    body["type"] = kind.into();
    body["sequence_number"] = sequence.into();
    body
}

fn sse_event(value: Value) -> Event {
    Event::default()
        .event(value["type"].as_str().unwrap_or("response.failed"))
        .json_data(value)
        .expect("Responses events are serializable")
}

fn sse_response<S: Stream<Item = Result<Event, Infallible>> + Send + 'static>(
    stream: S,
) -> Response {
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-cache, no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("x-accel-buffering", "no".parse().unwrap());
    response
}
