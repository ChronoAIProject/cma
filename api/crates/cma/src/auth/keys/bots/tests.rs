use super::*;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use layer2_core::resource_access::ResourceOwner;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

async fn inventory() -> Json<Value> {
    Json(json!({"services":[
        {"id":"personal-llm","slug":"llm","is_active":true,"credential_source":{"type":"personal"}},
        {"id":"org-one-llm","slug":"llm","is_active":true,"credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":true}},
        {"id":"org-two-llm","slug":"llm","is_active":true,"credential_source":{"type":"org","org_id":"org-two","role":"admin","allowed":true}},
        {"id":"org-one-denied","slug":"denied","is_active":true,"credential_source":{"type":"org","org_id":"org-one","role":"admin","allowed":false}},
        {"id":"personal-only","slug":"personal-only","is_active":true,"credential_source":{"type":"personal"}}
    ]}))
}
async fn mint(
    State(requests): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    requests.lock().await.push(body.clone());
    Json(
        json!({"id":"minted-org-key","full_key":"fixture-credential","state_version":1,"scopes":"proxy read",
        "allowed_service_ids":body["allowed_service_ids"],"allowed_node_ids":body["allowed_node_ids"],"allow_all_services":false,"allow_all_nodes":false,"allow_auto_connected_services":false}),
    )
}

#[tokio::test]
async fn org_bot_scope_uses_only_exact_org_connection_and_native_target_owner() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/api/v1/user-services", get(inventory))
        .route(
            "/api/v1/api-keys/scope-plan",
            post(|Json(body): Json<Value>| async move {
                Json(scope_plan_fixture("person", &body, &["node-one"]))
            }),
        )
        .route("/api/v1/api-keys", post(mint))
        .with_state(requests.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let issuer = KeyIssuer::new(reqwest::Client::new(), endpoint, "test");
    let org = ResourceOwner::Organization {
        subject: layer2_core::SubjectId::new("org-one"),
    };
    issuer
        .mint_bot(
            "cma:test:agent",
            "fixture-user",
            &[ServiceId::new("llm")],
            None,
            &org,
        )
        .await
        .unwrap();
    let rows = requests.lock().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["target_org_id"], "org-one");
    assert_eq!(rows[0]["allowed_service_ids"], json!(["org-one-llm"]));
    for field in [
        "allow_all_services",
        "allow_all_nodes",
        "allow_auto_connected_services",
    ] {
        assert_eq!(rows[0][field], false);
    }
    drop(rows);
    for rejected in ["personal-only", "personal-llm", "org-two-llm", "denied"] {
        assert!(issuer
            .mint_bot(
                "cma:test:agent",
                "fixture-user",
                &[ServiceId::new(rejected)],
                None,
                &org
            )
            .await
            .is_err());
    }
    assert_eq!(
        requests.lock().await.len(),
        1,
        "rejected grants must not issue a key"
    );
    let person = ResourceOwner::Person {
        subject: layer2_core::SubjectId::new("person"),
    };
    issuer
        .mint_bot(
            "cma:test:personal",
            "fixture-user",
            &[ServiceId::new("llm")],
            None,
            &person,
        )
        .await
        .unwrap();
    let rows = requests.lock().await;
    assert_eq!(rows[1]["allowed_service_ids"], json!(["personal-llm"]));
    assert!(rows[1].get("target_org_id").is_none());
    server.abort();
}
