//! Owner-authorized retained supplementary response discovery. The returned
//! Manager URLs retain their independent response authorization.
use super::{
    activity::owned,
    management_responses::{management_error, parse_channel_id},
    problems::{no_store, problem},
};
use crate::{
    auth::Creator,
    channels::management::ManagementState,
    ingress::routing_store::{EventIdentity, PlanStore},
    turns::routed::{ExecutionError, Journal, MediaObligationStore, MediaResponsePage},
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use cmaeg_core::{ids::EventId, problem::ProblemCode};
use serde::Deserialize;
use utoipa::IntoParams;
#[derive(Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct MediaResponsesQuery {
    pub cursor: Option<usize>,
}
#[utoipa::path(get,path="/v1/channels/{channel_id}/events/{event_id}/media-responses",operation_id="getEventMediaResponses",tag="channels",
 description="Owner-authorized retained media continuation metadata. Returns at most 16 attachment slots per page with independent response identities and terminal delivery statuses. No response text, attachment paths, credentials or wider Agent conversation visibility are returned. Each Agent Manager response URL still requires its normal authorization.",
 security(("creatorBearer"=[])),params(MediaResponsesQuery,("channel_id"=String,Path),("event_id"=String,Path),("X-CMAEG-Owner"=Option<String>,Header)),
 responses((status=200,body=MediaResponsePage),(status="default",body=cmaeg_core::problem::Problem,content_type="application/problem+json")))]
pub async fn list(
    State(state): State<ManagementState>,
    creator: Creator,
    Path((raw, event)): Path<(String, String)>,
    Query(query): Query<MediaResponsesQuery>,
) -> Response {
    let id = match parse_channel_id(&raw) {
        Ok(id) => id,
        Err(e) => return management_error(e),
    };
    let event = match EventId::new(event) {
        Ok(id) => id,
        Err(_) => {
            return problem(
                StatusCode::BAD_REQUEST,
                ProblemCode::InvalidRequest,
                "invalid event identifier",
            )
        }
    };
    let channel = match owned(&state, &creator, &id).await {
        Ok(c) => c,
        Err(e) => return management_error(e),
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let plans = PlanStore::new((*state.records).clone(), state.master_keys.clone());
        let identity = EventIdentity {
            owner: creator.subject,
            channel_id: id,
            record_id: channel.definition.record_id,
            event_id: event,
        };
        let Some(plan) = plans
            .load(&identity)
            .await
            .map_err(|_| ExecutionError::Unavailable)?
        else {
            return Ok(None);
        };
        let store = MediaObligationStore(Journal::new(
            (*state.records).clone(),
            state.master_keys.clone(),
        ));
        store
            .retained_responses(&plan, query.cursor.unwrap_or(0))
            .await
            .map(Some)
    })
    .await;
    match result {
        Ok(Ok(Some(page))) => no_store(Json(page).into_response()),
        Ok(Ok(None)) => problem(
            StatusCode::NOT_FOUND,
            ProblemCode::InvalidRequest,
            "retained event not found",
        ),
        Ok(Err(ExecutionError::Invalid)) => problem(
            StatusCode::BAD_REQUEST,
            ProblemCode::InvalidRequest,
            "invalid media response cursor",
        ),
        _ => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            ProblemCode::Internal,
            "media response metadata unavailable",
        ),
    }
}
