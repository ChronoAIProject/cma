use super::*;
use crate::auth::{
    identity_retry,
    verification_error::{ProviderVerificationError, VerificationCause},
};
use layer2_core::diagnostic_context::DiagnosticContext;
use tracing::Instrument;

mod capture;
mod consumption;
mod selected_errors;
use capture::{capture, context, fields};

async fn choices(f: &Fixture) -> Result<Vec<OrganizationChoice>, MembershipError> {
    f.auth
        .organization_choices(&caller(), "fixture-credential")
        .await
}

async fn response(f: &Fixture) -> (StatusCode, Value) {
    use axum::{body::Body, http::Request};
    use layer1_public::{AuthenticatedSubject, DelegatedCredential};
    use layer2_core::{
        resource_access::ResourceAccessService, sandbox::store::InMemorySandboxStore,
    };
    use tower::ServiceExt;
    let router = layer1_public::rest::resource_access_router(Arc::new(ResourceAccessService::new(
        Arc::new(InMemorySandboxStore::default()),
        f.auth.clone(),
    )));
    let resolved = crate::auth::RequestAuth {
        binding: None,
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        subject: caller().subject,
        authorization: caller().authorization,
        profile: crate::auth::profile::Profile::default(),
        delegated_credential: Some("fixture-credential".into()),
    };
    let context = f.auth.resource_request_context(&resolved);
    let mut request = Request::builder()
        .uri("/api/v1/organizations")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(context.caller.subject.clone()));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(DelegatedCredential::new("fixture-credential".into()));
    let result = router.oneshot(request).await.unwrap();
    let status = result.status();
    assert_eq!(
        result.headers()[header::CONTENT_TYPE],
        if status.is_success() {
            "application/json"
        } else {
            "application/problem+json"
        }
    );
    let body = axum::body::to_bytes(result.into_body(), 8192)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn shared_publication_failure_is_recorded_in_each_waiters_context() {
    use futures_util::future::join_all;
    let f = Fixture::new().await;
    let gate = f.block_verification().await;
    let (results, events) = capture(async {
        let requests =
            join_all((0..8).map(|i| context(&format!("sha256:fixture-{i}")).scope(choices(&f))));
        tokio::pin!(requests);
        assert!(futures_util::poll!(&mut requests).is_pending());
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::select! {
                _ = &mut requests => panic!("verification must be held"),
                arrival = f.control.verification_arrivals.acquire() => arrival.unwrap().forget(),
            }
        })
        .await
        .unwrap();
        // Invalidation is known; its production cause must not be invented.
        f.auth.identity_store.invalidate_token("fixture-credential");
        gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(2), requests)
            .await
            .unwrap()
    })
    .await;
    assert!(results
        .iter()
        .all(|result| result == &Err(MembershipError::Unavailable)));
    assert_eq!(events.len(), 8);
    let mut hashes = std::collections::BTreeSet::new();
    for event in events {
        let field = &event["fields"];
        assert_eq!(field["phase"], "principal_verification");
        assert_eq!(field["reason"], "publication_epoch_changed");
        assert_eq!(field["outcome"], "unavailable");
        assert_eq!(field["result"], "unavailable");
        hashes.insert(field["request_hash"].as_str().unwrap().to_owned());
    }
    assert_eq!(
        hashes,
        (0..8).map(|i| format!("sha256:fixture-{i}")).collect()
    );
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
    *f.control.verification_gate.write().await = None;
    assert!(
        choices(&f).await.is_ok(),
        "later callers obtain fresh verification"
    );
}

#[tokio::test]
async fn actual_http_success_refusal_and_unavailability_keep_their_projection() {
    for case in ["success", "subject", "inactive", "metadata", "verification"] {
        let f = Fixture::new().await;
        match case {
            "subject" => *f.control.subject.write().await = "different-fixture-subject".into(),
            "inactive" => {
                f.block_verification().await.add_permits(1);
                *f.control.introspection.write().await = json!({"active":false});
            }
            "metadata" => *f.auth.metadata.write().await = None,
            "verification" => {
                f.auth
                    .metadata
                    .write()
                    .await
                    .as_mut()
                    .unwrap()
                    .userinfo_endpoint = None
            }
            _ => {}
        }
        let ((status, body), events) =
            capture(context("sha256:http-fixture").scope(response(&f))).await;
        if case == "success" {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["items"][0]["id"], "org-a");
            assert!(events.is_empty());
        } else {
            let (phase, reason) = match case {
                "subject" => ("principal_subject", "subject_mismatch"),
                "inactive" => ("principal_verification", "rejected"),
                "metadata" => ("principal_metadata", "unknown"),
                _ => ("principal_verification", "unknown"),
            };
            let field = fields(&events, phase, reason);
            assert_eq!(field["request_hash"], "sha256:http-fixture");
            assert_eq!(
                status,
                if case == "subject" {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            );
            assert_eq!(
                body["code"],
                if case == "subject" {
                    "resource_forbidden"
                } else {
                    "organization_authority_unavailable"
                }
            );
            assert_eq!(
                body["detail"],
                if case == "subject" {
                    "Current authority does not permit this operation."
                } else {
                    "Current organization authorization is unavailable. Retry after the identity provider recovers."
                }
            );
            assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn publication_failure_after_provider_success_preserves_http_503() {
    let f = Fixture::new().await;
    let gate = f.block_verification().await;
    let ((status, body), events) = capture(context("sha256:publication-http").scope(async {
        let request = response(&f);
        tokio::pin!(request);
        tokio::select! {
            _ = &mut request => panic!("provider must be held"),
            arrival = f.control.verification_arrivals.acquire() => arrival.unwrap().forget(),
        }
        f.auth.identity_store.invalidate_token("fixture-credential");
        gate.add_permits(1);
        request.await
    }))
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    fields(
        &events,
        "principal_verification",
        "publication_epoch_changed",
    );
    assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn private_ancestors_never_enter_principal_diagnostics() {
    let f = Fixture::new().await;
    *f.control.subject.write().await = "must-not-leak-subject".into();
    let (result, events) = capture(context("sha256:privacy-fixture").scope(async {
        choices(&f)
            .instrument(tracing::info_span!(
                "private-parent",
                private = "must-not-leak-ancestor"
            ))
            .await
    }))
    .await;
    assert_eq!(result, Err(MembershipError::NotMember));
    fields(&events, "principal_subject", "subject_mismatch");
    let (result, text) = capture::disabled_root(async {
        choices(&f)
            .instrument(tracing::info_span!(
                "private-parent",
                private = "must-not-leak-ancestor"
            ))
            .await
    })
    .await;
    assert_eq!(result, Err(MembershipError::NotMember));
    assert!(!text.contains("organization_principal_failure"), "{text}");
}

#[tokio::test]
async fn already_expired_receipt_is_rejected_after_successful_provider_responses() {
    let f = Fixture::new().await;
    let metadata = f.auth.metadata().await.unwrap();
    let result = identity_retry::verify_detailed(
        &f.auth,
        Some(&metadata),
        "fixture-credential",
        Some(SystemTime::UNIX_EPOCH),
        None,
        Duration::from_secs(2),
    )
    .await;
    let failure = result.err().unwrap();
    assert_eq!(failure.cause, VerificationCause::ReceiptExpired);
    assert!(matches!(
        failure.error,
        ProviderVerificationError::Rejected(_)
    ));
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
}
