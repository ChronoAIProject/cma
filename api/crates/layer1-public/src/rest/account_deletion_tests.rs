use std::sync::Arc;

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    Router,
};
use layer2_core::{
    account_deletion::{
        AccountDeletionService, AvailableAccounts, MemoryAccountDeletionStore,
        OwnerSessionAuthority,
    },
    Authorization, Capability, SubjectId,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn router(enabled: bool) -> Router {
    super::account_deletion::account_deletion_router(Arc::new(AccountDeletionService::new(
        Arc::new(MemoryAccountDeletionStore::default()),
        Arc::new(AvailableAccounts),
        enabled,
    )))
}

fn request(method: Method, key: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri("/api/v1/account/deletion");
    if let Some(key) = key {
        builder = builder.header("Idempotency-Key", key);
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let mut request = builder
        .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))
        .unwrap();
    request
        .extensions_mut()
        .insert(crate::AuthenticatedSubject(SubjectId::new("owner")));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    request
        .extensions_mut()
        .insert(OwnerSessionAuthority::verified_personal(SubjectId::new(
            "owner",
        )));
    request
}

async fn send(router: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16_384).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn post_returns_202_receipt_with_location() {
    let response = router(true)
        .oneshot(request(
            Method::POST,
            Some("delete"),
            Some(json!({"acknowledge":"irreversible"})),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/api/v1/account/deletion"
    );
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await.unwrap()).unwrap();
    assert_eq!(value["status"], "running");
    assert_eq!(value["step"], "inventory");
    for field in [
        "subject",
        "subject_hash",
        "active_subject_hash",
        "inventory",
        "work",
    ] {
        assert!(value.get(field).is_none(), "private field {field}");
    }
}

#[tokio::test]
async fn post_replay_same_key_returns_same_receipt() {
    let router = router(true);
    let first = send(
        &router,
        request(
            Method::POST,
            Some("delete"),
            Some(json!({"acknowledge":"irreversible"})),
        ),
    )
    .await;
    let replay = send(
        &router,
        request(
            Method::POST,
            Some("delete"),
            Some(json!({"acknowledge":"irreversible"})),
        ),
    )
    .await;
    assert_eq!(first, replay);
    assert_eq!(
        send(&router, request(Method::GET, None, None)).await,
        (StatusCode::OK, first.1)
    );
}

#[tokio::test]
async fn post_without_acknowledgement_is_422() {
    for body in [json!({}), json!({"acknowledge":"yes"})] {
        let (status, value) = send(
            &router(true),
            request(Method::POST, Some("delete"), Some(body)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(value["code"], "account_deletion_acknowledgement_required");
    }
}

#[tokio::test]
async fn post_with_binding_key_is_403() {
    let mut request = request(
        Method::POST,
        Some("delete"),
        Some(json!({"acknowledge":"irreversible"})),
    );
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseBindingConversations]));
    request
        .extensions_mut()
        .insert(crate::AuthMethod::BindingKey {
            binding_id: "binding".into(),
        });
    let (status, value) = send(&router(true), request).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["code"], "account_deletion_requires_owner_session");
}

#[tokio::test]
async fn post_when_disabled_is_503() {
    let (status, value) = send(
        &router(false),
        request(
            Method::POST,
            Some("delete"),
            Some(json!({"acknowledge":"irreversible"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["code"], "account_deletion_unavailable");
}

#[tokio::test]
async fn get_before_request_is_404() {
    let (status, value) = send(&router(true), request(Method::GET, None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(value["code"], "account_deletion_not_found");
}

#[tokio::test]
async fn request_requires_valid_key_and_authenticated_human() {
    let (status, value) = send(
        &router(true),
        request(
            Method::POST,
            None,
            Some(json!({"acknowledge":"irreversible"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["code"], "missing_idempotency_key");
    let mut request = request(Method::GET, None, None);
    request
        .extensions_mut()
        .remove::<crate::AuthenticatedSubject>();
    let (status, value) = send(&router(true), request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["code"], "not_authenticated");
}

#[tokio::test]
async fn resource_and_document_gates_return_typed_problems() {
    use axum::response::IntoResponse;
    use layer2_core::{
        document_artifacts::DocumentError,
        resource_access::{ResourceAccessError, ResourceAccessStoreError},
    };
    for (response, status, code) in [
        (
            super::resource_access_problem::ResourceProblem(
                ResourceAccessError::AccountDeletionInProgress,
            )
            .into_response(),
            StatusCode::FORBIDDEN,
            "account_deletion_in_progress",
        ),
        (
            super::resource_access_problem::ResourceProblem(ResourceAccessError::Store(
                ResourceAccessStoreError::ShareRecipientUnavailable,
            ))
            .into_response(),
            StatusCode::CONFLICT,
            "share_recipient_unavailable",
        ),
        (
            super::documents::DocumentProblem::from(DocumentError::AccountDeletionInProgress)
                .into_response(),
            StatusCode::FORBIDDEN,
            "account_deletion_in_progress",
        ),
        (
            super::documents::DocumentProblem::from(DocumentError::ShareRecipientUnavailable)
                .into_response(),
            StatusCode::CONFLICT,
            "share_recipient_unavailable",
        ),
    ] {
        assert_eq!(response.status(), status);
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["code"], code);
    }
}
#[test]
fn openapi_receipt_requires_nullable_completion_time() {
    let document = serde_json::to_value(super::account_deletion::document()).unwrap();
    let receipt = &document["components"]["schemas"]["DeletionReceipt"];
    assert!(
        receipt["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "completed_at"),
        "receipt schema: {receipt}"
    );
    assert_eq!(
        receipt["properties"]["completed_at"]["type"],
        json!(["string", "null"])
    );
    assert_eq!(
        receipt["properties"]["status"]["enum"],
        json!(["running", "blocked", "completed"])
    );
}

#[test]
fn openapi_request_requires_irreversible_acknowledgement() {
    let document = serde_json::to_value(super::account_deletion::document()).unwrap();
    let request = &document["components"]["schemas"]["DeletionRequest"];
    assert!(request["required"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| field == "acknowledge")));
    assert_eq!(
        request["properties"]["acknowledge"]["enum"],
        json!(["irreversible"])
    );
}

#[tokio::test]
async fn deletion_recovery_and_state_errors_have_actionable_distinct_details() {
    use layer2_core::account_deletion::AccountDeletionError;
    for (error, code, expected, forbidden) in [
        (
            AccountDeletionError::RecoveryRequired,
            "idempotency_recovery_required",
            "same",
            "manual review",
        ),
        (
            AccountDeletionError::Invalid,
            "store_failed",
            "state",
            "Idempotency-Key",
        ),
        (
            AccountDeletionError::VerificationRequired,
            "store_failed",
            "state",
            "Idempotency-Key",
        ),
        (
            AccountDeletionError::Store("database unavailable".into()),
            "store_failed",
            "temporarily unavailable",
            "Idempotency-Key",
        ),
    ] {
        let response = super::account_deletion::account_deletion_problem(error);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["code"], code);
        let detail = body["detail"].as_str().unwrap();
        assert!(detail.contains(expected), "{detail}");
        assert!(!detail.contains(forbidden), "{detail}");
    }
}
