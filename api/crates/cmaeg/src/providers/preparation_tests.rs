use super::*;
use crate::ingress::{
    routing_plan::fixture,
    routing_store::{test_server::Server, Candidate},
};
use crate::providers::{management_auth::CmaManagementAuth, ProviderRegistry};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    response::Response,
    Router,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
struct Observed {
    path: String,
    bearer: String,
    operation: Option<String>,
    generation: Option<String>,
    body: Value,
}
async fn receive(State(seen): State<Arc<Mutex<Vec<Observed>>>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body: Value =
        serde_json::from_slice(&to_bytes(body, 32768).await.unwrap()).unwrap_or(Value::Null);
    let observed = Observed {
        path: parts.uri.path().into(),
        bearer: parts.headers["authorization"].to_str().unwrap().into(),
        operation: parts
            .headers
            .get("idempotency-key")
            .map(|v| v.to_str().unwrap().into()),
        generation: parts
            .headers
            .get("x-cma-binding-generation")
            .map(|v| v.to_str().unwrap().into()),
        body: body.clone(),
    };
    seen.lock().unwrap().push(observed);
    let response = if parts.uri.path().ends_with("/preparation-inspections") {
        json!({"state":"pending","intent":{
            "version":1,"admission_id":body["admission_id"],"target_id":body["target_id"],"request_hash":body["request_hash"],
            "binding_id":parts.uri.path().split('/').nth(5).unwrap(),"owner":body["owner"],"manager":"manager","generation":3,"binding_revision":14,
            "binding_digest":"a".repeat(64),"sandbox_digest":"b".repeat(64),"workspace_digest":"c".repeat(64),"admitted_at":body["admitted_at"],"expires_at":body["expires_at"]}})
    } else if parts.uri.path().ends_with("/preparations") {
        let intent: PreparationIntent = serde_json::from_value(body["intent"].clone()).unwrap();
        json!({"version":1,"operation_id":intent.operation_id().unwrap(),"intent_fingerprint":intent.fingerprint().unwrap(),"generation":4,"sandbox_id":"sbx_reserved","workspace_id":"wks_reserved"})
    } else {
        json!({})
    };
    Response::builder()
        .header("content-type", "application/json")
        .body(Body::from(response.to_string()))
        .unwrap()
}
fn registry(base: &str) -> Value {
    json!([{"slug":"cma_codex","base_url":base,"kind":"responses_http","max_inflight":2,"paths":{
        "agent_card":"/card","binding":"/api/v1/cmaeg/bindings/{binding_id}","conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses",
        "preparation":{"inspection":"/api/v1/cmaeg/bindings/{binding_id}/preparation-inspections","reservation":"/api/v1/cmaeg/bindings/{binding_id}/preparations"}}}])
}

#[tokio::test]
async fn preparation_uses_only_management_auth_exact_operation_keys_and_generation_headers() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new().fallback(receive).with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let records = Server::start().await;
    let request = fixture::request(fixture::targets, fixture::message("fresh event"));
    let upload = Candidate::prepare(
        &request,
        &fixture::routed(&request),
        b"raw",
        &fixture::keys(),
        fixture::NOW,
    )
    .unwrap();
    let mut plan = records.store().admit(&upload).await.unwrap();
    plan.binding_preparation_version = Some(1);
    plan.targets[0].binding_replacement_version = Some(1);
    let target = &plan.targets[0];
    let key = Secret::new("nyx_bound_agent_key");
    let mut client = ProviderClient::new(
        ProviderRegistry::parse(&registry(&base).to_string()).unwrap(),
        Duration::from_secs(1),
        Duration::from_secs(2),
        2,
    )
    .unwrap();
    assert!(!client.supports_binding_preparation(&target.launch.provider.slug));
    assert!(matches!(
        client
            .inspect_binding_preparation(&plan, target, &key)
            .await,
        Err(ProviderError::InvalidRequest(_))
    ));
    assert!(seen.lock().unwrap().is_empty());
    client.management_auth =
        Some(CmaManagementAuth::new(&base, Secret::new("manager_service_token")).unwrap());
    assert!(client.supports_binding_preparation(&target.launch.provider.slug));
    let BindingPreparation::Pending { intent } = client
        .inspect_binding_preparation(&plan, target, &key)
        .await
        .unwrap()
    else {
        panic!("expected intent");
    };
    assert_eq!(
        intent.binding_id,
        target.launch.provider.binding_id.as_str()
    );
    let first = client
        .reserve_binding_preparation(
            &target.launch.provider.slug,
            &target.launch.provider.binding_id,
            &intent,
            &key,
        )
        .await
        .unwrap();
    let replay = client
        .reserve_binding_preparation(
            &target.launch.provider.slug,
            &target.launch.provider.binding_id,
            &intent,
            &key,
        )
        .await
        .unwrap();
    assert_eq!(first, replay);
    client
        .ensure_conversation_at_generation(
            &target.launch.provider.slug,
            &target.launch.provider.binding_id,
            &target.launch.event.conversation_id,
            &key,
            Some(4),
        )
        .await
        .unwrap();
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests
        .iter()
        .all(|request| request.bearer == "Bearer manager_service_token"));
    assert!(requests[0].path.ends_with("/preparation-inspections"));
    for request in &requests[1..3] {
        assert_eq!(
            request.operation.as_deref(),
            Some(intent.operation_id().unwrap().as_str())
        );
        assert_eq!(
            request.body["intent"],
            serde_json::to_value(&intent).unwrap()
        );
        assert_eq!(request.body["agent_key"], key.as_str());
    }
    assert_eq!(requests[3].generation.as_deref(), Some("4"));
    drop(requests);
    server.abort();
}

#[test]
fn preparation_capability_refuses_paths_outside_the_exact_cma_contract() {
    for path in [
        "/other/{binding_id}",
        "/api/v1/cmaeg/bindings/{binding_id}/preparations?target=other",
        "//other.example/{binding_id}",
    ] {
        let mut value = registry("http://127.0.0.1:8080");
        value[0]["paths"]["preparation"]["reservation"] = path.into();
        assert!(ProviderRegistry::parse(&value.to_string()).is_err());
    }
}
