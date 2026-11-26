//! Stateful OpenAI Responses subset over owned Agent conversations.

use crate::conversation::failure;
use crate::{InterfaceProfile, Operation};
use axum::{
    http::StatusCode,
    response::Response,
    routing::{get, post},
    Router,
};

pub mod v2;
pub use retrieve::parse_response_id;

mod conversations;
mod create;
#[cfg(test)]
mod credit_failure_tests;
mod items;
mod model;
#[cfg(test)]
mod output_compat_tests;
mod parse;
#[cfg(test)]
mod provider_failure_tests;
mod retrieve;
mod stream;
#[cfg(test)]
mod tests;
mod types;

pub use stream::ResponsesConnectionEvent;
pub use types::*;

pub(crate) fn document() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi;
    #[derive(OpenApi)]
    #[openapi(
        paths(
            conversations::create,
            conversations::retrieve,
            create::create,
            retrieve::retrieve,
            retrieve::delete,
            retrieve::cancel,
            items::list
        ),
        components(schemas(Conversation, ResponseObject))
    )]
    struct Api;
    let mut document = Api::openapi();
    document.merge(v2::document());
    document
}

pub fn router(state: crate::AgentConversationState) -> Router {
    Router::new()
        .route("/api/v1/conversations", post(conversations::create))
        .route("/api/v1/conversations/{id}", get(conversations::retrieve))
        .route("/api/v1/conversations/{id}/items", get(items::list))
        .route("/api/v1/responses", post(create::create))
        .route(
            "/api/v1/responses/{response_id}",
            get(retrieve::retrieve).delete(retrieve::delete),
        )
        .route(
            "/api/v1/responses/{response_id}/cancel",
            post(retrieve::cancel),
        )
        .with_state(state)
}

pub struct ResponsesInterface;

impl InterfaceProfile for ResponsesInterface {
    fn interface_id(&self) -> &'static str {
        "responses"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::ManageAgents,
            Operation::SendAgentInput,
            Operation::StreamAgentEvents,
            Operation::ReadAgentUsage,
        ]
    }
}

use response_not_found as not_found;

pub fn response_not_found() -> Response {
    failure(
        StatusCode::NOT_FOUND,
        "response_not_found",
        "No response matches this request.",
    )
}

fn field_refusal(field: &str, detail: &str) -> Response {
    let mut body = crate::rest::problem_document(
        StatusCode::UNPROCESSABLE_ENTITY,
        "responses-field-unsupported",
        detail,
        "responses_field_unsupported",
    );
    body["field"] = field.into();
    crate::rest::problem_response(StatusCode::UNPROCESSABLE_ENTITY, body)
}

pub(crate) fn previous_conflict() -> Response {
    let mut body = crate::rest::problem_document(
        StatusCode::CONFLICT,
        "previous-response-conflict",
        "previous_response_id is not the conversation's latest response.",
        "previous_response_id_conflict",
    );
    body["field"] = "previous_response_id".into();
    crate::rest::problem_response(StatusCode::CONFLICT, body)
}
