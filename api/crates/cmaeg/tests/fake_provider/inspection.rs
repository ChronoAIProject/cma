use super::*;

pub(super) async fn inspect_conversation(
    State(state): State<Arc<StateData>>,
    Path((binding, conversation)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    record(
        &state,
        "GET",
        format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}"),
        headers,
        Bytes::new(),
    );
    if let Some(view) = reset::inspection(&state, &binding, &conversation) {
        return Json(view).into_response();
    }
    let Some((agent_id, status)) = &state.config.inspection_agent else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"code":"binding_not_found"})),
        )
            .into_response();
    };
    let mut view = json!({"binding_id":binding, "generation":1,
        "conversation_id":conversation,"agent_id":agent_id,"state":"ready","agent_status":status});
    if let Some(drift) = &state.config.inspection_profile_drift {
        view["profile_drift"] = drift.clone();
    }
    Json(view).into_response()
}
