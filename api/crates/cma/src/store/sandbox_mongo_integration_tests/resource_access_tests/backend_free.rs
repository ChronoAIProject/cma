use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{net::SocketAddr, time::Duration};
use tower::ServiceExt as _;

use super::*;
use crate::config::{BackendFreeConfig, StartupConfig};
use crate::store::{
    sandbox_mongo::{AGENT_COMMANDS, AGENT_COMMAND_ORDERS},
    sandbox_mongo_integration_tests::command_ledger_tests,
};
use layer2_core::{ids::AgentResponseId, sandbox::agent_io::AgentCommandStore};

#[path = "backend_free/authentication.rs"]
mod authentication;
#[path = "backend_free/legacy_responses.rs"]
mod legacy_responses;
use authentication::{assert_actor_callback_is_refused, browser_session, Identity};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn backend_free_http_uses_durable_owner_reads_without_resuming_pending_work() {
    let (client, name) = database("backend_free").await;
    let (store, target) = fixture(&client, &name).await;
    let ResourceTarget::Agent { agent_id } = target else {
        panic!("retained Agent fixture required");
    };
    let agent = store.get_agent(&agent_id).await.unwrap().unwrap();
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let (agent, segmented_agent_id) = legacy_responses::seed(&store, agent, &parent).await;
    let command = command_ledger_tests::record(
        store
            .reserve_command(command_ledger_tests::request(
                &agent,
                &parent,
                "retained-response",
            ))
            .await
            .unwrap(),
    );
    let receipt = &command.receipt;
    let response_id = receipt.response_id.as_ref().unwrap();
    let command_key = doc! {"_id": receipt.envelope.command_id.as_str()};
    let command_before = store
        .agent_commands
        .find_one(command_key.clone())
        .await
        .unwrap();
    let mut pending = provisioning_sandbox("sbx_pending_backend_free");
    pending.name = "pending backend-free fixture".into();
    store.create_sandbox(pending.clone()).await.unwrap();
    let db = client.database(&name);
    let mongo_uri = isolated_uri(&db).await;
    let sandbox_records = db.collection::<Document>(SANDBOXES);
    let pending_key = doc! {"_id": pending.id.as_str()};
    let before = sandbox_records
        .find_one(pending_key.clone())
        .await
        .unwrap()
        .unwrap();
    let execution_collections = [
        crate::store::sandbox_mongo::SANDBOX_ADMISSIONS,
        crate::store::trigger_definitions::LAUNCHES,
        crate::store::agent_lifecycle_operations::COLLECTION,
        crate::store::agent_observation::COLLECTION,
        AGENT_COMMANDS,
        AGENT_COMMAND_ORDERS,
        AGENT_LOG,
    ];
    let mut work_counts = Vec::new();
    for name in execution_collections {
        let collection = db.collection::<Document>(name);
        work_counts.push((
            collection.clone(),
            collection.count_documents(doc! {}).await.unwrap(),
        ));
    }
    drop(store);

    let owner_config = config(&name, mongo_uri.as_str(), Some("owner"));
    let (owner, owner_refresh) = crate::composition::backend_free::compose(&owner_config)
        .await
        .unwrap();
    let (foreign, foreign_refresh) = crate::composition::backend_free::compose(&config(
        &name,
        mongo_uri.as_str(),
        Some("other-owner"),
    ))
    .await
    .unwrap();
    let identity = Identity::start().await;
    let mut provider = config(&name, mongo_uri.as_str(), None);
    if let crate::auth::AuthMode::Provider { base_url, .. } = &mut provider.authentication.auth_mode
    {
        *base_url = identity.origin.clone();
    }
    let (unauthenticated, unauthenticated_refresh) =
        crate::composition::backend_free::compose(&provider)
            .await
            .unwrap();
    assert_actor_callback_is_refused(&unauthenticated, &provider, &client, &name).await;
    for id in [&agent_id, &segmented_agent_id] {
        legacy_responses::assert_refusals(&owner, &foreign, &unauthenticated, id).await;
    }
    let absent_response = AgentResponseId::generate();
    for suffix in ["", "/events", "/output", "/retained-output"] {
        assert_command_refusals(
            &owner,
            &foreign,
            &unauthenticated,
            &format!("/api/v2/responses/{response_id}{suffix}"),
            &format!("/api/v2/responses/{absent_response}{suffix}"),
        )
        .await;
    }
    assert_command_refusals(
        &owner,
        &foreign,
        &unauthenticated,
        &format!("/api/v2/commands/{}", receipt.envelope.command_id),
        &format!(
            "/api/v2/commands/{}",
            layer2_core::ids::AgentCommandId::generate()
        ),
    )
    .await;
    assert_command_refusals(
        &owner,
        &foreign,
        &unauthenticated,
        &format!("/api/v2/agents/{agent_id}/responses/{response_id}/controls"),
        &format!("/api/v2/agents/agt_absent/responses/{response_id}/controls"),
    )
    .await;
    for version in ["v1", "v2"] {
        for suffix in ["", "/items"] {
            let path = format!("/api/{version}/conversations/{agent_id}{suffix}");
            let (status, problem) = request(&owner, Method::GET, &path, None).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {problem}");
            assert_eq!(problem["code"], "sandbox_backend_unavailable");
            assert_eq!(
                request(&foreign, Method::GET, &path, None).await.0,
                StatusCode::NOT_FOUND
            );
        }
    }

    assert_eq!(
        request(&owner, Method::GET, "/livez", None).await.0,
        StatusCode::OK
    );
    let ready = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let (status, ready) = request(&owner, Method::GET, "/ready", None).await;
            if status == StatusCode::OK {
                return ready;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(ready["sandbox"]["available"], false);
    assert_eq!(ready["workers"], json!([]));
    assert_eq!(
        request(&owner, Method::GET, "/auth/session", None).await.0,
        StatusCode::OK
    );

    let (status, inventory) = request(&owner, Method::GET, "/api/v1/agents", None).await;
    assert_eq!(status, StatusCode::OK, "{inventory}");
    let owned = inventory["items"].as_array().unwrap();
    assert_eq!(owned.len(), 2);
    for id in [&agent_id, &segmented_agent_id] {
        assert!(owned.iter().any(|row| row["id"] == id.as_str()));
    }
    let (status, inventory) = request(&foreign, Method::GET, "/api/v1/agents", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inventory["items"], json!([]));

    let operator = browser_session(&unauthenticated, "fixture-operator-code").await;
    let (status, settings) = request(&operator, Method::GET, "/api/v1/admin/settings", None).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    let update = json!({
        "expected_revision": settings["revision"].as_u64().unwrap(),
        "usage_reporting": {"measured_visible": true},
    });
    assert_eq!(
        request(
            &operator,
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(update.clone())
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &operator,
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(update)
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &unauthenticated,
            Method::GET,
            "/api/v1/admin/settings",
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    let desktop = format!("/api/v1/agents/{agent_id}/desktop/ws");
    let events = format!("/api/v1/agents/{agent_id}/events");
    let agui = format!("/api/v1/agui/events?thread_id={agent_id}");
    let agent_events_v2 = format!("/api/v2/agents/{agent_id}/events");
    for (method, path) in [
        (Method::POST, "/api/v1/sandboxes"),
        (Method::GET, &desktop),
        (Method::GET, &events),
        (Method::GET, &agent_events_v2),
        (Method::GET, &agui),
    ] {
        let (status, problem) = request(&owner, method.clone(), path, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {problem}");
        assert_eq!(problem["code"], "sandbox_backend_unavailable");
        let (status, _) = request(&unauthenticated, method, path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(
        request(&foreign, Method::GET, &desktop, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&foreign, Method::GET, &agui, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &owner,
            Method::GET,
            "/api/v1/agui/launches/absent/events",
            None,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&owner, Method::PATCH, &desktop, None).await.0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        request(
            &owner,
            Method::GET,
            "/api/v1/unknown-backend-free-route",
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let signed_in = browser_session(&unauthenticated, "fixture-owner-code").await;
    assert_eq!(
        request(&signed_in, Method::GET, "/api/v1/agents", None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        request(&signed_in, Method::GET, "/api/v1/admin/settings", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&signed_in, Method::GET, &desktop, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&signed_in, Method::POST, "/api/v1/sandboxes", None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    drop(signed_in);

    drop((owner, foreign, unauthenticated, operator));
    for handle in [owner_refresh, foreign_refresh, unauthenticated_refresh] {
        handle.abort();
        let _ = handle.await;
    }
    assert_eq!(
        sandbox_records
            .find_one(pending_key)
            .await
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(sandbox_records.count_documents(doc! {}).await.unwrap(), 2);
    assert_eq!(
        db.collection::<Document>(AGENT_COMMANDS)
            .find_one(command_key)
            .await
            .unwrap(),
        command_before,
    );
    for (collection, before) in work_counts {
        assert_eq!(collection.count_documents(doc! {}).await.unwrap(), before);
    }
    db.run_command(doc! {"dropUser": "control-plane-fixture"})
        .await
        .unwrap();
    db.run_command(doc! {"dropRole": "cma_am_runtime"})
        .await
        .unwrap();
    db.drop().await.unwrap();
}

async fn assert_command_refusals(
    owner: &Router,
    foreign: &Router,
    unauthenticated: &Router,
    path: &str,
    absent: &str,
) {
    let (status, problem) = request(owner, Method::GET, path, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {problem}");
    assert_eq!(problem["code"], "sandbox_backend_unavailable");
    let hidden = request(foreign, Method::GET, path, None).await;
    assert_eq!(hidden.0, StatusCode::NOT_FOUND, "{path}: {}", hidden.1);
    assert_eq!(hidden.1["error"]["code"], "response_not_found");
    assert_eq!(request(owner, Method::GET, absent, None).await, hidden);
    assert_eq!(request(foreign, Method::GET, absent, None).await, hidden);
    let (status, problem) = request(unauthenticated, Method::GET, path, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}: {problem}");
    assert_eq!(problem["error"]["code"], "authentication_required");
}

async fn isolated_uri(db: &mongodb::Database) -> url::Url {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut uri = url::Url::parse(&uri).unwrap();
    let password = uuid::Uuid::new_v4().simple().to_string();
    db.run_command(crate::store::isolation::role(db.name()))
        .await
        .unwrap();
    db.run_command(doc! {
        "createUser": "control-plane-fixture", "pwd": &password,
        "roles": [{"role": "cma_am_runtime", "db": db.name()}],
    })
    .await
    .unwrap();
    uri.set_username("control-plane-fixture").unwrap();
    uri.set_password(Some(&password)).unwrap();
    uri.query_pairs_mut().append_pair("authSource", db.name());
    uri
}

fn config(database: &str, uri: &str, trusted: Option<&str>) -> BackendFreeConfig {
    let selected = StartupConfig::from_lookup(|key| {
        Ok(match key {
            "CHRONO_SANDBOX_PROVIDER" => Some("none".into()),
            "CHRONO_ENVIRONMENT" => Some("staging".into()),
            "CHRONO_MONGODB_URI" => Some(uri.into()),
            "CHRONO_MONGODB_DATABASE" => Some(database.into()),
            "CHRONO_DEPLOYMENT_ID" => Some("fixture".into()),
            "CHRONO_AUTH_ENCRYPTION_KEY" => Some(STANDARD.encode([9; 32])),
            "CHRONO_AUTH_MODE" => trusted.map(|_| "trusted".into()),
            "CHRONO_TRUSTED_SUBJECT" => trusted.map(str::to_owned),
            "CHRONO_NYXID_BASE_URL" | "CHRONO_NYXID_PUBLIC_URL" => {
                Some("https://identity.example.test".into())
            }
            "CHRONO_NYXID_CLIENT_ID" => Some("fixture".into()),
            "CHRONO_NYXID_CLIENT_SECRET" => Some(uuid::Uuid::new_v4().simple().to_string()),
            _ => None,
        })
    })
    .unwrap();
    let StartupConfig::BackendFree(config) = selected else {
        panic!("backend-free mode not selected");
    };
    *config
}

async fn request(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()));
    if path.ends_with("/desktop/ws") {
        request = request
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(
                header::SEC_WEBSOCKET_KEY,
                STANDARD.encode(uuid::Uuid::new_v4().as_bytes()),
            );
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = tokio::time::timeout(
        Duration::from_secs(20),
        app.clone().oneshot(request.body(body).unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let status = response.status();
    assert!(!response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| { value.as_bytes().starts_with(b"text/event-stream") }));
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        to_bytes(response.into_body(), 64 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
