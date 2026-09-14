//! The Agent milestone has no terminal capability. Attachment tickets stay unrouted.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::get,
    Router,
};
use layer2_core::{sandbox::AgentReadService, AgentId};

use crate::{conversation::read_failure, AuthenticatedCaller};

pub fn router(reads: Arc<AgentReadService>) -> Router {
    Router::new()
        .route("/api/v1/agents/{id}/terminal", get(terminal))
        .with_state(reads)
}

#[utoipa::path(get, path = "/api/v1/agents/{id}/terminal", tag = "agents",
    params(("id" = String, Path)), responses((status = 501, body = crate::rest::ProblemDocument),
    (status = 404, body = crate::rest::ProblemDocument)))]
pub(crate) async fn terminal(
    State(reads): State<Arc<AgentReadService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Response {
    if let Err(error) = reads.read(&caller, &AgentId(id)).await {
        return read_failure(error, "agent_not_found");
    }
    let mut body = crate::rest::problem_document(
        StatusCode::NOT_IMPLEMENTED,
        "capability-unsupported",
        "Agent terminal attachment is unavailable in this milestone.",
        "capability_unsupported",
    );
    body["capability"] = "terminal".into();
    crate::rest::problem_response(StatusCode::NOT_IMPLEMENTED, body)
}

#[cfg(test)]
mod tests {
    use crate::rest::agents_tests::{
        conversation_fixture, insert_agent, json_response, request, seeded_agent,
    };
    use axum::http::{Method, StatusCode};
    use layer2_core::sandbox::AgentStatus;

    #[tokio::test]
    async fn owned_agents_receive_typed_501_without_attachment_tickets() {
        let (state, store) = conversation_fixture().await;
        insert_agent(
            &store,
            seeded_agent("agt_terminal", AgentStatus::Ready, 2, None),
            vec![],
        )
        .await;
        let router = super::router(state.reads);
        let (status, body) = json_response(
            &router,
            request(
                Method::GET,
                "/api/v1/agents/agt_terminal/terminal",
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(body["code"], "capability_unsupported");
        assert_eq!(body["capability"], "terminal");
        let (status, _) = json_response(
            &router,
            request(
                Method::GET,
                "/api/v1/agents/agt_terminal/terminal",
                "owner-b",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
