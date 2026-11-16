use super::*;

pub(super) async fn reset(
    State(state): State<Arc<StateData>>,
    Path((binding, conversation)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    state.requests.lock().unwrap().push(received(
        "POST",
        format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/resets"),
        &headers,
        body.to_string().as_bytes(),
    ));
    if let Some(code) = &state.config.reset_refusal {
        return (StatusCode::CONFLICT, Json(json!({"code":code}))).into_response();
    }
    let key = headers
        .get("idempotency-key")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let mut resets = state.resets.lock().unwrap();
    if let Some((saved, result)) = resets.get(&key) {
        return if *saved == body {
            Json(result.clone()).into_response()
        } else {
            (
                StatusCode::CONFLICT,
                Json(json!({"code":"partition_reset_key_conflict"})),
            )
                .into_response()
        };
    }
    let result = json!({"binding_id":binding,"generation":1,"conversation_id":conversation,"incarnation":1,"previous_agent_id":body["expected_agent_id"],"previous_conversation_id":conversation,"successor_agent_id":"agt_successor","state":"reserved"});
    resets.insert(key, (body, result.clone()));
    if state.config.reset_lost_response {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(result).into_response()
}
