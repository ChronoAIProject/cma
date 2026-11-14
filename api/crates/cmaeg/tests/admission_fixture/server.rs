use super::{ledger::hash, protocol, runner, signer::Signer, Shared};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use subtle::ConstantTimeEq;

pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/jwks", get(|| async { Json(Signer::shared().jwks()) }))
        .route("/readyz", get(|| async { StatusCode::OK }))
        .fallback(handle)
        .with_state(shared)
}
fn authorized(request: &Request, expected: &str) -> bool {
    let Some(supplied) = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    hash(supplied.as_bytes())
        .as_bytes()
        .ct_eq(hash(expected.as_bytes()).as_bytes())
        .into()
}
fn problem(status: StatusCode, code: &'static str) -> Response {
    (status, Json(json!({"code":code}))).into_response()
}
async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    if path.starts_with("/control/") {
        if !authorized(&request, &shared.config.control_bearer) {
            return problem(StatusCode::UNAUTHORIZED, "fixture_auth");
        }
        if method == axum::http::Method::GET && path == "/control/ledger" {
            return Json(shared.ledger.lock().expect("fixture ledger").snapshot()).into_response();
        }
        if method == axum::http::Method::POST && path == "/control/close" {
            let Ok(_exclusive) = shared.run_lock.try_lock() else {
                return problem(StatusCode::CONFLICT, "case_active");
            };
            let mut ledger = shared.ledger.lock().expect("fixture ledger");
            if ledger.close().is_err() {
                return problem(StatusCode::SERVICE_UNAVAILABLE, "ledger_persistence");
            }
            return Json(ledger.snapshot()).into_response();
        }
        if method == axum::http::Method::POST && path == "/control/reconcile" {
            return match super::reconcile::observe(shared).await {
                Ok(value) => Json(value).into_response(),
                Err(code) => problem(StatusCode::CONFLICT, code),
            };
        }
        if method == axum::http::Method::POST && path == "/control/run" {
            let Ok(body) = to_bytes(request.into_body(), 16 * 1024).await else {
                return problem(StatusCode::BAD_REQUEST, "control_body");
            };
            let Ok(input) = serde_json::from_slice::<runner::Run>(&body) else {
                return problem(StatusCode::BAD_REQUEST, "control_schema");
            };
            return match runner::run(shared, input).await {
                Ok(value) => Json(value).into_response(),
                Err(code) => problem(StatusCode::CONFLICT, code),
            };
        }
        return problem(StatusCode::NOT_FOUND, "control_route");
    }
    if method == axum::http::Method::POST && path == "/api/v1/channel-relay/reply" {
        let supplied = request
            .headers()
            .get("authorization")
            .and_then(|x| x.to_str().ok())
            .map(str::to_owned);
        let Ok(body) = to_bytes(request.into_body(), 2 * 1024 * 1024).await else {
            return problem(StatusCode::BAD_REQUEST, "sink_body");
        };
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            return problem(StatusCode::BAD_REQUEST, "sink_json");
        };
        let Some(event_id) = value["message_id"].as_str() else {
            return problem(StatusCode::BAD_REQUEST, "sink_event");
        };
        let authenticated = {
            let events = shared.events.lock().expect("fixture events");
            events
                .values()
                .find(|event| event.identity.event_id.as_str() == event_id)
                .and_then(|event| event.signed.as_ref())
                .is_some_and(|signed| {
                    supplied.as_deref().is_some_and(|value| {
                        hash(value.as_bytes())
                            .as_bytes()
                            .ct_eq(hash(format!("Bearer {}", signed.reply).as_bytes()).as_bytes())
                            .into()
                    })
                })
        };
        if !authenticated {
            return problem(StatusCode::UNAUTHORIZED, "sink_auth");
        }
        let Some(text) = value["reply"]["text"].as_str() else {
            return problem(StatusCode::BAD_REQUEST, "sink_text");
        };
        let receipt = shared
            .ledger
            .lock()
            .expect("fixture ledger")
            .effect(event_id, &body, text);
        return match receipt {
            Ok(value) => Json(value).into_response(),
            Err(code) => problem(StatusCode::SERVICE_UNAVAILABLE, code),
        };
    }
    if !authorized(&request, &shared.provider_bearer) {
        return problem(StatusCode::UNAUTHORIZED, "provider_auth");
    }
    if method == axum::http::Method::PUT
        && path.starts_with("/bindings/")
        && path.contains("/conversations/")
    {
        return (StatusCode::CREATED, Json(json!({}))).into_response();
    }
    if method == axum::http::Method::POST && path == "/api/v2/responses" {
        let Some(key) = request
            .headers()
            .get("idempotency-key")
            .and_then(|x| x.to_str().ok())
            .map(str::to_owned)
        else {
            return problem(StatusCode::BAD_REQUEST, "provider_idempotency");
        };
        let Ok(body) = to_bytes(request.into_body(), 2 * 1024 * 1024).await else {
            return problem(StatusCode::BAD_REQUEST, "provider_body");
        };
        let Ok(value) = serde_json::from_slice::<Value>(&body) else {
            return problem(StatusCode::BAD_REQUEST, "provider_json");
        };
        let Some(event_id) = value["event_context"]["activity"]["event_id"].as_str() else {
            return problem(StatusCode::BAD_REQUEST, "provider_event");
        };
        if !shared
            .events
            .lock()
            .expect("fixture events")
            .values()
            .any(|event| event.identity.event_id.as_str() == event_id)
        {
            return problem(StatusCode::FORBIDDEN, "unregistered_event");
        }
        let admitted = shared
            .ledger
            .lock()
            .expect("fixture ledger")
            .admit(&key, event_id, &body, &value);
        return match admitted {
            Ok(row) => stream_response(&row, None, true),
            Err(code) => problem(StatusCode::CONFLICT, code),
        };
    }
    if method == axum::http::Method::GET {
        let Some(rest) = path.strip_prefix("/api/v2/responses/") else {
            return problem(StatusCode::NOT_FOUND, "provider_route");
        };
        let id = rest.strip_suffix("/events").unwrap_or(rest);
        let row = shared
            .ledger
            .lock()
            .expect("fixture ledger")
            .responses
            .values()
            .find(|row| row.response_id == id)
            .cloned();
        let Some(row) = row else {
            return problem(StatusCode::NOT_FOUND, "response_missing");
        };
        if rest.ends_with("/events") {
            let after = match request.uri().query() {
                None => None,
                Some(query) => match query
                    .strip_prefix("starting_after=")
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    Some(value) => Some(value),
                    None => return problem(StatusCode::BAD_REQUEST, "replay_cursor"),
                },
            };
            return stream_response(&row, after, false);
        }
        return Json(protocol::snapshot(&row, "completed")).into_response();
    }
    problem(StatusCode::NOT_FOUND, "fixture_route")
}
fn stream_response(
    row: &super::ledger::ResponseRecord,
    after: Option<u64>,
    admission: bool,
) -> Response {
    Response::builder()
        .header("content-type", "text/event-stream")
        .header("x-cma-response-id", &row.response_id)
        .body(Body::from(protocol::frames(row, after, admission)))
        .expect("fixed fixture response headers")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_rejects_missing_wrong_and_malformed_bearers() {
        let credential = uuid::Uuid::new_v4().to_string();
        let valid = Request::builder()
            .header("authorization", format!("Bearer {credential}"))
            .body(Body::empty())
            .unwrap();
        assert!(authorized(&valid, &credential));
        for header in ["Bearer wrong", "Basic wrong", "Bearer "] {
            let request = Request::builder()
                .header("authorization", header)
                .body(Body::empty())
                .unwrap();
            assert!(!authorized(&request, &credential));
        }
        assert!(!authorized(&Request::new(Body::empty()), &credential));
    }
}
