use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use layer2_core::sandbox::{HierarchyReadService, WorkspaceService};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn acknowledged_policy_post_remains_readable_on_a_second_client() {
    for origin in [
        SandboxOrigin::Lifecycle {
            operation_id: "http-policy-operation".into(),
        },
        SandboxOrigin::Trigger {
            launch_id: "http-policy-launch".into(),
        },
    ] {
        let (client, name) = database("policy_http").await;
        let database = client.database(&name);
        assert!(database.list_collection_names().await.unwrap().is_empty());
        let result = AssertUnwindSafe(exercise_http(&client, &name, origin))
            .catch_unwind()
            .await;
        database.drop().await.unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}

async fn router(client: &Client, database: &str) -> Router {
    let store = Arc::new(MongoSandboxStore::open(client, database).await.unwrap());
    let operations = Arc::new(
        crate::store::MongoOperationStore::open(client, database)
            .await
            .unwrap(),
    );
    layer1_public::rest::hierarchy_router(layer1_public::rest::HierarchyState::new(
        owner_service(store.clone()),
        WorkspaceService::new(store.clone(), Arc::new(InMemoryProfileStore::default())),
        HierarchyReadService::new(store),
        operations,
    ))
}

async fn request(app: &Router, path: &str, body: Option<&Value>, expected: StatusCode) -> Value {
    let method = if body.is_some() {
        Method::POST
    } else {
        Method::GET
    };
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("Idempotency-Key", "retained-policy-relinquish")
        .body(Body::from(body.map(Value::to_string).unwrap_or_default()))
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId(
            "owner".into(),
        )));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).unwrap()
}

async fn exercise_http(client: &Client, database: &str, origin: SandboxOrigin) {
    let (store, admission, current, agent) = setup(client, database, origin, Some(true)).await;
    let before = store
        .sandboxes
        .find_one(doc! {"_id": current.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let writer = router(client, database).await;
    let peer_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let peer = router(&peer_client, database).await;
    let path = format!("/api/v1/sandboxes/{}", current.id);
    assert_eq!(
        request(&peer, &path, None, StatusCode::OK).await["always_on"],
        true
    );
    let body = json!({"expected_revision": current.revision, "always_on": false});
    let accepted = request(
        &writer,
        &format!("{path}/always-on"),
        Some(&body),
        StatusCode::OK,
    )
    .await;
    assert_eq!(accepted["always_on"], false);
    assert_eq!(accepted["revision"], current.revision + 1);
    let observed = request(&peer, &path, None, StatusCode::OK).await;
    assert_eq!(observed["id"], current.id.as_str());
    assert_eq!(observed["always_on"], false);
    assert_eq!(observed["revision"], current.revision + 1);
    drop(writer);
    drop(peer);
    drop(peer_client);
    let reopened_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let reopened = router(&reopened_client, database).await;
    assert_eq!(
        request(
            &reopened,
            &format!("{path}/always-on"),
            Some(&body),
            StatusCode::OK
        )
        .await,
        accepted
    );
    assert_eq!(
        request(&reopened, &path, None, StatusCode::OK).await,
        observed
    );
    let after = store
        .sandboxes
        .find_one(doc! {"_id": current.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    for field in [
        "admission_name",
        "admission_fingerprint",
        "origin_type",
        "origin_launch_id",
        "origin_operation_id",
    ] {
        assert_eq!(before.get(field), after.get(field), "changed {field}");
    }
    let expected = store.get_sandbox(&current.id).await.unwrap().unwrap();
    assert_reopened(database, &expected, &admission, &agent).await;
}
