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
async fn startup_snapshot_caches_denials_and_refreshes_in_background() {
    let (source, gate, subject) = fixture();
    let gate = Arc::new(gate);
    let refreshing = gate.start().await;
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 1);
    assert!(!gate.erasing(&subject).await.unwrap());
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
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
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 1);
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
async fn other_replica_denies_before_another_refresh() {
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
async fn every_allow_requires_its_own_point_read() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    for _ in 0..100 {
        assert!(!gate.erasing(&subject).await.unwrap());
    }
    assert_eq!(source.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(source.point_reads.load(Ordering::SeqCst), 100);
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

#[tokio::test]
async fn fresh_negative_snapshot_cannot_authorize_after_other_replica_accepts() {
    let (source, accepting, subject) = fixture();
    let observing = ErasingSnapshot::with_source(source.clone());
    accepting.refresh().await.unwrap();
    observing.refresh().await.unwrap();
    assert!(!observing.erasing(&subject).await.unwrap());
    source
        .hashes
        .write()
        .unwrap()
        .insert(subject_hash(&subject));
    accepting.insert_local(&subject_hash(&subject));
    assert!(observing.erasing(&subject).await.unwrap());
}

#[tokio::test]
async fn fresh_negative_snapshot_cannot_mask_authority_unavailability() {
    let (source, gate, subject) = fixture();
    gate.refresh().await.unwrap();
    source.failed.store(true, Ordering::SeqCst);
    assert_eq!(
        gate.erasing(&subject).await,
        Err(AccountDeletionError::Unavailable)
    );
}

#[path = "account_standing_tests/sessions.rs"]
mod sessions;
