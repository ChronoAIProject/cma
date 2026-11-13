use crate::rest::{
    admin::tests::{fixture, sandbox},
    agents_tests::request,
};
use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
    Router,
};
use layer2_core::sandbox::store::SandboxStore;
use layer2_core::{Authorization, Capability};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn send(
    app: &Router,
    method: Method,
    path: &str,
    body: Option<Value>,
    capabilities: &[Capability],
) -> (StatusCode, Value) {
    let mut req = request(method, path, "operator-a", None, body);
    req.extensions_mut()
        .insert(Authorization::new(capabilities.iter().copied()));
    let response = app.clone().oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_048_576).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn administrator_approves_and_revokes_with_audit_and_filtered_inventory() {
    let f = fixture(false);
    let original = sandbox("sbx_exception", "owner-a", "v1");
    f.sandboxes.create_sandbox(original.clone()).await.unwrap();
    let path = "/api/v1/admin/sandboxes/sbx_exception/always-on";
    let control = [Capability::ControlAnySandbox];
    let refused = send(
        &f.app,
        Method::POST,
        path,
        Some(json!({"always_on": true, "expected_revision": 1})),
        &[Capability::UseOwnSandboxes],
    )
    .await;
    assert_eq!(refused.0, StatusCode::FORBIDDEN);
    assert_eq!(refused.1["code"], "forbidden");
    assert_eq!(
        f.sandboxes.get_sandbox(&original.id).await.unwrap(),
        Some(original)
    );
    for (value, revision) in [(true, 1), (true, 2), (false, 3)] {
        let result = send(
            &f.app,
            Method::POST,
            path,
            Some(json!({"always_on": value, "expected_revision": revision})),
            &control,
        )
        .await;
        assert_eq!(result.0, StatusCode::OK);
        assert_eq!(result.1["always_on"], value);
        assert_eq!(result.1["revision"], revision + 1);
        let listed = send(
            &f.app,
            Method::GET,
            "/api/v1/admin/sandboxes?always_on=true&limit=1",
            None,
            &[Capability::ReadAllSandboxes],
        )
        .await;
        assert_eq!(listed.0, StatusCode::OK);
        assert_eq!(
            listed.1["items"].as_array().unwrap().len(),
            usize::from(value)
        );
    }
    let stale = send(
        &f.app,
        Method::POST,
        path,
        Some(json!({"always_on": true, "expected_revision": 1})),
        &control,
    )
    .await;
    assert_eq!(stale.0, StatusCode::CONFLICT);
    assert_eq!(stale.1["code"], "operator_conflict");
    let audits = send(
        &f.app,
        Method::GET,
        "/api/v1/admin/activity?action=configure",
        None,
        &[Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(audits.0, StatusCode::OK);
    let items = audits.1["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for (record, (value, revision)) in
        items
            .iter()
            .zip([("true", "2"), ("true", "3"), ("false", "4")])
    {
        assert_eq!(record["actor"], "operator-a");
        assert_eq!(
            record["target"],
            json!({"kind": "resource", "id": "sandbox:sbx_exception"})
        );
        assert_eq!(
            record["detail"],
            json!({"setting": "always_on", "status": value, "revision": revision})
        );
    }
}
