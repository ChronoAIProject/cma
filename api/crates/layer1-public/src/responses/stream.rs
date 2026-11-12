use crate::stream_telemetry::{self, Kind, Poll};
use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    time::Duration,
};

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
#[path = "stream_indexed.rs"]
mod indexed;
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
    open_inner(state, caller, agent, turn, watching, None, false)
}

fn open_inner(
    state: AgentConversationState,
    caller: Caller,
    agent: Agent,
    turn: u64,
    watching: Option<layer2_core::sandbox::AgentLiveSubscription>,
    authorization: Option<AgentEventStreamAuthorization>,
    indexed: bool,
) -> Response {
    let open = Open {
        state,
        caller,
        agent,
        turn,
        cursor: turn.saturating_sub(1),
        sequence: 0,
        pending: VecDeque::new(),
        tools: HashMap::new(),
        announced: false,
        output_count: 0,
        next_output_index: 0,
        ended: false,
        live: live::LiveState::new(watching),
        authorization,
        indexed,
        pagination_required: false,
    };
    sse_response(stream::unfold(open, |mut open| async move {
        let mut quiet_pages = 0;
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
            if open.pagination_required {
                if let Some(held) = &open.authorization {
                    if open.state.reads.validate_event_stream(held).await.is_err() {
                        return None;
                    }
                }
                open.pagination_required = false;
                open.ended = true;
                let value = envelope(
                    "error",
                    open.sequence,
                    json!({"code":"response_pagination_required","message":"Read the response with output_page=true and its returned cursor; output was not truncated."}),
                );
                return Some((Ok(sse_event(value)), open));
            }
            if open.ended {
                return None;
            }
            let poll = Poll::new(Kind::Responses);
            let (agent, response, history, more) = if open.indexed {
                match open.read_indexed().await {
                    Ok(value) => value,
                    Err(_) => return None,
                }
            } else {
                let (agent, history) = match open.read().await {
                    Ok(value) => value,
                    Err(_) => return None,
                };
                let Some(response) =
                    retrieve::project(&open.state.agent_page_origin, &agent, &history, open.turn)
                else {
                    poll.complete(false);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                };
                (agent, response, history, false)
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
                    AgentLogKind::AgentMessage { content } => {
                        if open.indexed {
                            let message =
                                retrieve::output_message(&agent, turn, open.output_count, content);
                            open.append_message(&message, open.output_count);
                            open.output_count += 1;
                        } else {
                            open.append_output(&response, message_count);
                        }
                    }
                    AgentLogKind::ToolCallStarted {
                        call_id,
                        title,
                        kind,
                        status,
                    } => {
                        let item = json!({"id": format!("tool_{}_{}", agent.id, entry.seq), "type": "cma_chat.tool_activity",
                            "call_id": call_id, "kind": kind, "target": title, "status": status, "duration_ms": null});
                        open.tools.insert(call_id.clone(), item.clone());
                        open.push_tool(&response.id, item);
                    }
                    AgentLogKind::ToolOutputReported { call_id, output } => {
                        if let Some(item) = open.tools.get_mut(call_id) {
                            item["output"] = json!(output);
                            let item = item.clone();
                            open.push_tool(&response.id, item);
                        }
                    }
                    AgentLogKind::ToolCallUpdated {
                        call_id,
                        title,
                        status,
                        output,
                    } => {
                        let item = open.tools.entry(call_id.clone()).or_insert_with(|| json!({
                            "id": format!("tool_{}_{}", agent.id, entry.seq), "type": "cma_chat.tool_activity",
                            "call_id": call_id, "kind": "unknown", "target": null, "duration_ms": null
                        }));
                        item["status"] = json!(status);
                        if let Some(title) = title {
                            item["target"] = json!(title);
                        }
                        if let Some(output) = output {
                            item["output"] = json!(output);
                        }
                        let item = item.clone();
                        open.push_tool(&response.id, item);
                    }
                    _ => {}
                }
            }
            if open.indexed
                && (serde_json::to_vec(&open.tools).map_or(true, |b| b.len() > 4 * 1024 * 1024)
                    || serde_json::to_vec(&open.pending)
                        .map_or(true, |b| b.len() > 6 * 4 * 1024 * 1024))
            {
                open.pending.clear();
                open.pagination_required = true;
            }
            if response.status != ResponseStatus::InProgress && !more && !open.pagination_required {
                open.finish_reasoning();
                let kind = match response.status {
                    ResponseStatus::Completed => "response.completed",
                    ResponseStatus::Failed => "response.failed",
                    _ => "response.incomplete",
                };
                open.push(kind, json!({"response": response}));
                open.ended = true;
            } else if !more && !open.pagination_required {
                let committed = history
                    .iter()
                    .rev()
                    .find(|entry| matches!(entry.kind, AgentLogKind::AgentMessage { .. }))
                    .map_or(0, |entry| entry.seq);
                open.drain_live(&agent, committed);
            }
            poll.complete(!open.pending.is_empty());
            if open.indexed && open.pending.is_empty() && more {
                quiet_pages += 1;
                if quiet_pages == 8 {
                    // A sparse turn must not scan its entire prefix in one
                    // body poll. SSE comments preserve event numbering and
                    // every intermediate update while yielding to backpressure.
                    return Some((Ok(Event::default().comment("keep-alive")), open));
                }
            }
            if open.pending.is_empty() && !more {
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
    tools: HashMap<String, Value>,
    announced: bool,
    output_count: usize,
    next_output_index: usize,
    ended: bool,
    live: live::LiveState,
    authorization: Option<AgentEventStreamAuthorization>,
    indexed: bool,
    pagination_required: bool,
}

impl Open {
    fn push_tool(&mut self, response_id: &str, item: Value) {
        self.push(
            "response.cma_chat.tool_activity",
            json!({"response_id": response_id, "item": item}),
        );
    }

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
            self.append_message(message, ordinal);
        }
        self.output_count = count;
    }

    fn append_message(&mut self, message: &super::OutputMessage, ordinal: usize) {
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
    let mut response = Sse::new(stream_telemetry::observe(stream, Kind::Responses))
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
