use super::*;
use axum::http::HeaderMap;

fn node(state: &ProviderState) -> &'static str {
    if state.rebound.load(Ordering::SeqCst) {
        "replacement-node"
    } else {
        "backing-node"
    }
}

pub(super) async fn plan(
    State(state): State<Arc<ProviderState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    Json(crate::auth::keys::scope_plan_fixture(
        "owner",
        &body,
        &[node(&state)],
    ))
}

pub(super) async fn proxy(
    State(state): State<Arc<ProviderState>>,
    headers: HeaderMap,
) -> StatusCode {
    let Some(number) = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer plaintext-key-"))
        .and_then(|value| value.parse::<usize>().ok())
    else {
        return StatusCode::UNAUTHORIZED;
    };
    if number == 0
        || state
            .revoked
            .lock()
            .unwrap()
            .contains(&format!("provider-key-{number}"))
    {
        return StatusCode::UNAUTHORIZED;
    }
    let requests = state.mint_requests.lock().unwrap();
    let Some(grant) = requests.get(number - 1) else {
        return StatusCode::UNAUTHORIZED;
    };
    if grant["allowed_service_ids"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("provider-service-id"))
        && grant["allowed_node_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(node(&state)))
    {
        StatusCode::OK
    } else {
        StatusCode::FORBIDDEN
    }
}

#[tokio::test]
async fn node_backed_http_access_survives_renewal_and_rebinding_then_revokes() {
    let (endpoint, state, server) = provider().await;
    let client = reqwest::Client::new();
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(client.clone(), &endpoint, "test")),
        Arc::new(Tokens::default()),
        Arc::new(MemoryCustody::default()),
    );
    let value = sandbox();
    let now = SystemTime::now();
    let first = controller.issue(&value, 1, now).await.unwrap();
    let call = |key: String| {
        client
            .get(format!("{endpoint}/proxy"))
            .bearer_auth(key)
            .send()
    };
    assert_eq!(
        call(first.value.expose().into()).await.unwrap().status(),
        StatusCode::OK
    );
    let renewed = controller.issue(&value, 2, now).await.unwrap();
    assert_eq!(
        call(renewed.value.expose().into()).await.unwrap().status(),
        StatusCode::OK
    );
    controller
        .revoke(&value, &first.authority.provider_key_id)
        .await
        .unwrap();
    assert_eq!(
        call(first.value.expose().into()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    state.rebound.store(true, Ordering::SeqCst);
    assert_eq!(
        call(renewed.value.expose().into()).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let rebound = controller.issue(&value, 3, now).await.unwrap();
    assert_eq!(
        call(rebound.value.expose().into()).await.unwrap().status(),
        StatusCode::OK
    );
    assert_ne!(
        renewed.authority.grant_fingerprint,
        rebound.authority.grant_fingerprint
    );
    controller
        .revoke(&value, &renewed.authority.provider_key_id)
        .await
        .unwrap();
    controller
        .revoke(&value, &rebound.authority.provider_key_id)
        .await
        .unwrap();
    assert_eq!(
        call(rebound.value.expose().into()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    for grant in state.mint_requests.lock().unwrap().iter() {
        assert_eq!(grant["allow_all_nodes"], false);
        assert_eq!(grant["allow_all_services"], false);
        assert_eq!(grant["allowed_node_ids"].as_array().unwrap().len(), 1);
        assert!(grant["scope_plan_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
    }
    server.abort();
}
