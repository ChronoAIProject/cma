use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Source {
    hashes: std::sync::RwLock<BTreeSet<String>>,
    failed: AtomicBool,
    refresh_failed: AtomicBool,
    refreshes: AtomicUsize,
    point_reads: AtomicUsize,
}
#[async_trait::async_trait]
impl SnapshotSource for Source {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        if self.failed.load(Ordering::SeqCst) || self.refresh_failed.load(Ordering::SeqCst) {
            return Err(AccountDeletionError::Unavailable);
        }
        Ok(self.hashes.read().unwrap().clone())
    }
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.point_reads.fetch_add(1, Ordering::SeqCst);
        if self.failed.load(Ordering::SeqCst) {
            return Err(AccountDeletionError::Unavailable);
        }
        Ok(self.hashes.read().unwrap().contains(&subject_hash(subject)))
    }
}
fn fixture() -> (Arc<Source>, ErasingSnapshot, SubjectId) {
    let source = Arc::new(Source::default());
    let gate = ErasingSnapshot::with_source(source.clone());
    (source, gate, SubjectId::new("deleting-owner"))
}
#[tokio::test(start_paused = true)]
async fn startup_snapshot_success_uses_memory_and_refreshes_in_background() {
    let (source, gate, subject) = fixture();
    let gate = Arc::new(gate);
    let refreshing = gate.start().await;
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 1);
    assert!(!gate.erasing(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 0);
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 2);
    assert!(gate.erasing(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 0);
    refreshing.abort();
}
#[tokio::test(start_paused = true)]
async fn startup_snapshot_failure_keeps_point_reads_and_recovers_in_background() {
    let (source, gate, subject) = fixture();
    source.refresh_failed.store(true, Ordering::SeqCst);
    let gate = Arc::new(gate);
    let refreshing = gate.start().await;
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 1);
    assert!(!gate.erasing(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
    source.failed.store(true, Ordering::SeqCst);
    assert_eq!(
        gate.erasing(&subject).await,
        Err(AccountDeletionError::Unavailable)
    );
    source.failed.store(false, Ordering::SeqCst);
    source.refresh_failed.store(false, Ordering::SeqCst);
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 2);
    assert!(gate.erasing(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 2);
    refreshing.abort();
}
#[tokio::test]
async fn accepting_replica_denies_synchronously() {
    let (_, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    gate.insert_local(&subject_hash(&subject));
    assert!(gate.erasing(&subject).await.unwrap());
}
#[tokio::test]
async fn uncertain_local_denial_requires_successful_snapshot_reconciliation() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    gate.insert_local(&subject_hash(&subject));
    source.failed.store(true, Ordering::SeqCst);
    assert!(gate.refresh().await.is_err());
    assert!(gate.erasing(&subject).await.unwrap());

    source.failed.store(false, Ordering::SeqCst);
    gate.refresh().await.unwrap();
    assert!(!gate.erasing(&subject).await.unwrap());

    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    gate.insert_local(&subject_hash(&subject));
    gate.refresh().await.unwrap();
    assert!(gate.erasing(&subject).await.unwrap());
}
#[tokio::test]
async fn other_replica_denies_within_one_refresh() {
    let (source, first, subject) = fixture();
    let other = ErasingSnapshot::with_source(source.clone());
    first.refresh().await.unwrap();
    other.refresh().await.unwrap();
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    first.insert_local(&subject_hash(&subject));
    other.refresh().await.unwrap();
    assert!(other.erasing(&subject).await.unwrap());
}
#[tokio::test]
async fn stale_snapshot_falls_back_to_point_read() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    gate.state.write().unwrap().refreshed_at = Some(Instant::now() - Duration::from_secs(11));
    assert!(gate.erasing(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn stale_snapshot_authority_failure_never_allows() {
    let (source, gate, subject) = fixture();
    source.failed.store(true, Ordering::SeqCst);
    assert!(gate.erasing(&subject).await.is_err());
    assert!(gate.refresh().await.is_err());
    assert!(gate.erasing(&subject).await.is_err());
}
#[tokio::test]
async fn gate_adds_no_mongo_command_per_request() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    for _ in 0..100 {
        assert!(!gate.erasing(&subject).await.unwrap());
    }
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn other_owner_is_unaffected() {
    let (_, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    gate.insert_local(&subject_hash(&subject));
    assert!(!gate.erasing(&SubjectId::new("other-owner")).await.unwrap());
}
#[tokio::test]
async fn completed_deletion_does_not_gate() {
    let (source, gate, subject) = fixture();
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    gate.refresh().await.unwrap();
    assert!(gate.erasing(&subject).await.unwrap());
    source.hashes.write().unwrap().clear();
    gate.refresh().await.unwrap();
    assert!(!gate.erasing(&subject).await.unwrap());
}
#[tokio::test]
async fn authoritative_checks_bypass_a_fresh_snapshot() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    assert!(gate.authoritative(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
}
#[test]
fn status_and_logout_allowed_while_erasing() {
    for (method, path) in [
        ("GET", "/auth/session"),
        ("POST", "/auth/logout"),
        ("GET", "/auth/logout"),
        ("GET", "/api/v1/account/deletion"),
    ] {
        assert!(allowed_while_erasing(method, path));
    }
    for (method, path) in [
        ("POST", "/api/v1/account/deletion"),
        ("GET", "/api/v1/account/deletion/other"),
        ("POST", "/auth/session"),
        ("GET", "/api/v1/sandboxes"),
    ] {
        assert!(!allowed_while_erasing(method, path));
    }
}

#[tokio::test]
async fn standing_gate_denies_every_surface() {
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;
    let (_, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    gate.insert_local(&subject_hash(&subject));
    let auth = super::super::AuthState::in_memory(
        super::super::AuthMode::Trusted {
            subject: subject.to_string(),
        },
        None,
    )
    .with_account_standing(Arc::new(gate));
    let app = Router::new()
        .route(
            "/api/v1/sandboxes",
            get(|| async { "data" }).post(|| async { "write" }),
        )
        .route("/auth/session", get(|| async { "progress" }))
        .route("/auth/logout", get(|| async { "logout" }))
        .route(
            "/api/v1/account/deletion",
            get(|| async { "progress" }).post(|| async { "deletion" }),
        )
        .layer(axum::middleware::from_fn_with_state(
            auth,
            super::super::middleware::attach_subject,
        ));
    for method in ["GET", "POST"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/api/v1/sandboxes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], "account_deletion_in_progress");
    }
    for path in ["/auth/session", "/auth/logout", "/api/v1/account/deletion"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
    }
}
#[tokio::test]
async fn delayed_command_authorization_checks_current_standing() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    let auth = super::super::AuthState::in_memory(
        super::super::AuthMode::Trusted {
            subject: subject.to_string(),
        },
        None,
    )
    .with_account_standing(Arc::new(gate));
    assert_eq!(
        auth.require_command_account(&subject).await,
        Err(layer2_core::sandbox::agent_io::CommandAuthorizationError::Revoked)
    );
}

#[tokio::test]
async fn delayed_owner_and_actor_checks_bypass_a_fresh_snapshot() {
    use layer2_core::resource_access::{ResourceActor, ResourceOwner};
    for erasing in ["actor", "owner"] {
        let (source, gate, _) = fixture();
        gate.refresh().await.unwrap();
        source
            .hashes
            .write()
            .unwrap()
            .insert(subject_hash(&SubjectId::new(erasing)));
        let auth = super::super::AuthState::in_memory(
            super::super::AuthMode::Trusted {
                subject: "actor".into(),
            },
            None,
        )
        .with_account_standing(Arc::new(gate));
        assert_eq!(
            auth.require_actor_owner_accounts(
                &ResourceActor::Human {
                    subject: SubjectId::new("actor")
                },
                &ResourceOwner::Person {
                    subject: SubjectId::new("owner")
                }
            )
            .await,
            Err(layer2_core::sandbox::agent_io::CommandAuthorizationError::Revoked)
        );
    }
}

struct PausedRefresh {
    entered: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl SnapshotSource for PausedRefresh {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(BTreeSet::new())
    }
    async fn erasing(&self, _: &SubjectId) -> Result<bool, AccountDeletionError> {
        Ok(false)
    }
}
#[tokio::test]
async fn local_acceptance_survives_a_refresh_that_started_before_commit() {
    let source = Arc::new(PausedRefresh {
        entered: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let gate = Arc::new(ErasingSnapshot::with_source(source.clone()));
    let refreshing = {
        let gate = gate.clone();
        tokio::spawn(async move { gate.refresh().await })
    };
    source.entered.notified().await;
    gate.insert_local(&subject_hash(&SubjectId::new("owner")));
    source.resume.notify_one();
    refreshing.await.unwrap().unwrap();
    assert!(gate.erasing(&SubjectId::new("owner")).await.unwrap());
}

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
    let auth = super::super::AuthState::in_memory(
        super::super::AuthMode::Trusted {
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
            super::super::middleware::attach_subject,
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
        let auth = super::super::AuthState::in_memory(
            super::super::AuthMode::Trusted {
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
                super::super::middleware::attach_subject,
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
        let auth = super::super::AuthState::in_memory(
            super::super::AuthMode::Trusted {
                subject: "owner".into(),
            },
            None,
        )
        .with_account_deletion_enabled(enabled);
        let mut app = super::super::router(auth)
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
    let auth = super::super::AuthState::in_memory(
        super::super::AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    )
    .with_account_standing(standing.clone())
    .with_account_deletion_enabled(false);
    let service = Arc::new(AccountDeletionService::new(store, standing, false));
    let app = layer1_public::rest::account_deletion_router(service)
        .merge(super::super::router(auth.clone()))
        .layer(axum::middleware::from_fn_with_state(
            auth,
            super::super::middleware::attach_subject,
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
    use super::super::{profile::Profile, AuthMode, AuthState, RequestAuth};
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
    let store = Arc::new(super::super::persistence::MemoryAuthStore::default());
    store.set_unavailable(true);
    let auth = super::super::AuthState::with_store(
        super::super::AuthMode::Provider {
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
            super::super::middleware::attach_subject,
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
