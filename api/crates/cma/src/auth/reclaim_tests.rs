//! Interaction between explicit revoke debt and provider-discovered orphans.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{delete, get},
    Json, Router,
};
use layer2_core::{
    AgentConfiguration, Baseline, Deployment, EnvironmentProfile, EnvironmentProfileId,
    EnvironmentProfileService, EnvironmentProfileSpec, EnvironmentProfileStore, Event, Page,
    PageRequest, PlatformSettingsService, Reserved, ScreenFactory, ScreenModel, ScreenSnapshot,
    SessionId, SessionListQuery, SessionPorts, SessionRecord, SessionService, SessionStore,
    StoreError, StoredEvent, SubjectId, TerminalGeometry,
};
use tokio::sync::{Mutex, Semaphore};

use super::{queue_key, retry_tokens};
use crate::auth::{
    keys::{key_name, KeyIssuer},
    AuthMode, AuthState, Issued,
};

#[derive(Clone)]
struct Platform {
    key_id: String,
    key_name: String,
    delete_statuses: Arc<Mutex<VecDeque<StatusCode>>>,
    delete_attempts: Arc<Mutex<Vec<String>>>,
    listings: Arc<AtomicUsize>,
}

fn assert_owner(headers: &HeaderMap) {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer retry-owner-token")
    );
}

async fn list_keys(
    State(platform): State<Platform>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    assert_owner(&headers);
    platform.listings.fetch_add(1, Ordering::SeqCst);
    Json(serde_json::json!({
        "keys": [{ "id": platform.key_id, "name": platform.key_name }]
    }))
}

async fn delete_key(
    State(platform): State<Platform>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    assert_owner(&headers);
    platform.delete_attempts.lock().await.push(id);
    platform
        .delete_statuses
        .lock()
        .await
        .pop_front()
        .expect("only the two expected DELETE attempts occur")
}

async fn platform(session: &SessionId) -> (String, Platform, tokio::task::JoinHandle<()>) {
    let platform = Platform {
        key_id: "owed-key".into(),
        key_name: key_name("production", session),
        delete_statuses: Arc::new(Mutex::new(
            [StatusCode::SERVICE_UNAVAILABLE, StatusCode::NO_CONTENT].into(),
        )),
        delete_attempts: Arc::new(Mutex::new(Vec::new())),
        listings: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/api-keys", get(list_keys))
        .route("/api/v1/api-keys/{id}", delete(delete_key))
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock identity platform");
    let base = format!("http://{}", listener.local_addr().expect("mock address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve mock identity platform");
    });
    (base, platform, server)
}

#[derive(Clone)]
struct ClaimGate {
    started: Arc<Semaphore>,
    finish: Arc<Semaphore>,
}

impl ClaimGate {
    fn new() -> Self {
        Self {
            started: Arc::new(Semaphore::new(0)),
            finish: Arc::new(Semaphore::new(0)),
        }
    }
}

#[derive(Default)]
struct NoSessions {
    claim_gate: Option<ClaimGate>,
}

#[async_trait::async_trait]
impl SessionStore for NoSessions {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(&self, _id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        if let Some(gate) = self.claim_gate.as_ref() {
            // Reclamation reaches the store only after it has fetched and
            // decoded the provider listing, which is the boundary this test
            // needs to hold while release transfers ownership of the key.
            gate.started.add_permits(1);
            gate.finish
                .acquire()
                .await
                .expect("the test still owns the claim gate")
                .forget();
        }
        Ok(None)
    }

    async fn list_for(
        &self,
        _owner: &SubjectId,
        _query: &SessionListQuery,
    ) -> Result<Page<SessionRecord>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        Ok(Reserved::default())
    }

    async fn list_all(&self, _query: &SessionListQuery) -> Result<Page<SessionRecord>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
        Ok(())
    }

    async fn append_events(&self, _events: &[Event]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn commit_session_idempotent(
        &self,
        _write_id: &str,
        record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        Ok(record.revision.saturating_add(1))
    }

    async fn create_session_idempotent(
        &self,
        _write_id: &str,
        record: &SessionRecord,
        _events: &[Event],
    ) -> Result<u64, StoreError> {
        Ok(record.revision.saturating_add(1))
    }

    async fn events_since(
        &self,
        _id: &SessionId,
        _after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        Ok(Vec::new())
    }
}

struct NoProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for NoProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        Ok(())
    }

    async fn replace_content(
        &self,
        _id: &EnvironmentProfileId,
        _content: &EnvironmentProfileSpec,
        _updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(
        &self,
        _owner: &SubjectId,
        _page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        Ok(Page {
            items: Vec::new(),
            next: None,
        })
    }

    async fn set_preferred(
        &self,
        _owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        if chosen.is_some() {
            return Err(StoreError::Malformed(
                "an empty store has no selectable profile".into(),
            ));
        }
        Ok(())
    }

    async fn preferred_for(
        &self,
        _owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct EmptyScreen(TerminalGeometry);

impl ScreenModel for EmptyScreen {
    fn apply(&mut self, _bytes: &[u8]) {}

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.0 = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.0,
            rows: Vec::new(),
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn session_service(store: Arc<dyn SessionStore>) -> Arc<SessionService> {
    let screens: ScreenFactory =
        Arc::new(|geometry| Box::new(EmptyScreen(geometry)) as Box<dyn ScreenModel>);
    let environments = EnvironmentProfileService::new(
        Arc::new(NoProfiles),
        Baseline::default(),
        None,
        None,
        AgentConfiguration::default(),
        Arc::new(PlatformSettingsService::unconfigured()),
    );
    SessionService::new(
        HashMap::new(),
        screens,
        store,
        SessionPorts::default(),
        environments,
        Deployment::default(),
    )
}

pub(super) fn empty_session_service() -> Arc<SessionService> {
    session_service(Arc::new(NoSessions::default()))
}

async fn auth(base: String) -> Arc<AuthState> {
    let auth = AuthState::new(
        AuthMode::Trusted {
            subject: "local".into(),
        },
        None,
    );
    auth.use_issuer(Arc::new(KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "production",
    )))
    .await;
    auth
}

#[tokio::test]
async fn a_failed_orphan_revoke_is_not_counted_as_reclaimed() {
    let session = SessionId("sesn_orphaned".into());
    let (base, platform, server) = platform(&session).await;
    let auth = auth(base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;

    assert_eq!(
        auth.reclaim_orphaned_credentials(&empty_session_service())
            .await,
        0
    );
    assert_eq!(platform.listings.load(Ordering::SeqCst), 1);
    assert_eq!(
        platform.delete_attempts.lock().await.as_slice(),
        ["owed-key"]
    );
    server.abort();
}

#[tokio::test]
async fn orphan_reclaim_does_not_steal_a_failed_superseded_retry() {
    let session = SessionId("sesn_ended".into());
    let (base, platform, server) = platform(&session).await;
    let auth = auth(base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    queue_key(&auth, &platform.key_id, &owner).await;

    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(auth.superseded.read().await.len(), 1);
    assert_eq!(platform.delete_attempts.lock().await.len(), 1);

    assert_eq!(
        auth.reclaim_orphaned_credentials(&empty_session_service())
            .await,
        0
    );
    assert_eq!(platform.listings.load(Ordering::SeqCst), 1);
    assert_eq!(
        platform.delete_attempts.lock().await.len(),
        1,
        "the orphan sweep must leave explicit revoke debt to its owner"
    );
    assert_eq!(auth.superseded.read().await.len(), 1);

    assert_eq!(auth.take_back_superseded().await, 1);
    assert!(auth.superseded.read().await.is_empty());
    assert_eq!(
        platform.delete_attempts.lock().await.as_slice(),
        ["owed-key", "owed-key"]
    );
    server.abort();
}

#[tokio::test]
async fn repeated_release_does_not_bypass_or_strand_queued_retry_debt() {
    let session = SessionId("sesn_repeated_release".into());
    let (base, platform, server) = platform(&session).await;
    let auth = auth(base).await;
    let owner = SubjectId("retry-owner".into());
    auth.issued.write().await.insert(
        session.clone(),
        Issued {
            id: platform.key_id.clone(),
            value: "session-key-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );

    layer2_core::CredentialSource::release(&*auth, &session).await;
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [(platform.key_id.clone(), owner.clone())]
    );
    assert!(platform.delete_attempts.lock().await.is_empty());

    auth.store_tokens(&owner, retry_tokens()).await;
    layer2_core::CredentialSource::release(&*auth, &session).await;
    assert_eq!(platform.listings.load(Ordering::SeqCst), 1);
    assert!(
        platform.delete_attempts.lock().await.is_empty(),
        "fallback release must not DELETE a key the retry queue owns"
    );
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [(platform.key_id.clone(), owner)]
    );

    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(auth.superseded.read().await.len(), 1);
    assert_eq!(auth.take_back_superseded().await, 1);
    assert!(auth.superseded.read().await.is_empty());
    assert_eq!(
        platform.delete_attempts.lock().await.as_slice(),
        ["owed-key", "owed-key"]
    );
    server.abort();
}

#[tokio::test]
async fn cancelling_tokenless_release_still_completes_and_discharges_retry_debt() {
    let session = SessionId("sesn_release_gap".into());
    let (base, platform, server) = platform(&session).await;
    let auth = auth(base).await;
    let owner = SubjectId("retry-owner".into());
    auth.store_tokens(&owner, retry_tokens()).await;
    auth.issued.write().await.insert(
        session.clone(),
        Issued {
            id: platform.key_id.clone(),
            value: "session-key-value".into(),
            owner: owner.clone(),
            minted_at: std::time::Instant::now(),
        },
    );

    let claims = ClaimGate::new();
    let service = session_service(Arc::new(NoSessions {
        claim_gate: Some(claims.clone()),
    }));
    let mut orphan = tokio::spawn({
        let auth = Arc::clone(&auth);
        async move { auth.reclaim_orphaned_credentials(&service).await }
    });
    claims
        .started
        .acquire()
        .await
        .expect("the orphan sweep reaches the already-fetched listing")
        .forget();

    // Release can no longer revoke immediately, while the orphan sweep retains
    // the token it cloned before asking the provider for this listing.
    auth.tokens.write().await.remove(&owner);
    let queue_gate = auth.superseded.write().await;
    let release = tokio::spawn({
        let auth = Arc::clone(&auth);
        let session = session.clone();
        async move { layer2_core::CredentialSource::release(&*auth, &session).await }
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if auth.issued.try_read().is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("release holds issued while it waits to enqueue the key");
    release.abort();
    assert!(release
        .await
        .expect_err("the release caller was cancelled")
        .is_cancelled());

    claims.finish.add_permits(1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut orphan)
            .await
            .is_err(),
        "the orphan sweep must wait until the ownership transfer is complete"
    );
    assert!(
        auth.issued.try_read().is_err(),
        "the key must remain protected by issued while queue insertion is blocked"
    );
    assert!(platform.delete_attempts.lock().await.is_empty());

    drop(queue_gate);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if !auth.issued.read().await.contains_key(&session)
                && auth.superseded.read().await.as_slice()
                    == [(platform.key_id.clone(), owner.clone())]
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached transfer finishes after its caller is cancelled");

    assert_eq!(orphan.await.expect("the orphan sweep completes"), 0);
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [(platform.key_id.clone(), owner.clone())]
    );
    assert!(platform.delete_attempts.lock().await.is_empty());

    let progress = auth.reissues.read().await;
    assert!(progress.pending.is_empty());
    assert!(progress.minting.is_empty());
    assert!(progress.retiring.is_empty());
    drop(progress);

    auth.store_tokens(&owner, retry_tokens()).await;
    assert_eq!(auth.take_back_superseded().await, 0);
    assert_eq!(
        auth.superseded.read().await.as_slice(),
        [(platform.key_id.clone(), owner)]
    );
    assert_eq!(auth.take_back_superseded().await, 1);
    assert!(auth.superseded.read().await.is_empty());
    assert_eq!(
        platform.delete_attempts.lock().await.as_slice(),
        ["owed-key", "owed-key"]
    );
    server.abort();
}
