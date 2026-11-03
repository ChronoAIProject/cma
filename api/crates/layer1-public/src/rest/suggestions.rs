use axum::{extract::{Path, Query, State}, Json};
use layer2_core::{AgentId, sandbox::{agent_io::AnswerContext, suggestions::SuggestionReading}};

use super::{agents::AgentRoutesState, instance_problem::InstanceProblem};
use crate::AuthenticatedCaller;

pub(crate) async fn read(
    State(state): State<AgentRoutesState>, AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>, query: Result<Query<AnswerContext>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<SuggestionReading>, InstanceProblem> {
    let scope = query.map_err(|_| InstanceProblem::InvalidQuery)?.0;
    let service = state.suggestions.ok_or(InstanceProblem::Unavailable)?;
    Ok(Json(service.read(&caller, &AgentId(id), scope).await?))
}
