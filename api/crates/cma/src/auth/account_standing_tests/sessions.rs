use super::*;

struct ReplayStanding;
#[async_trait::async_trait]
impl AccountStanding for ReplayStanding {
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(true)
    }
}
#[tokio::test]
async fn deletion_request_reaches_handler_for_every_personal_request_while_erasing() {
    use axum::{body::Body, http::Request, routing::post, Router};
    use tower::ServiceExt;
    let auth = crate::auth::AuthState::in_memory(
        crate::auth::AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    )
    .with_account_standing(Arc::new(ReplayStanding));
    let app = Router::new()
        .route(
            "/api/v1/account/deletion",
            post(|| async { axum::http::StatusCode::ACCEPTED }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::middleware::attach_subject,
        ));
    for (key, expected) in [
        (Some("accepted-request"), 202),
        (Some("new-request"), 202),
        (None, 202),
    ] {
        let mut request = Request::post("/api/v1/account/deletion");
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            expected
        );
    }
}

#[tokio::test]
async fn personal_account_authority_never_enters_generic_authorization() {
    use axum::{body::Body, http::Request, routing::get, Extension, Json, Router};
    use layer2_core::Authorization;
    use tower::ServiceExt;
    for enabled in [false, true] {
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: "owner".into(),
            },
            None,
        )
        .with_account_deletion_enabled(enabled);
        let original = auth
            .resolve_request(&Default::default())
            .await
            .unwrap()
            .unwrap();
        let expected =
            serde_json::to_value(original.authorization.iter().collect::<Vec<_>>()).unwrap();
        let app = Router::new()
            .route(
                "/api/v1/authorization-fixture",
                get(
                    |Extension(authorization): Extension<Authorization>| async move {
                        Json(authorization.iter().collect::<Vec<_>>())
                    },
                ),
            )
            .layer(axum::middleware::from_fn_with_state(
                auth,
                crate::auth::middleware::attach_subject,
            ));
        let response = app
            .oneshot(
                Request::get("/api/v1/authorization-fixture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let actual: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            actual, expected,
            "personal deletion authority must not alter verified grants"
        );
    }
}

#[tokio::test]
async fn session_deletion_entitlement_requires_subject_bound_marker() {
    use axum::{body::Body, http::Request, Extension};
    use layer2_core::{account_deletion::OwnerSessionAuthority, Authorization, Capability};
    use tower::ServiceExt;
    for (enabled, marker_subject, expected) in [
        (true, None, false),
        (true, Some("other-owner"), false),
        (true, Some("owner"), true),
        (false, Some("owner"), false),
    ] {
        let auth = crate::auth::AuthState::in_memory(
            crate::auth::AuthMode::Trusted {
                subject: "owner".into(),
            },
            None,
        )
        .with_account_deletion_enabled(enabled);
        let mut app = crate::auth::router(auth)
            .layer(Extension(layer1_public::AuthenticatedSubject(
                SubjectId::new("owner"),
            )))
            .layer(Extension(Authorization::new([Capability::UseOwnSandboxes])));
        if let Some(subject) = marker_subject {
            app = app.layer(Extension(OwnerSessionAuthority::verified_personal(
                SubjectId::new(subject),
            )));
        }
        let response = app
            .oneshot(Request::get("/auth/session").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|capability| capability == "delete_own_account"),
            expected,
            "the entitlement requires enabled personal authority for this caller"
        );
    }
}

#[tokio::test]
async fn status_remains_readable_when_request_feature_is_disabled_and_session_omits_entitlement() {
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use layer2_core::account_deletion::{
        AccountDeletionService, AccountDeletionStore, MemoryAccountDeletionStore,
    };
    use tower::ServiceExt;
    let store = Arc::new(MemoryAccountDeletionStore::default());
    let deletion = store
        .request(&SubjectId::new("owner"), "delete", "2026-11-14T00:00:00Z")
        .await
        .unwrap();
    let standing = Arc::new(ReplayStanding);
    let auth = crate::auth::AuthState::in_memory(
        crate::auth::AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    )
    .with_account_standing(standing.clone())
    .with_account_deletion_enabled(false);
    let service = Arc::new(AccountDeletionService::new(store, standing, false));
    let app = layer1_public::rest::account_deletion_router(service)
        .merge(crate::auth::router(auth.clone()))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::middleware::attach_subject,
        ));
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/account/deletion")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let receipt: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(receipt["id"], deletion.id.as_str());
    let session = app
        .oneshot(Request::get("/auth/session").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(session.status(), 200);
    let session: serde_json::Value =
        serde_json::from_slice(&to_bytes(session.into_body(), 16384).await.unwrap()).unwrap();
    assert!(!session["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value == "delete_own_account"));
}

#[test]
fn delete_authority_is_only_marked_for_verified_personal_credentials() {
    use crate::auth::{profile::Profile, AuthMode, AuthState, RequestAuth};
    use base64::Engine as _;
    use layer2_core::{resource_access::CredentialSource, Authorization, Capability};
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    );
    let service_token = format!(
        "e30.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sa":true}"#)
    );
    let delegated_token = format!(
        "e30.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"act":{"sub":"agent"}}"#)
    );
    for (source, token, expected) in [
        (CredentialSource::Retained, None, true),
        (
            CredentialSource::Independent { expires_at: None },
            Some("ordinary-user".to_owned()),
            true,
        ),
        (
            CredentialSource::Independent { expires_at: None },
            Some("nyx_agent-key".to_owned()),
            false,
        ),
        (
            CredentialSource::Independent { expires_at: None },
            Some(service_token),
            false,
        ),
        (
            CredentialSource::Independent { expires_at: None },
            Some(delegated_token),
            false,
        ),
        (CredentialSource::Binding, None, false),
        (
            CredentialSource::delegated("grant"),
            Some("ordinary-user".to_owned()),
            false,
        ),
        (
            CredentialSource::IdentityProxy {
                grant_digest: [0; 32],
            },
            Some("ordinary-user".to_owned()),
            false,
        ),
        (
            CredentialSource::RequestBoundRead {
                expires_at: std::time::SystemTime::now(),
            },
            None,
            false,
        ),
    ] {
        let request = RequestAuth {
            binding: None,
            credential_source: source,
            subject: SubjectId::new("owner"),
            authorization: Authorization::new([Capability::UseOwnSandboxes]),
            profile: Profile::default(),
            delegated_credential: token,
        };
        let marker = auth.owner_session_authority(&request);
        assert_eq!(
            marker.as_ref().map(|authority| authority.subject()),
            expected.then_some(&request.subject)
        );
        assert_eq!(
            request.authorization,
            Authorization::new([Capability::UseOwnSandboxes])
        );
    }
}

#[tokio::test]
async fn account_route_authentication_failures_are_private() {
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;
    let store = Arc::new(crate::auth::persistence::MemoryAuthStore::default());
    store.set_unavailable(true);
    let auth = crate::auth::AuthState::with_store(
        crate::auth::AuthMode::Provider {
            base_url: "https://identity.invalid".into(),
            client_id: "client".into(),
            client_secret: "fixture-value".into(),
            public_url: "https://cma.example".into(),
        },
        None,
        store,
    );
    let app = Router::new()
        .route(
            "/api/v1/account/deletion",
            get(|| async { "status" }).post(|| async { "request" }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::auth::middleware::attach_subject,
        ));
    for method in ["GET", "POST"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/v1/account/deletion")
                    .header("cookie", "cma_session=unavailable")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
    }
}
