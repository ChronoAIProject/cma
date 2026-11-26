use axum::http::Request;
use layer2_core::{Authorization, SubjectId};

use super::*;

#[test]
fn operation_scopes_preserve_opaque_resource_boundaries() {
    for operation in ["agent:approval", "trigger:placement:retire"] {
        assert_ne!(
            operation_scope(operation, &["one:two", "three"]),
            operation_scope(operation, &["one", "two:three"]),
        );
    }
    let resources = ["colon:inside", "quoted\"id", "back\\slash", "nul\0id", ""];
    let scope = operation_scope("workspace:create", &resources);
    let decoded: (String, Vec<String>) = serde_json::from_str(&scope).unwrap();
    assert_eq!(decoded.0, "workspace:create");
    assert_eq!(decoded.1, resources);
    assert_ne!(scope, operation_scope("workspace:delete", &resources));
    assert_ne!(
        operation_scope("operation", &[]),
        operation_scope("operation", &[""])
    );
}

#[tokio::test]
async fn keys_are_visible_ascii_and_bounded() {
    let longest = "x".repeat(MAX_KEY_BYTES);
    let oversized = "x".repeat(MAX_KEY_BYTES + 1);
    for (value, accepted) in [
        ("retry-1", true),
        ("", false),
        ("contains space", false),
        (longest.as_str(), true),
        (oversized.as_str(), false),
    ] {
        let request = Request::builder()
            .header(IDEMPOTENCY_KEY, value)
            .body(())
            .unwrap();
        let (mut parts, _) = request.into_parts();
        let result = IdempotencyKey::from_request_parts(&mut parts, &()).await;
        assert_eq!(result.is_ok(), accepted, "for a {} byte key", value.len());
    }

    let request = Request::builder()
        .header(IDEMPOTENCY_KEY, "first")
        .header(IDEMPOTENCY_KEY, "second")
        .body(())
        .unwrap();
    let (mut parts, _) = request.into_parts();
    assert!(matches!(
        IdempotencyKey::from_request_parts(&mut parts, &()).await,
        Err(IdempotencyProblem::Invalid)
    ));
}

#[tokio::test]
async fn an_absent_key_never_reads_the_store() {
    struct Panics;
    #[async_trait::async_trait]
    impl OperationStore for Panics {
        async fn reserve(
            &self,
            _claim: &OperationClaim,
        ) -> Result<OperationReservation, StoreError> {
            panic!("an unkeyed request must not reserve")
        }

        async fn complete(
            &self,
            _claim: &OperationClaim,
            _outcome: Vec<u8>,
        ) -> Result<(), StoreError> {
            panic!("an unkeyed request must not complete")
        }
    }

    let caller = Caller::new(SubjectId("subject-a".into()), Authorization::none());
    let result = Idempotency::begin(
        IdempotencyKey(None),
        Arc::new(Panics),
        &caller,
        "create-resource",
        [0; 32],
    )
    .await;
    let Ok(begun) = result else {
        panic!("an unkeyed request is accepted");
    };
    assert!(matches!(begun, Idempotency::Unkeyed));
}

#[tokio::test]
async fn response_encoding_preserves_status_headers_and_body() {
    let original = Response::builder()
        .status(StatusCode::CREATED)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"id":"resource-a"}"#))
        .unwrap();
    let (_, encoded) = encode_response(original).await.unwrap();
    let replay = decode_response(&encoded).unwrap();
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(replay.headers()["content-type"], "application/json");
    assert_eq!(
        to_bytes(replay.into_body(), usize::MAX).await.unwrap(),
        r#"{"id":"resource-a"}"#
    );
}
