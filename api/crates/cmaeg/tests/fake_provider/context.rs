use super::*;

pub(super) async fn event_context(
    State(state): State<Arc<StateData>>,
    Path((binding, conversation, event)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    state.requests.lock().unwrap().push(received(
        "GET",
        format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/events/{event}"),
        &headers,
        &[],
    ));
    let attempt = state.context_attempt.fetch_add(1, Ordering::SeqCst);
    let (delay, status, retry_after) =
        state.config.context_steps.get(attempt).cloned().unwrap_or((
            state.config.context_delay,
            state.config.context_status,
            None,
        ));
    sleep(delay).await;
    if let Some(status) = status {
        let mut response = StatusCode::from_u16(status).unwrap().into_response();
        if let Some(value) = retry_after {
            response
                .headers_mut()
                .insert("retry-after", value.parse().unwrap());
        }
        return response;
    }
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    let snapshot = state
        .admitted
        .lock()
        .unwrap()
        .iter()
        .filter(|request| {
            request.method == "POST" && request.authorization.as_deref() == authorization
        })
        .find_map(|request| {
            let value: Value = serde_json::from_str(&request.body).ok()?;
            let context = value.get("event_context")?;
            (context["conversation_id"] == conversation && context["activity"]["event_id"] == event)
                .then(|| context.clone())
        });
    match snapshot {
        Some(mut value) => {
            if let Some(patch) = &state.config.context_override {
                for (key, replacement) in patch.as_object().unwrap() {
                    value[key] = replacement.clone();
                }
            }
            Json(value).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
