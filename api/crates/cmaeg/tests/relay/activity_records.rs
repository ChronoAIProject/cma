//! Opaque activity persistence for the real binary's HTTP-backed relay fixture.
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone)]
struct Records {
    rows: Arc<Mutex<BTreeMap<String, Stored>>>,
    status: Arc<AtomicUsize>,
    lose_response: Arc<AtomicBool>,
}
struct Stored {
    owner: String,
    blob: String,
    version: u64,
    expires_at: i64,
}
#[derive(Deserialize)]
struct Put {
    owner: String,
    blob: String,
    expected_version: Option<u64>,
    expires_at: i64,
}

pub fn router(status: Arc<AtomicUsize>, lose_response: Arc<AtomicBool>) -> Router {
    Router::new()
        .route(
            "/api/v1/gateway/interaction-records/{id}",
            get(read).put(write),
        )
        .with_state(Records {
            rows: Arc::default(),
            status,
            lose_response,
        })
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                if request
                    .headers()
                    .get("authorization")
                    .is_none_or(|value| value != "Bearer test-service-bearer")
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                next.run(request).await
            },
        ))
}

async fn read(State(records): State<Records>, Path(id): Path<String>) -> Response {
    let records = records.rows.lock().unwrap();
    let Some(row) = records
        .get(&id)
        .filter(|row| row.expires_at > time::OffsetDateTime::now_utc().unix_timestamp())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(
        json!({"record_id": id, "owner": row.owner, "blob": row.blob,
        "version": row.version}),
    )
    .into_response()
}

async fn write(
    State(records): State<Records>,
    Path(id): Path<String>,
    Json(body): Json<Put>,
) -> Response {
    let status = records.status.load(Ordering::SeqCst) as u16;
    if status == 504 {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    }
    if status != 200 {
        return StatusCode::from_u16(status).unwrap().into_response();
    }
    let lose_response = records.lose_response.load(Ordering::SeqCst);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if body.owner.is_empty()
        || body.expires_at <= now
        || body.expires_at > now + 7 * 86400
        || !STANDARD
            .decode(&body.blob)
            .is_ok_and(|blob| !blob.is_empty() && blob.len() <= 65_536)
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut records = records.rows.lock().unwrap();
    let existing = records.get(&id).filter(|row| row.expires_at > now);
    if existing.map(|row| row.version) != body.expected_version
        || existing.is_some_and(|row| row.owner != body.owner)
    {
        return (
            StatusCode::CONFLICT,
            Json(json!({"actual": existing.map_or(0, |row| row.version)})),
        )
            .into_response();
    }
    let version = existing.map_or(1, |row| row.version + 1);
    records.insert(
        id.clone(),
        Stored {
            owner: body.owner,
            blob: body.blob,
            version,
            expires_at: body.expires_at,
        },
    );
    if lose_response {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(json!({"record_id": id, "version": version})).into_response()
}
