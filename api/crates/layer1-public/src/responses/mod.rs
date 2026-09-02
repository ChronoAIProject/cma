//! OpenAI Responses API chat projection (priority 2).
//!
//! This is deliberately a subset: conversations are deployment-managed chat
//! sessions, response creation is streaming and stateful, and tool activity is
//! carried in CMA's namespaced extension lane. Durable response state is always
//! reconstructed from Layer 2 events; this module owns no response database.

use std::collections::BTreeMap;

use axum::{http::StatusCode, response::Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::rest::{problem_document, problem_response, SessionsState};
use crate::{InterfaceProfile, Operation};

mod conversations;
mod create;
mod items;
mod retrieve;
mod stream;

#[cfg(test)]
mod tests;

pub use stream::ResponsesConnectionEvent;

pub(crate) const OBJECT_CONVERSATION: &str = "conversation";
pub(crate) const OBJECT_LIST: &str = "list";
pub(crate) const OBJECT_RESPONSE: &str = "response";
pub(crate) const DEFAULT_MODEL_NAME: &str = "deployment-default";

/// Routes contributed by the Responses chat projection.
pub fn router(state: SessionsState) -> axum::Router {
    documented_router(state).into()
}

/// Documented routes merged into CMA's single OpenAPI document.
pub(crate) fn documented_router(state: SessionsState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(conversations::create))
        .routes(routes!(conversations::retrieve))
        .routes(routes!(items::list))
        .routes(routes!(create::create))
        .routes(routes!(retrieve::retrieve, retrieve::delete))
        .routes(routes!(retrieve::cancel))
        .with_state(state)
}

/// The Responses chat projection of the core session model.
#[derive(Debug, Default)]
pub struct ResponsesInterface;

impl InterfaceProfile for ResponsesInterface {
    fn interface_id(&self) -> &'static str {
        "responses"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::GetUsage,
        ]
    }
}

/// Standard conversation resource for the supported chat subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Conversation {
    pub id: String,
    pub object: &'static str,
    pub created_at: u64,
    pub metadata: BTreeMap<String, String>,
}

/// Standard response lifecycle values emitted by this projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    InProgress,
    Completed,
    Failed,
    Incomplete,
}

/// Standard response error member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

/// Token counts reported for one response turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResponseUsage {
    pub input_tokens: u64,
    pub input_tokens_details: InputTokenDetails,
    pub output_tokens: u64,
    pub output_tokens_details: OutputTokenDetails,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct InputTokenDetails {
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputTokenDetails {
    pub reasoning_tokens: u64,
}

/// Assistant text content in a response output message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputText {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
    pub annotations: Vec<serde_json::Value>,
    pub logprobs: Vec<serde_json::Value>,
}

/// The only durable standard output item in the supported subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OutputMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub status: &'static str,
    pub role: &'static str,
    pub content: Vec<OutputText>,
}

/// A response resource reconstructed from one durable turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ResponseObject {
    pub id: String,
    pub object: &'static str,
    pub created_at: u64,
    pub status: ResponseStatus,
    pub background: bool,
    pub error: Option<ResponseError>,
    pub incomplete_details: Option<serde_json::Value>,
    pub instructions: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub model: String,
    pub output: Vec<OutputMessage>,
    pub parallel_tool_calls: bool,
    pub previous_response_id: Option<String>,
    pub reasoning: serde_json::Value,
    pub store: bool,
    pub temperature: Option<f64>,
    pub text: serde_json::Value,
    pub tool_choice: &'static str,
    pub tools: Vec<serde_json::Value>,
    pub top_p: Option<f64>,
    pub truncation: &'static str,
    pub usage: Option<ResponseUsage>,
    pub metadata: BTreeMap<String, String>,
}

pub(crate) fn response_id(session: &layer2_core::SessionId, turn: u64) -> String {
    format!("resp_{}_{}", session.as_str(), turn)
}

pub(crate) fn message_id(session: &layer2_core::SessionId, turn: u64) -> String {
    format!("msg_{}_{}", session.as_str(), turn)
}

pub(crate) fn reasoning_id(session: &layer2_core::SessionId, turn: u64) -> String {
    format!("rs_{}_{}", session.as_str(), turn)
}

pub(crate) fn not_found() -> Response {
    crate::rest::Problem::from(layer2_core::CoreError::SessionNotFound).into_response()
}

pub(crate) fn field_refusal(field: &str, detail: &str) -> Response {
    let mut body = problem_document(
        StatusCode::UNPROCESSABLE_ENTITY,
        "responses-field-unsupported",
        detail,
        "responses_field_unsupported",
    );
    if let Some(object) = body.as_object_mut() {
        object.insert("field".into(), field.into());
    }
    problem_response(StatusCode::UNPROCESSABLE_ENTITY, body)
}

pub(crate) fn previous_conflict() -> Response {
    let mut body = problem_document(
        StatusCode::CONFLICT,
        "previous-response-conflict",
        "previous_response_id is not the conversation's latest response.",
        "previous_response_id_conflict",
    );
    if let Some(object) = body.as_object_mut() {
        object.insert("field".into(), "previous_response_id".into());
    }
    problem_response(StatusCode::CONFLICT, body)
}

pub(crate) fn response_in_progress() -> Response {
    let body = problem_document(
        StatusCode::CONFLICT,
        "response-in-progress",
        "This conversation already has a response in progress.",
        "response_in_progress",
    );
    problem_response(StatusCode::CONFLICT, body)
}

use axum::response::IntoResponse as _;
