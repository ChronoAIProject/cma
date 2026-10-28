//! Optional persistent-domain fixture; ordinary interaction expiry remains strict.
use super::*;
#[derive(Deserialize)]
pub(super) struct Put {
    owner: String,
    blob: String,
    expected_version: Option<u64>,
    expires_at: Option<i64>,
}
pub(super) async fn write(
    State(state): State<Arc<Memory>>,
    Path(id): Path<String>,
    Json(input): Json<Put>,
) -> Response {
    let Ok(blob) = STANDARD.decode(input.blob) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if blob.len() > 65536 || input.expires_at.is_some() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let fault = {
        let mut faults = state.fault.lock().unwrap();
        if faults.as_ref().is_some_and(|f| f.id == id && f.skip > 0) {
            faults.as_mut().unwrap().skip -= 1;
            None
        } else if faults.as_ref().is_some_and(|f| f.id == id) {
            faults.take()
        } else {
            None
        }
    };
    if fault.as_ref().is_some_and(|f| !f.store) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let version = {
        let mut rows = state.flows.lock().unwrap();
        let actual = rows.get(&id).map(|r| r.version);
        if actual != input.expected_version {
            return (
                StatusCode::CONFLICT,
                Json(json!({"actual":actual.unwrap_or(0)})),
            )
                .into_response();
        }
        let version = actual.unwrap_or(0) + 1;
        rows.insert(
            id,
            Row {
                owner: input.owner,
                blob,
                expiry: i64::MAX,
                version,
            },
        );
        version
    };
    state.writes.fetch_add(1, Ordering::SeqCst);
    if let Some(f) = fault {
        if f.lose_read {
            state.unavailable_reads.store(1, Ordering::SeqCst);
        }
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        Json(json!({"version":version})).into_response()
    }
}
pub(super) async fn read(State(state): State<Arc<Memory>>, Path(id): Path<String>) -> Response {
    if state
        .unavailable_reads
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_sub(1))
        .is_ok()
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    match state.flows.lock().unwrap().get(&id) {
        Some(r) => {
            Json(json!({"version":r.version,"blob":STANDARD.encode(&r.blob)})).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
pub(super) async fn list(State(state): State<Arc<Memory>>, Query(query): Query<List>) -> Response {
    let rows = state.flows.lock().unwrap();
    let items: Vec<_> = rows
        .iter()
        .filter(|(id, row)| {
            query.owner.as_ref().is_none_or(|o| o == &row.owner)
                && query.after.as_ref().is_none_or(|a| *id > a)
        })
        .take(query.limit)
        .map(|(id, row)| json!({"record_id":id,"owner":row.owner,"version":row.version}))
        .collect();
    Json(json!({"items":items})).into_response()
}
