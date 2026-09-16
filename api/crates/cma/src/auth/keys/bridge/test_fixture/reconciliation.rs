use super::*;
use axum::extract::Query;
use std::collections::BTreeMap;

pub(super) async fn list(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers)?;
    state.list_count.fetch_add(1, Ordering::SeqCst);
    match &state.owner {
        ResourceOwner::Person { .. } => assert!(!query.contains_key("org_id")),
        ResourceOwner::Organization { subject } => assert_eq!(
            query.get("org_id").map(String::as_str),
            Some(subject.as_str())
        ),
    }
    if let Some(value) = state.listing_override.lock().unwrap().clone() {
        return Ok(Json(value));
    }
    let keys = if state.revoked.load(Ordering::SeqCst) {
        vec![]
    } else {
        state.create_body.lock().unwrap().as_ref().map(|body| json!({"id":"key-created","name":body["name"],"created_at":layer2_core::rfc3339_at(SystemTime::now())})).into_iter().collect()
    };
    Ok(Json(json!({"keys":keys})))
}

pub(super) async fn revoke(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    authorized(&headers)?;
    if state.revoked.swap(true, Ordering::SeqCst) {
        return Err(StatusCode::NOT_FOUND);
    }
    state.revoke_count.fetch_add(1, Ordering::SeqCst);
    if state.delete_unknown.load(Ordering::SeqCst) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(StatusCode::NO_CONTENT)
}
