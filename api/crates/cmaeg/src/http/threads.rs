//! Thread lifecycle receives only an original event or a scoped sealed thread reference.
use super::tools::{event_ref, json, result};
use crate::{auth::AgentCaller, tools::ToolsState};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use cmaeg_core::{
    problem::{Problem, ProblemCode},
    thread_lifecycle::{
        CreateThreadRequest, ManageThreadRequest, ThreadList, ThreadListView, ThreadReceipt,
    },
};
use serde::Deserialize;

pub(super) fn routes() -> Router<ToolsState> {
    Router::new()
        .route(
            "/v1/events/{event_ref}/threads",
            get(list_event_threads).post(create_event_thread),
        )
        .route("/v1/thread-operations", post(manage_thread))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    #[serde(default)]
    view: ThreadListView,
}

#[utoipa::path(get,path="/v1/events/{event_ref}/threads",operation_id="listEventThreads",tag="tools",
    security(("agentBearer"=[])),params(("event_ref"=String,Path),("view"=Option<ThreadListView>,Query)),
    responses((status=200,body=ThreadList),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn list_event_threads(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(raw): Path<String>,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    result(
        async {
            let Query(query) = query.map_err(|_| {
                Problem::new(
                    ProblemCode::InvalidRequest,
                    400,
                    "invalid thread list query",
                )
            })?;
            state
                .list_event_threads(caller, &event_ref(&raw)?, query.view)
                .await
        }
        .await,
    )
}
#[utoipa::path(post,path="/v1/events/{event_ref}/threads",operation_id="createEventThread",tag="tools",
    security(("agentBearer"=[])),params(("event_ref"=String,Path),("Idempotency-Key"=String,Header)),request_body=CreateThreadRequest,
    responses((status=200,body=ThreadReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn create_event_thread(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Result<Json<CreateThreadRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    result(
        async {
            state
                .create_event_thread(caller, &event_ref(&raw)?, json(body)?, key(&headers)?)
                .await
        }
        .await,
    )
}
#[utoipa::path(post,path="/v1/thread-operations",operation_id="manageThread",tag="tools",
    security(("agentBearer"=[])),params(("Idempotency-Key"=String,Header)),request_body=ManageThreadRequest,
    responses((status=200,body=ThreadReceipt),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn manage_thread(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    headers: HeaderMap,
    body: Result<Json<ManageThreadRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    result(
        async {
            state
                .manage_thread(caller, json(body)?, key(&headers)?)
                .await
        }
        .await,
    )
}
fn key(headers: &HeaderMap) -> Result<&str, Problem> {
    let invalid = || {
        Problem::new(
            ProblemCode::InvalidRequest,
            400,
            "exactly one Idempotency-Key header is required",
        )
    };
    let mut values = headers.get_all("idempotency-key").iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(invalid)?;
    if values.next().is_some() {
        return Err(invalid());
    }
    Ok(value)
}
