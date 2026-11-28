use super::fixture::*;
use super::*;
use axum::{body::Body, http::Request};
use layer2_core::resource_access::*;
use std::sync::atomic::Ordering;
use tower::ServiceExt;

async fn call(
    router: &axum::Router,
    context: CallerContext,
    method: &str,
    suffix: &str,
) -> StatusCode {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/agents/agt-public/{suffix}"))
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(
            context.caller.subject.clone(),
        ));
    request
        .extensions_mut()
        .insert(context.caller.authorization.clone());
    request.extensions_mut().insert(context);
    router.clone().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn foreign_and_wrong_operation_callers_never_touch_ownership_or_effects() {
    let f = fixture(auth("foreign"), "agt-public", "owner").await;
    let (router, store) = router(&f).await;
    for (method, suffix) in [
        ("GET", "environment"),
        ("GET", "models"),
        ("POST", "skills"),
        ("POST", "files"),
        ("POST", "attachments"),
        ("GET", "browser"),
        ("POST", "recover-worktree"),
        ("PATCH", "preferences"),
        ("PATCH", "settings"),
    ] {
        let response = call(&router, context("foreign"), method, suffix).await;
        assert!(
            matches!(response, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{method} {suffix}: {response}"
        );
    }
    assert_eq!(
        call(&router, context("foreign"), "PATCH", "environment").await,
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn missing_expired_or_changed_principal_never_touches_ownership() {
    let f = fixture(auth("owner"), "agt-public", "owner").await;
    let (router, store) = router(&f).await;
    let mut expired = context("owner");
    expired.credential_source = CredentialSource::Independent {
        expires_at: Some(SystemTime::now() - std::time::Duration::from_secs(1)),
    };
    assert_eq!(
        call(&router, expired, "GET", "environment").await,
        StatusCode::UNAUTHORIZED
    );
    let mut restricted = context("owner");
    restricted.caller.authorization = layer2_core::Authorization::none();
    assert_eq!(
        call(&router, restricted, "GET", "models").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, context("other"), "GET", "environment").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
}

async fn share(f: &Fixture, mode: ShareMode) -> ResourceShare {
    let token = "c".repeat(64);
    let receipt = f
        .access
        .create_agent_share_link(
            &context("owner"),
            None,
            CreateAgentShareLink {
                agent_id: AgentId("agt-public".into()),
                audience: AgentShareAudience::Public,
                mode,
                token: token.clone(),
                idempotency_key: "ingress-link".into(),
            },
        )
        .await
        .unwrap();
    f.access
        .redeem_agent_share_link(&context("viewer"), None, &receipt.share.id, &token)
        .await
        .unwrap()
}
async fn authorize(
    f: &Fixture,
    context: &CallerContext,
    method: &str,
    suffix: &str,
) -> Result<(), PublicRefusal> {
    f.policy
        .authorize(PublicRequest {
            id: &AgentId("agt-public".into()),
            method: &method.parse().unwrap(),
            uri: &format!("/api/v1/agents/agt-public/{suffix}")
                .parse()
                .unwrap(),
            headers: &HeaderMap::new(),
            context,
            credential: None,
            browser_proof: None,
            peer_browser: None,
            capture_write_target: false,
        })
        .await
        .map(|_| ())
}

#[tokio::test]
async fn view_link_cannot_route_writes_and_revocation_precedes_owner_lookup() {
    let f = fixture(auth("viewer"), "agt-public", "owner").await;
    let share = share(&f, ShareMode::View).await;
    let viewer = context("viewer");
    for suffix in ["environment", "files/tree", "browser", "browser/control"] {
        authorize(&f, &viewer, "GET", suffix).await.unwrap();
    }
    let (router, store) = router(&f).await;
    for (method, suffix) in [
        ("POST", "files"),
        ("POST", "attachments"),
        ("PUT", "browser"),
        ("POST", "browser/control"),
        ("GET", "browser/control?operation_id=retained"),
        ("GET", "models"),
        ("PATCH", "preferences"),
        ("PATCH", "settings"),
    ] {
        assert!(
            call(&router, viewer.clone(), method, suffix)
                .await
                .is_client_error(),
            "{method} {suffix}"
        );
    }
    f.access
        .change_share(
            &context("owner"),
            None,
            ShareCommand {
                selector: share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-ingress".into(),
            },
        )
        .await
        .unwrap();
    assert!(call(&router, viewer, "GET", "environment")
        .await
        .is_client_error());
    for suffix in ["preferences", "settings"] {
        assert!(call(&router, context("viewer"), "PATCH", suffix)
            .await
            .is_client_error());
    }
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn accepted_write_link_routes_files_and_upload_but_not_browser_management() {
    let f = fixture(auth("viewer"), "agt-public", "owner").await;
    share(&f, ShareMode::ViewWrite).await;
    let viewer = context("viewer");
    for (method, suffix) in [
        ("POST", "files"),
        ("DELETE", "files"),
        ("POST", "attachments"),
        ("GET", "models"),
        ("PATCH", "preferences"),
        ("PATCH", "settings"),
    ] {
        authorize(&f, &viewer, method, suffix).await.unwrap();
    }
    assert!(authorize(&f, &viewer, "PUT", "browser").await.is_err());
    assert!(authorize(&f, &viewer, "POST", "skills").await.is_err());
}

#[tokio::test]
async fn exact_read_proof_keeps_read_scope_without_ambient_owner_authority() {
    let f = fixture(auth("unrelated-ambient"), "agt-public", "owner").await;
    let mut proof = context("owner");
    proof.credential_source = CredentialSource::RequestBoundRead {
        expires_at: SystemTime::now() + std::time::Duration::from_secs(10),
    };
    authorize(&f, &proof, "GET", "environment").await.unwrap();
    assert!(authorize(&f, &proof, "POST", "files").await.is_err());
    assert!(authorize(&f, &proof, "PATCH", "preferences").await.is_err());
    assert!(authorize(&f, &proof, "PATCH", "settings").await.is_err());
    proof.credential_source = CredentialSource::RequestBoundRead {
        expires_at: SystemTime::now() - std::time::Duration::from_secs(1),
    };
    let (router, store) = router(&f).await;
    assert_eq!(
        call(&router, proof, "GET", "environment").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owner_preflight_preserves_head_skills_post_and_observer_release() {
    let f = fixture(auth("owner"), "agt-public", "owner").await;
    for (method, suffix) in [
        ("HEAD", "environment"),
        ("HEAD", "models"),
        ("HEAD", "files/content"),
        ("POST", "skills"),
        ("POST", "recover-worktree"),
        ("DELETE", "browser"),
        ("GET", "browser/control?operation_id=one"),
        ("PATCH", "preferences"),
        ("PATCH", "settings"),
    ] {
        authorize(&f, &context("owner"), method, suffix)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn settings_and_preferences_require_current_mutation_authority_for_exact_routes() {
    let f = fixture(auth("owner"), "agt-public", "owner").await;
    let (router, ownership) = router(&f).await;
    for suffix in ["preferences", "settings"] {
        authorize(&f, &context("owner"), "PATCH", suffix)
            .await
            .unwrap();
        for method in [
            "GET", "HEAD", "POST", "PUT", "DELETE", "OPTIONS", "TRACE", "CONNECT", "CUSTOM",
        ] {
            assert!(
                authorize(&f, &context("owner"), method, suffix)
                    .await
                    .is_err(),
                "{method} {suffix}"
            );
        }
        assert!(
            authorize(&f, &context("owner"), "PATCH", &format!("{suffix}/extra"))
                .await
                .is_err()
        );
        let mut expired = context("owner");
        expired.credential_source = CredentialSource::Independent {
            expires_at: Some(SystemTime::now() - std::time::Duration::from_secs(1)),
        };
        let mut restricted = context("owner");
        restricted.caller.authorization = layer2_core::Authorization::none();
        for caller in [expired, restricted, context("other")] {
            assert_eq!(
                call(&router, caller, "PATCH", suffix).await,
                StatusCode::UNAUTHORIZED
            );
        }
    }
    assert_eq!(ownership.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn revoked_write_share_cannot_route_preferences() {
    let f = fixture(auth("viewer"), "agt-public", "owner").await;
    let share = share(&f, ShareMode::ViewWrite).await;
    for suffix in ["preferences", "settings"] {
        authorize(&f, &context("viewer"), "PATCH", suffix)
            .await
            .unwrap();
    }
    f.access
        .change_share(
            &context("owner"),
            None,
            ShareCommand {
                selector: share.selector.clone(),
                change: ShareChange::Revoke {
                    share_id: share.id.clone(),
                },
                expected_share_revision: Some(share.revision),
                idempotency_key: "revoke-preferences".into(),
            },
        )
        .await
        .unwrap();
    let (router, ownership) = router(&f).await;
    for suffix in ["preferences", "settings"] {
        assert!(call(&router, context("viewer"), "PATCH", suffix)
            .await
            .is_client_error());
    }
    assert_eq!(ownership.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn settings_mutation_refuses_a_read_only_or_revoked_share_before_lookup() {
    for mode in [ShareMode::View, ShareMode::ViewWrite] {
        let f = fixture(auth("viewer"), "agt-public", "owner").await;
        let share = share(&f, mode).await;
        let viewer = context("viewer");
        if mode == ShareMode::ViewWrite {
            authorize(&f, &viewer, "PATCH", "settings").await.unwrap();
            f.access
                .change_share(
                    &context("owner"),
                    None,
                    ShareCommand {
                        selector: share.selector.clone(),
                        change: ShareChange::Revoke { share_id: share.id },
                        expected_share_revision: Some(share.revision),
                        idempotency_key: "revoke-settings".into(),
                    },
                )
                .await
                .unwrap();
        }
        assert!(authorize(&f, &viewer, "PATCH", "settings").await.is_err());
    }
    let f = fixture(auth("owner"), "agt-public", "owner").await;
    let mut proof = context("owner");
    proof.credential_source = CredentialSource::RequestBoundRead {
        expires_at: SystemTime::now() + std::time::Duration::from_secs(10),
    };
    assert!(authorize(&f, &proof, "PATCH", "settings").await.is_err());
}
