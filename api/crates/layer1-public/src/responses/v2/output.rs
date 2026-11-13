use super::super::{OutputMessage, OutputText};
use layer2_core::{sandbox::AgentLogKind, AgentId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum V2OutputItem {
    Message(OutputMessage),
    Activity(ActivityItem),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ActivityItem {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub server_label: &'static str,
    pub name: String,
    pub arguments: String,
    pub output: Option<String>,
    #[serde(rename = "cma:output", skip_serializing_if = "Option::is_none")]
    pub recorded_output: Option<Value>,
    pub error: Option<Value>,
    pub status: &'static str,
}

/// Events follow immutable log order, while the response retains each item's
/// latest state. Completing a tool never moves events ahead of earlier text.
#[derive(Default)]
pub(super) struct OutputProjection {
    pub items: Vec<V2OutputItem>,
    pub events: Vec<Value>,
    tools: BTreeMap<String, usize>,
    messages: usize,
    base_index: usize,
}
impl OutputProjection {
    pub(super) fn at(item: usize, message: usize) -> Self {
        Self {
            base_index: item,
            messages: message,
            ..Self::default()
        }
    }
    pub fn observe(&mut self, kind: &AgentLogKind, agent: &AgentId, turn: u64) {
        match kind {
            AgentLogKind::AgentMessage { content } => self.message(agent, turn, content),
            AgentLogKind::ToolCallStarted {
                call_id,
                title,
                status,
                ..
            } => {
                if self.tools.contains_key(call_id) {
                    return;
                }
                let item = ActivityItem {
                    id: format!("mcp_{call_id}"),
                    kind: "mcp_call",
                    server_label: "cma",
                    name: title.chars().take(200).collect(),
                    arguments: String::new(),
                    output: None,
                    recorded_output: None,
                    error: None,
                    status: "in_progress",
                };
                let index = self.items.len();
                self.push(
                    "response.output_item.added",
                    json!({"output_index":self.base_index+index,"item":item}),
                );
                self.push(
                    "response.mcp_call.in_progress",
                    json!({"output_index":self.base_index+index,"item_id":item.id}),
                );
                self.items.push(V2OutputItem::Activity(item));
                self.tools.insert(call_id.clone(), index);
                self.tool_update(call_id, status);
            }
            AgentLogKind::ToolCallUpdated {
                call_id,
                status,
                output,
                ..
            } => {
                if let Some(output) = output {
                    self.tool_output(call_id, output);
                }
                self.tool_update(call_id, status);
            }
            AgentLogKind::ToolOutputReported { call_id, output } => {
                self.tool_output(call_id, output)
            }
            _ => {}
        }
    }
    fn tool_output(&mut self, call_id: &str, output: &layer2_core::sandbox::RecordedToolOutput) {
        let Some(&index) = self.tools.get(call_id) else {
            return;
        };
        let V2OutputItem::Activity(item) = &mut self.items[index] else {
            return;
        };
        item.recorded_output = Some(json!(output));
    }
    fn tool_update(&mut self, call_id: &str, status: &str) {
        let status = match status {
            "completed" => "completed",
            "failed" => "failed",
            _ => return,
        };
        let Some(&index) = self.tools.get(call_id) else {
            return;
        };
        let V2OutputItem::Activity(item) = &mut self.items[index] else {
            return;
        };
        if item.status != "in_progress" {
            return;
        }
        item.status = status;
        let item = item.clone();
        self.push(
            if status == "completed" {
                "response.mcp_call.completed"
            } else {
                "response.mcp_call.failed"
            },
            json!({"output_index":self.base_index+index,"item_id":item.id}),
        );
        self.push(
            "response.output_item.done",
            json!({"output_index":self.base_index+index,"item":item}),
        );
    }
    fn message(&mut self, agent: &AgentId, turn: u64, text: &str) {
        let index = self.base_index + self.items.len();
        // Message IDs retain their existing identity in conversation item reads.
        let item = OutputMessage {
            id: super::super::retrieve::output_id(agent, turn, self.messages),
            kind: "message",
            status: "completed",
            role: "assistant",
            content: vec![OutputText {
                kind: "output_text",
                text: text.into(),
                annotations: vec![],
                logprobs: vec![],
            }],
        };
        self.messages += 1;
        let mut empty = item.clone();
        empty.status = "in_progress";
        empty.content.clear();
        self.push(
            "response.output_item.added",
            json!({"output_index":index,"item":empty}),
        );
        let part = &item.content[0];
        let mut empty = part.clone();
        empty.text.clear();
        self.push(
            "response.content_part.added",
            json!({"output_index":index,"item_id":item.id,"content_index":0,"part":empty}),
        );
        if !text.is_empty() {
            self.push("response.output_text.delta", json!({"output_index":index,"item_id":item.id,"content_index":0,"delta":text,"logprobs":[]}));
        }
        self.push("response.output_text.done", json!({"output_index":index,"item_id":item.id,"content_index":0,"text":text,"logprobs":[]}));
        self.push(
            "response.content_part.done",
            json!({"output_index":index,"item_id":item.id,"content_index":0,"part":part}),
        );
        self.push(
            "response.output_item.done",
            json!({"output_index":index,"item":item}),
        );
        self.items.push(V2OutputItem::Message(item));
    }
    fn push(&mut self, kind: &str, mut body: Value) {
        body["type"] = kind.into();
        self.events.push(body);
    }
}
