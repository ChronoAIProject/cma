//! The native connection fixture's channel deletion endpoint uses real CAS.
use super::*;

pub(super) async fn delete(
    State(peer): State<Arc<Peer>>,
    Path(id): Path<String>,
    Query(query): Query<BTreeMap<String, u64>>,
    headers: HeaderMap,
) -> Response {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut rows = peer.channels.lock().unwrap();
    let Some(current) = rows.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if query.get("expected_version") != Some(&current.version) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual": current.version})),
        )
            .into_response();
    }
    rows.remove(&id);
    StatusCode::NO_CONTENT.into_response()
}
