use super::*;
use crate::rest::agent_retained_output::{RetainedOutputPageView, RetainedOutputQuery};

#[utoipa::path(get, path = "/api/v2/responses/{response_id}/retained-output", tag = "responses v2",
    params(("response_id" = String, Path), ("cursor" = Option<String>, Query)),
    responses((status = 200, body = RetainedOutputPageView), (status = 404, body = Value)))]
pub(in crate::responses::v2) async fn read(
    State(state): State<ResponsesV2State>,
    auth: Auth,
    Path(id): Path<String>,
    query: Result<Query<RetainedOutputQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Ok(auth) = auth else {
        return error::unauthenticated();
    };
    let Ok(Query(query)) = query else {
        return error::field(
            "cursor",
            "invalid_retained_output_cursor",
            "Use the returned retained-output cursor.",
        );
    };
    let (authority, status) = match authorize(&state, &auth, &AgentResponseId(id)).await {
        Ok(value) => value,
        Err(value) => return *value,
    };
    let page = match status.turn_sequence {
        Some(turn) => {
            state
                .reads
                .retained_output_for_stream(&authority, Some(turn), query.cursor.as_deref())
                .await
        }
        None => state
            .reads
            .validate_event_stream(&authority)
            .await
            .and_then(|()| {
                if query.cursor.is_some() {
                    Err(layer2_core::sandbox::AgentReadError::InvalidPage)
                } else {
                    Ok(Default::default())
                }
            }),
    };
    match page {
        Ok(page) => crate::rest::agent_retained_output::response(page),
        Err(layer2_core::sandbox::AgentReadError::InvalidPage) => error::field(
            "cursor",
            "invalid_retained_output_cursor",
            "Use a cursor for this response's retained output.",
        ),
        Err(value) => error::read(value),
    }
}
