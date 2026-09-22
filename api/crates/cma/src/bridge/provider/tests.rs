use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use axum::{
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use layer2_core::bridge::{BridgePlatform, BridgeSecretRef};
use serde_json::{json, Value};

use super::*;

const SECRET: &str = "fixture-selected-agent-key";
type ProviderReplies = BTreeMap<String, (StatusCode, Value)>;
#[derive(Clone)]
struct FixtureState {
    replies: Arc<Mutex<ProviderReplies>>,
    key_patch_on_probe: Arc<Mutex<Option<Value>>>,
    requests: Arc<Mutex<Vec<(String, String, String)>>>,
    verification_barrier: Arc<Mutex<Option<Arc<tokio::sync::Barrier>>>>,
}

async fn respond(
    State(state): State<FixtureState>,
    request: Request<axum::body::Body>,
) -> impl IntoResponse {
    let path = request.uri().path().to_owned();
    let auth = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    state.requests.lock().unwrap().push((
        request.method().to_string(),
        request.uri().to_string(),
        auth.clone(),
    ));
    if !path.starts_with("/api/v1/channel-relay/")
        && (auth.starts_with("Bearer nyx_") || auth.starts_with("Bearer nyxid_"))
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if path == "/api/v1/users/me/authorization"
        || path.starts_with("/api/v1/channel-relay/messages/")
    {
        let barrier = state.verification_barrier.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            barrier.wait().await;
        }
    }
    let mut replies = state.replies.lock().unwrap();
    let (status, body) = replies
        .get(&path)
        .cloned()
        .unwrap_or((StatusCode::NOT_FOUND, json!({})));
    if path.starts_with("/api/v1/channel-relay/messages/") && status == StatusCode::OK {
        let key = &mut replies.get_mut("/api/v1/api-keys").unwrap().1["keys"][0];
        key["state_version"] = json!(key["state_version"].as_i64().unwrap() + 1);
        if let Some(patch) = state.key_patch_on_probe.lock().unwrap().take() {
            for (field, value) in patch.as_object().unwrap() {
                key[field] = value.clone();
            }
        }
    }
    let mut response = (status, Json(body.clone())).into_response();
    for (field, header) in [("fixture_location", "location"), ("fixture_link", "link")] {
        if let Some(value) = body.get(field).and_then(Value::as_str) {
            response.headers_mut().insert(
                axum::http::HeaderName::from_static(header),
                value.parse().unwrap(),
            );
        }
    }
    response
}

struct Fixture {
    provider: NyxBridgeProvider,
    connector: BridgeConnector,
    state: FixtureState,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn replies(owner: &str, source: Value) -> BTreeMap<String, (StatusCode, Value)> {
    BTreeMap::from([
        (
            "/api/v1/channel-bots".into(),
            (
                StatusCode::OK,
                json!({"bots":[{"id":"bot-one","user_id":owner,"platform":"telegram","is_active":true}],"total":1}),
            ),
        ),
        (
            "/api/v1/api-keys".into(),
            (
                StatusCode::OK,
                json!({"keys":[{"id":"key-one","is_active":true,"state_version":3,"scopes":"proxy read write","allowed_service_ids":["service-one"],"allow_all_services":false,"allow_auto_connected_services":false,"allowed_node_ids":[],"allow_all_nodes":false,"platform":"telegram","callback_url":"https://cma.example/callback","credential_source":source,"purpose":"general","scheduled_write_enabled":false}]}),
            ),
        ),
        (
            "/api/v1/channel-conversations".into(),
            (
                StatusCode::OK,
                json!({"conversations":[{"id":"route-one","channel_bot_id":"bot-one","platform":"telegram","agent_api_key_id":"key-one","is_active":true}],"total":1}),
            ),
        ),
    ])
}
async fn fixture(replies: BTreeMap<String, (StatusCode, Value)>) -> Fixture {
    let state = FixtureState {
        replies: Arc::new(Mutex::new(replies)),
        key_patch_on_probe: Arc::new(Mutex::new(None)),
        requests: Arc::new(Mutex::new(vec![])),
        verification_barrier: Arc::new(Mutex::new(None)),
    };
    let app = Router::new()
        .route("/{*path}", get(respond))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let provider =
        NyxBridgeProvider::with_endpoint(reqwest::Url::parse(&endpoint).unwrap()).unwrap();
    let connector = BridgeConnector {
        issuer: endpoint.clone(),
        base_url: endpoint,
        bot_id: "bot-one".into(),
        platform: BridgePlatform::Telegram,
        route_ids: vec!["route-one".into()],
        key_id: "key-one".into(),
        custody: BridgeSecretRef::new("brsec_00000000000000000000000000000001".into()).unwrap(),
        callback_url: "https://cma.example/callback".into(),
    };
    Fixture {
        provider,
        connector,
        state,
        server,
    }
}
fn personal() -> ResourceOwner {
    ResourceOwner::Person {
        subject: SubjectId::new("person-one"),
    }
}
fn org() -> ResourceOwner {
    ResourceOwner::Organization {
        subject: SubjectId::new("org-one"),
    }
}
async fn inspect(
    f: &Fixture,
    owner: &ResourceOwner,
    credential_subject: &str,
) -> Result<VerifiedProviderBinding, BridgeError> {
    f.provider
        .inspect(
            owner,
            &SubjectId::new(credential_subject),
            &SecretValue::new(SECRET.into()),
            &f.connector,
        )
        .await
}

#[tokio::test]
async fn personal_binding_uses_only_exact_credential_read_requests_and_preserves_key_revision() {
    let f = fixture(replies("person-one", json!({"type":"personal"}))).await;
    let observed = inspect(&f, &personal(), "person-one").await.unwrap();
    assert_eq!(observed.key_state_version, 3);
    assert_eq!(observed.routes.len(), 1);
    assert_eq!(observed.routes[0].owner, personal());
    assert!(observed.deadline <= tokio::time::Instant::now() + Duration::from_secs(60));
    let requests = f.state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|(method, uri, auth)| method == "GET"
        && !uri.contains('?')
        && auth == &format!("Bearer {SECRET}")));
}

#[tokio::test]
async fn org_scope_and_actual_owner_are_independent_of_configuring_admin_identity() {
    let f = fixture(replies(
        "org-one",
        json!({"type":"org","org_id":"org-one","role":"admin","allowed":true}),
    ))
    .await;
    let observed = inspect(&f, &org(), "human-admin").await.unwrap();
    assert_eq!(observed.routes[0].owner, org());
    assert!(f
        .state
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|(_, uri, _)| uri.ends_with("?org_id=org-one") && !uri.contains("bot_id=")));
    // A selected organization key authenticates as the org. NyxID then marks
    // directly owned keys as Personal; the verified subject disambiguates it.
    let delegated = fixture(replies("org-one", json!({"type":"personal"}))).await;
    assert!(inspect(&delegated, &org(), "org-one").await.is_ok());
    assert!(inspect(&delegated, &org(), "human-admin").await.is_err());
}

#[tokio::test]
async fn org_member_unknown_role_different_org_and_wrong_bot_owner_are_rejected() {
    for source in [
        json!({"type":"org","org_id":"org-one","role":"member","allowed":true}),
        json!({"type":"org","org_id":"org-one","role":"unknown","allowed":true}),
        json!({"type":"org","org_id":"org-two","role":"admin","allowed":true}),
        json!({"type":"org","org_id":"org-one","role":"admin","allowed":false}),
    ] {
        let f = fixture(replies("org-one", source)).await;
        assert!(inspect(&f, &org(), "human-admin").await.is_err());
    }
    let f = fixture(replies("another-person", json!({"type":"personal"}))).await;
    assert!(inspect(&f, &personal(), "person-one").await.is_err());
}

#[tokio::test]
async fn mismatched_selected_route_key_bot_platform_and_disabled_key_fail() {
    for (endpoint, field, value) in [
        (
            "/api/v1/channel-conversations",
            "agent_api_key_id",
            json!("another-key"),
        ),
        (
            "/api/v1/channel-conversations",
            "channel_bot_id",
            json!("another-bot"),
        ),
        (
            "/api/v1/channel-conversations",
            "platform",
            json!("discord"),
        ),
        ("/api/v1/channel-conversations", "is_active", json!(false)),
        ("/api/v1/api-keys", "is_active", json!(false)),
        ("/api/v1/api-keys", "state_version", json!(0)),
        (
            "/api/v1/api-keys",
            "expires_at",
            json!("2000-01-01T00:00:00Z"),
        ),
        ("/api/v1/api-keys", "platform", json!("discord")),
    ] {
        let mut rows = replies("person-one", json!({"type":"personal"}));
        let collection = if endpoint.ends_with("api-keys") {
            "keys"
        } else {
            "conversations"
        };
        rows.get_mut(endpoint).unwrap().1[collection][0][field] = value;
        let f = fixture(rows).await;
        assert!(inspect(&f, &personal(), "person-one").await.is_err());
    }
}

#[tokio::test]
async fn existing_callback_and_all_other_active_routes_are_checked_without_overwrite() {
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.get_mut("/api/v1/api-keys").unwrap().1["keys"][0]["callback_url"] =
        json!("https://other.example/callback");
    let f = fixture(rows).await;
    assert!(matches!(
        inspect(&f, &personal(), "person-one").await,
        Err(BridgeError::ConnectionRequired(
            BridgeSetupBlock::CallbackConflict
        ))
    ));
    for active in [true, false] {
        let mut rows = replies("person-one", json!({"type":"personal"}));
        let routes = &mut rows.get_mut("/api/v1/channel-conversations").unwrap().1;
        routes["conversations"].as_array_mut().unwrap().push(json!({"id":"other-route","channel_bot_id":"other-bot","platform":"discord","agent_api_key_id":"key-one","is_active":active}));
        routes["total"] = json!(2);
        let f = fixture(rows).await;
        assert_eq!(
            inspect(&f, &personal(), "person-one").await.is_ok(),
            !active
        );
        assert!(f
            .state
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|(method, _, _)| method == "GET"));
    }
}

#[tokio::test]
async fn unavailable_malformed_incomplete_duplicate_and_paginated_results_fail_closed() {
    for variant in 0..6 {
        let mut rows = replies("person-one", json!({"type":"personal"}));
        let (status, body) = rows.get_mut("/api/v1/channel-conversations").unwrap();
        match variant {
            0 => *status = StatusCode::SERVICE_UNAVAILABLE,
            1 => *status = StatusCode::FORBIDDEN,
            2 => body["total"] = json!(2),
            3 => body["next_cursor"] = json!("next"),
            4 => {
                let first = body["conversations"][0].clone();
                body["conversations"].as_array_mut().unwrap().push(first);
                body["total"] = json!(2);
            }
            5 => body["conversations"] = json!([]),
            _ => unreachable!(),
        }
        let f = fixture(rows).await;
        let result = inspect(&f, &personal(), "person-one").await;
        assert!(result.is_err());
        if variant == 0 {
            assert!(matches!(result, Err(BridgeError::AuthorityUnavailable)));
        }
        assert!(!format!("{:?}", result.err().unwrap()).contains(SECRET));
    }
}

#[tokio::test]
async fn wrong_configured_destination_is_refused_before_sending_a_credential() {
    let mut f = fixture(replies("person-one", json!({"type":"personal"}))).await;
    f.connector.base_url = "https://unrelated.example".into();
    assert!(inspect(&f, &personal(), "person-one").await.is_err());
    assert!(f.state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn redirects_never_receive_the_selected_key_and_pagination_headers_refuse_inventory() {
    let destination = fixture(replies("person-one", json!({"type":"personal"}))).await;
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.insert("/api/v1/channel-conversations".into(), (StatusCode::FOUND,
        json!({"fixture_location": format!("{}/api/v1/channel-conversations", destination.connector.base_url)})));
    let f = fixture(rows).await;
    assert!(inspect(&f, &personal(), "person-one").await.is_err());
    assert!(destination.state.requests.lock().unwrap().is_empty());
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.get_mut("/api/v1/channel-conversations").unwrap().1["fixture_link"] =
        json!("</api/v1/channel-conversations?page=2>; rel=\"next\"");
    let f = fixture(rows).await;
    assert!(inspect(&f, &personal(), "person-one").await.is_err());
}

#[tokio::test]
async fn oversized_or_excessive_provider_inventory_cannot_extend_authority() {
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.get_mut("/api/v1/api-keys").unwrap().1["keys"][0]["description"] =
        json!("x".repeat(1_048_577));
    let f = fixture(rows).await;
    assert!(inspect(&f, &personal(), "person-one").await.is_err());
    let mut rows = replies("person-one", json!({"type":"personal"}));
    rows.get_mut("/api/v1/channel-bots").unwrap().1 = json!({"bots": (0..=MAX_ROWS).map(|id| json!({"id":format!("bot-{id}"),"user_id":"person-one","platform":"telegram","is_active":true})).collect::<Vec<_>>(),"total":MAX_ROWS+1});
    let f = fixture(rows).await;
    assert!(inspect(&f, &personal(), "person-one").await.is_err());
}

#[tokio::test]
async fn provider_fractional_and_offset_expirations_bound_the_original_observation() {
    for expiry in ["2099-01-01T00:00:00.123+00:00", "2099-01-01T08:00:00+08:00"] {
        let mut rows = replies("person-one", json!({"type":"personal"}));
        rows.get_mut("/api/v1/api-keys").unwrap().1["keys"][0]["expires_at"] = json!(expiry);
        let f = fixture(rows).await;
        let observation = inspect(&f, &personal(), "person-one").await.unwrap();
        assert!(observation.deadline <= tokio::time::Instant::now() + Duration::from_secs(60));
    }
}

#[path = "tests/custody_tests.rs"]
mod custody;
mod custody_semantics;

#[tokio::test]
async fn upstream_only_inventory_rows_do_not_block_supported_bots_or_alias_binding_authority() {
    let mut rows = replies("person-one", json!({"type":"personal"}));
    let bots = &mut rows.get_mut("/api/v1/channel-bots").unwrap().1;
    bots["bots"].as_array_mut().unwrap().push(
        json!({"id":"upstream-bot","user_id":"person-one","platform":"device","is_active":true}),
    );
    bots["total"] = json!(2);
    let keys = &mut rows.get_mut("/api/v1/api-keys").unwrap().1["keys"];
    let mut extra = keys[0].clone();
    extra["id"] = json!("slack-key");
    extra["platform"] = json!("device");
    keys.as_array_mut().unwrap().push(extra);
    let f = fixture(rows.clone()).await;
    inspect(&f, &personal(), "person-one").await.unwrap();
    let choices = f
        .provider
        .choices(
            &personal(),
            &SubjectId::new("person-one"),
            &SecretValue::new(SECRET.into()),
        )
        .await
        .unwrap();
    assert_eq!(choices.bots.len(), 1);
    assert_eq!(choices.bots[0].id, "bot-one");
    assert_eq!(choices.bots[0].platform, BridgePlatform::Telegram);
    for list in ["/api/v1/channel-bots", "/api/v1/api-keys"] {
        let mut changed = rows.clone();
        let field = if list.ends_with("channel-bots") {
            "bots"
        } else {
            "keys"
        };
        changed.get_mut(list).unwrap().1[field][0]["platform"] = json!("telegram-new");
        let f = fixture(changed).await;
        assert!(matches!(
            inspect(&f, &personal(), "person-one").await,
            Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::BindingMismatch
            ))
        ));
    }
    rows.get_mut("/api/v1/channel-bots").unwrap().1["bots"][1]["user_id"] = json!("foreign-owner");
    let f = fixture(rows).await;
    assert!(f
        .provider
        .choices(
            &personal(),
            &SubjectId::new("person-one"),
            &SecretValue::new(SECRET.into())
        )
        .await
        .is_err());
}

#[tokio::test]
async fn telegram_new_binding_requires_exact_bot_key_and_route_platform() {
    let mut rows = replies("person-one", json!({"type":"personal"}));
    for (path, field) in [
        ("/api/v1/channel-bots", "bots"),
        ("/api/v1/api-keys", "keys"),
        ("/api/v1/channel-conversations", "conversations"),
    ] {
        rows.get_mut(path).unwrap().1[field][0]["platform"] = json!("telegram-new");
    }
    let mut f = fixture(rows.clone()).await;
    f.connector.platform = BridgePlatform::TelegramNew;
    inspect(&f, &personal(), "person-one").await.unwrap();
    let choices = f
        .provider
        .choices(
            &personal(),
            &SubjectId::new("person-one"),
            &SecretValue::new(SECRET.into()),
        )
        .await
        .unwrap();
    assert_eq!(choices.bots[0].platform, BridgePlatform::TelegramNew);
    for (path, field) in [
        ("/api/v1/channel-bots", "bots"),
        ("/api/v1/api-keys", "keys"),
        ("/api/v1/channel-conversations", "conversations"),
    ] {
        let mut changed = rows.clone();
        changed.get_mut(path).unwrap().1[field][0]["platform"] = json!("telegram");
        let mut f = fixture(changed).await;
        f.connector.platform = BridgePlatform::TelegramNew;
        assert!(matches!(
            inspect(&f, &personal(), "person-one").await,
            Err(BridgeError::ConnectionRequired(
                BridgeSetupBlock::BindingMismatch
            ))
        ));
    }
}
