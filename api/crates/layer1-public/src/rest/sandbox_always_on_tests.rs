use crate::rest::{
    agents_tests::{request, sandbox},
    instances_tests::fixture,
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

async fn send(app: &Router, subject: &str, key: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/api/v1/sandboxes/sbx_exception/always-on",
            subject,
            Some(key),
            Some(body),
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_048_576).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn owner_always_on_is_typed_write_free_and_idempotent() {
    for (index, preference) in [None, Some(false), Some(true)].into_iter().enumerate() {
        let f = fixture(3);
        let mut original = sandbox("sbx_exception", "owner-a");
        original.profile.always_on = preference;
        f.store.create_sandbox(original.clone()).await.unwrap();
        let enable = json!({"always_on": true, "expected_revision": original.revision});
        let refused = send(&f.router, "owner-a", "enable", enable.clone()).await;
        assert_eq!(refused.0, StatusCode::FORBIDDEN);
        assert_eq!(refused.1["code"], "always_on_approval_required");
        assert!(refused.1["detail"]
            .as_str()
            .unwrap()
            .contains("administrator"));
        assert_eq!(send(&f.router, "owner-a", "enable", enable).await, refused);
        assert_eq!(
            f.store.get_sandbox(&original.id).await.unwrap(),
            Some(original.clone())
        );
        let disable = json!({"always_on": false, "expected_revision": original.revision});
        let result = send(&f.router, "owner-a", "disable", disable.clone()).await;
        assert_eq!(result.0, StatusCode::OK, "{index}: {}", result.1);
        assert_eq!(
            result.1["revision"],
            original.revision + u64::from(preference == Some(true))
        );
        assert_eq!(send(&f.router, "owner-a", "disable", disable).await, result);
        let saved = f.store.get_sandbox(&original.id).await.unwrap().unwrap();
        if preference != Some(true) {
            assert_eq!(saved, original);
        } else {
            assert_eq!(saved.profile.always_on, Some(false));
        }
        for always_on in [false, true] {
            let body = json!({"always_on": always_on, "expected_revision": saved.revision});
            assert_eq!(
                send(
                    &f.router,
                    "someone-else",
                    &format!("other-{always_on}"),
                    body.clone()
                )
                .await
                .0,
                StatusCode::NOT_FOUND
            );
            let mut req = request(
                Method::POST,
                "/api/v1/sandboxes/sbx_exception/always-on",
                "owner-a",
                Some(&format!("forbidden-{always_on}")),
                Some(body),
            );
            req.extensions_mut()
                .insert(Authorization::new([] as [Capability; 0]));
            assert_eq!(
                f.router.clone().oneshot(req).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
    }
}
