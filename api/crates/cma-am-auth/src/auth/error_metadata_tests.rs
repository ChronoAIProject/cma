//! Error recovery hints disclose disposition only and never constitute identity.
use axum::{body::to_bytes, http::header};
use serde::Deserialize;

#[derive(Deserialize)]
struct LegacyProblem {
    status: u16,
    code: String,
}

#[tokio::test]
async fn auth_retry_metadata_is_additive_and_contains_no_identity_evidence() {
    for (response, code, retryable, delay) in [
        (
            super::identity_failure(),
            "identity_unavailable",
            true,
            Some("3"),
        ),
        (super::store_failure(), "store_failed", false, None),
    ] {
        assert_eq!(response.status().as_u16(), 503);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .map(|v| v.to_str().unwrap()),
            delay
        );
        if retryable {
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let legacy: LegacyProblem = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(legacy.status, 503);
        assert_eq!(legacy.code, code);
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["retryable"], retryable);
        let keys: std::collections::BTreeSet<_> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["type", "title", "status", "detail", "code", "retryable"]
                .into_iter()
                .collect()
        );
    }
}
