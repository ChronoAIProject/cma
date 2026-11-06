use super::tests::{call, Store};
use super::*;
use axum::body::to_bytes;
use layer2_core::Capability;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn options_reauthorize_each_request_without_any_report_and_validate_inputs() {
    let store = Arc::new(Store::default());
    let admin = Some(("operator", &[Capability::ReadAllSandboxes][..]));
    let path = "/api/v1/admin/usage/metrics/options";
    for body in [
        r#"{"kind":"owner"}"#,
        r#"{"kind":"sandbox","owner":"owner-a"}"#,
        r#"{"kind":"runtime"}"#,
        r#"{"kind":"owner","cursor":"next-page"}"#,
    ] {
        assert_eq!(
            call(&store, "POST", path, None, body).await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&store, "POST", path, Some(("operator", &[])), body)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(
                &store,
                "POST",
                path,
                Some(("owner-a", &[Capability::UseOwnSandboxes])),
                body
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        let response = call(&store, "POST", path, admin, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 10_000).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["items"][0]["value"], "owner-a");
        if !body.contains("runtime") {
            assert_ne!(value["items"][0]["label"], "owner-a");
        }
    }
    for body in [
        r#"{"kind":"unknown"}"#,
        r#"{"kind":"owner","limit":51}"#,
        r#"{"kind":"owner","limit":0}"#,
        r#"{"kind":"owner","owner":"x"}"#,
        r#"{"kind":"sandbox","selected":"x","search":"y"}"#,
        r#"{"kind":"owner","search":"\n"}"#,
        r#"{"kind":"owner","unexpected":"x"}"#,
    ] {
        assert_eq!(
            call(&store, "POST", path, admin, body).await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    store.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        call(&store, "POST", path, admin, r#"{"kind":"owner"}"#)
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        call(
            &store,
            "POST",
            path,
            Some(("operator", &[])),
            r#"{"kind":"owner","cursor":"next-page"}"#
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}
