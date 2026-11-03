use super::suggestion_shapes::SuggestionReadingView;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use layer2_core::{sandbox::agent_io::AnswerContext, AgentId};

use super::{agents::AgentRoutesState, instance_problem::InstanceProblem};
use crate::AuthenticatedCaller;

#[utoipa::path(get, operation_id = "read_suggestions", path = "/api/v1/agents/{id}/suggestions", tag = "agents",
    params(("id" = String, Path), ("turn_sequence" = u64, Query), ("answer_sequence" = u64, Query)),
    responses((status = 200, body = SuggestionReadingView), (status = 403, body = crate::rest::ProblemDocument),
        (status = 404, body = crate::rest::ProblemDocument), (status = 503, body = crate::rest::ProblemDocument)))]
pub(crate) async fn read(
    State(state): State<AgentRoutesState>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    query: Result<Query<AnswerContext>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<SuggestionReadingView>, InstanceProblem> {
    let scope = query.map_err(|_| InstanceProblem::InvalidQuery)?.0;
    let service = state.suggestions.ok_or(InstanceProblem::Unavailable)?;
    Ok(Json(
        service.read(&caller, &AgentId(id), scope).await?.into(),
    ))
}
