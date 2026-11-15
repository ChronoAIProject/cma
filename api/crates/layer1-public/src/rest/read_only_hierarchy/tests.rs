use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request},
};
use layer2_core::{
    sandbox::store::{InMemorySandboxStore, SandboxStore},
    Authorization, Capability, SubjectId,
};
use tower::ServiceExt;

async fn fixture() -> (Router, Arc<InMemorySandboxStore>) {
    let store = Arc::new(InMemorySandboxStore::default());
    for (id, owner) in [("sbx_a", "alice"), ("sbx_b", "bob")] {
        store
            .create_sandbox(super::super::instances_tests::paused_sandbox(id, owner))
            .await
            .unwrap();
    }
    (
        read_only_hierarchy_router(HierarchyReadService::new(store.clone())),
        store,
    )
}

fn request(method: Method, path: &str, subject: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    if let Some(subject) = subject {
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId::new(subject)));
        request
            .extensions_mut()
            .insert(Authorization::new([Capability::UseOwnSandboxes]));
    }
    request
}

#[tokio::test]
async fn personal_reads_keep_existing_shapes_and_conceal_foreign_ids() {
    let (app, _) = fixture().await;
    let listed = app
        .clone()
        .oneshot(request(Method::GET, "/api/v1/sandboxes", Some("alice")))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(listed.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], "sbx_a");
    let own = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/sandboxes/sbx_a",
            Some("alice"),
        ))
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::OK);
    let foreign = app
        .clone()
        .oneshot(request(
            Method::GET,
            "/api/v1/sandboxes/sbx_b",
            Some("alice"),
        ))
        .await
        .unwrap();
    let missing = app
        .oneshot(request(
            Method::GET,
            "/api/v1/sandboxes/sbx_missing",
            Some("alice"),
        ))
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        to_bytes(foreign.into_body(), 65536).await.unwrap(),
        to_bytes(missing.into_body(), 65536).await.unwrap()
    );
}

#[tokio::test]
async fn inert_router_never_admits_mutations_or_shared_inventory() {
    let (app, store) = fixture().await;
    let before = store.get_sandbox(&SandboxId("sbx_a".into())).await.unwrap();
    for (method, path) in [
        (Method::POST, "/api/v1/sandboxes"),
        (Method::DELETE, "/api/v1/sandboxes/sbx_a"),
        (Method::POST, "/api/v1/sandboxes/sbx_a/revive"),
        (Method::GET, "/api/v1/sandboxes?shared=true"),
        (Method::GET, "/api/v1/sandboxes?organization=alice"),
    ] {
        assert!(!app
            .clone()
            .oneshot(request(method, path, Some("alice")))
            .await
            .unwrap()
            .status()
            .is_success());
    }
    assert_eq!(
        store.get_sandbox(&SandboxId("sbx_a".into())).await.unwrap(),
        before
    );
    assert_eq!(
        app.oneshot(request(Method::GET, "/api/v1/sandboxes", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
