//! Only an original event and an issued continuation cursor may select history.
use super::tools::{event_ref, result};
use crate::{auth::AgentCaller, tools::ToolsState};
use axum::{
    extract::{Path, Query, State},
    response::Response,
    routing::get,
    Router,
};
use cmaeg_core::{
    conversation_history::ConversationHistoryPage,
    problem::{Problem, ProblemCode},
    seal::HistoryCursor,
};
use serde::Deserialize;

pub(super) fn routes() -> Router<ToolsState> {
    Router::new().route("/v1/events/{event_ref}/history", get(read_event_history))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    cursor: Option<HistoryCursor>,
    #[serde(default)]
    include_media: bool,
}

#[utoipa::path(get,path="/v1/events/{event_ref}/history",operation_id="readEventHistory",tag="tools",
    security(("agentBearer"=[])),params(("event_ref"=String,Path),("cursor"=Option<HistoryCursor>,Query),
        ("include_media"=Option<bool>,Query,description="Opt in to text from supported mixed Lark/Feishu posts and the media omission enum. Default false; requires the mixed-history gate and source consent. Repeat the same value for every cursor page. No media downloads or file authority.")),
    responses((status=200,body=ConversationHistoryPage),(status="default",body=Problem,content_type="application/problem+json")))]
pub async fn read_event_history(
    State(state): State<ToolsState>,
    caller: AgentCaller,
    Path(raw): Path<String>,
    query: Result<Query<HistoryQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    result(
        async {
            let Query(query) = query.map_err(|_| {
                Problem::new(ProblemCode::InvalidRequest, 400, "invalid history query")
            })?;
            state
                .read_event_history_with_media(caller, &event_ref(&raw)?, query.cursor.as_ref(), query.include_media)
                .await
        }
        .await,
    )
}
