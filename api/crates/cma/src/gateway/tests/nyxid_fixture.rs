use super::*;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Default)]
pub(super) struct IdentityState {
    pub keys: BTreeMap<String, Value>,
    pub routes: BTreeMap<String, Value>,
    pub calls: Vec<String>,
    pub fail_after: Option<&'static str>,
    pub expiry_suffix: Option<&'static str>,
    pub routes_total: Option<usize>,
}
pub(super) type Identity = Arc<Mutex<IdentityState>>;
pub(super) fn routes(state: Identity) -> Router {
    Router::new()
        .route(
            "/api/v1/user-services",
            get(|| async {
                Json(json!({"services":[
                {"id":"model-service","slug":"chrono-llm-public","is_active":true,"credential_source":{"type":"personal"}},
                {"id":"skills-service","slug":"ornn-api","is_active":true,"credential_source":{"type":"personal"}}]}))
            }),
        )
        .route("/api/v1/channel-bots/{id}", get(bot))
        .route("/api/v1/channel-bots", get(|| async { Json(json!({"bots":[{"id":"external-bot","user_id":"owner","label":"Telegram research","platform":"telegram","platform_bot_username":"fixture_bot","is_active":true}],"total":1})) }))
        .route("/api/v1/api-keys/scope-plan", post(|Json(body): Json<Value>| async move {
            Json(crate::auth::keys::scope_plan_fixture("owner", &body, &[]))
        }))
        .route("/api/v1/api-keys", get(keys).post(mint))
        .route(
            "/api/v1/api-keys/{id}",
            get(key).put(callback).delete(revoke),
        )
        .route(
            "/api/v1/channel-conversations",
            get(routes_list).post(route),
        )
        .route(
            "/api/v1/channel-conversations/{id}",
            post(route_unused).put(route_key).delete(delete_route),
        )
        .with_state(state)
}
async fn bot(Path(id): Path<String>) -> Json<Value> {
    Json(
        json!({"id":id,"user_id":"owner","label":"fixture bot","platform":"telegram","platform_bot_username":"fixture_bot","is_active":true}),
    )
}
async fn keys(State(state): State<Identity>) -> Json<Value> {
    Json(json!({"keys":state.lock().unwrap().keys.values().cloned().collect::<Vec<_>>()}))
}
async fn key(State(state): State<Identity>, Path(id): Path<String>) -> Response {
    state
        .lock()
        .unwrap()
        .keys
        .get(&id)
        .cloned()
        .map(|k| Json(k).into_response())
        .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}
async fn mint(State(state): State<Identity>, Json(mut body): Json<Value>) -> Response {
    // API-key platform identifies the client; it is not the channel transport.
    if body["platform"].as_str().is_some_and(|platform| {
        ![
            "nyxid-assistant",
            "claude-code",
            "cursor",
            "codex",
            "openclaw",
            "generic",
            "device-code",
            "device-onboard",
        ]
        .contains(&platform)
    }) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    assert_eq!(body["allow_all_services"], false);
    assert_eq!(body["allow_all_nodes"], false);
    assert_eq!(body["allowed_node_ids"], json!([]));
    assert_eq!(body["scopes"], "proxy read");
    let role = if body["name"].as_str().unwrap().contains(":agent:") {
        "mint_agent"
    } else {
        "mint_route"
    };
    if role == "mint_agent" {
        assert_eq!(
            body["allowed_service_ids"],
            json!(["model-service", "skills-service"])
        );
    } else {
        assert_eq!(body["allowed_service_ids"], json!([]));
        assert_eq!(body["platform"], "generic");
        assert!(body["callback_url"]
            .as_str()
            .unwrap()
            .ends_with("/v1/callbacks/pending"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    body["id"] = id.clone().into();
    body["state_version"] = 1.into();
    body["is_active"] = true.into();
    body["credential_source"] = json!({"type":"personal"});
    body["purpose"] = "general".into();
    body["scheduled_write_enabled"] = false.into();
    let mut state = state.lock().unwrap();
    // NyxID serializes UTC metadata with an offset, even when the request uses Z.
    body["expires_at"] = body["expires_at"]
        .as_str()
        .unwrap()
        .replace('Z', state.expiry_suffix.unwrap_or("+00:00"))
        .into();
    state.keys.insert(id, body.clone());
    body["full_key"] = format!("nyx_{}", uuid::Uuid::new_v4().simple()).into();
    effect(&mut state, role, body)
}
async fn callback(
    State(state): State<Identity>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut state = state.lock().unwrap();
    let key = state.keys.get_mut(&id).unwrap();
    key["callback_url"] = body["callback_url"].clone();
    let key = key.clone();
    effect(&mut state, "callback", key)
}
async fn revoke(State(state): State<Identity>, Path(id): Path<String>) -> Response {
    let mut state = state.lock().unwrap();
    assert!(state.routes.values().all(|r| r["agent_api_key_id"] != id));
    state.keys.remove(&id);
    effect(&mut state, "revoke", json!({"deleted":true}))
}
async fn routes_list(State(state): State<Identity>) -> Json<Value> {
    let state = state.lock().unwrap();
    Json(
        json!({"conversations":state.routes.values().cloned().collect::<Vec<_>>(),"total":state.routes_total.unwrap_or(state.routes.len())}),
    )
}
async fn route(State(state): State<Identity>, Json(mut body): Json<Value>) -> Response {
    assert_eq!(body["platform_conversation_id"], "*");
    assert_eq!(body["default_agent"], true);
    let mut state = state.lock().unwrap();
    let key = &state.keys[body["agent_api_key_id"].as_str().unwrap()];
    assert!(!key["callback_url"].as_str().unwrap().ends_with("/pending"));
    let id = uuid::Uuid::new_v4().to_string();
    body["id"] = id.clone().into();
    body["platform"] = "telegram".into();
    body["is_active"] = true.into();
    state.routes.insert(id, body.clone());
    effect(&mut state, "create_route", body)
}
async fn route_key(
    State(state): State<Identity>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let mut state = state.lock().unwrap();
    let route = state.routes.get_mut(&id).unwrap();
    route["agent_api_key_id"] = body["agent_api_key_id"].clone();
    let route = route.clone();
    effect(&mut state, "route_key", route)
}
async fn delete_route(State(state): State<Identity>, Path(id): Path<String>) -> Response {
    let mut state = state.lock().unwrap();
    state.routes.remove(&id);
    effect(&mut state, "delete_route", json!({"deleted":true}))
}
async fn route_unused() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}
fn effect(state: &mut IdentityState, name: &'static str, response: Value) -> Response {
    state.calls.push(name.into());
    if state.fail_after == Some(name) {
        state.fail_after = None;
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    } else {
        Json(response).into_response()
    }
}
