//! Durable revoke debt and reconciliation failure boundaries.

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::{Duration, SystemTime};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{delete, get},
    Json, Router,
};
use layer2_core::{
    AgentConfiguration, Baseline, CredentialSource as _, Deployment, EnvironmentProfile,
    EnvironmentProfileId, EnvironmentProfileService, EnvironmentProfileSpec,
    EnvironmentProfileStore, Event, Page, PageRequest, PlatformSettingsService, Reserved,
    ScreenFactory, ScreenModel, ScreenSnapshot, SessionId, SessionListQuery, SessionPorts,
    SessionRecord, SessionService, SessionStore, StoreError, StoredEvent, SubjectId,
    TerminalGeometry,
};

use super::*;
use crate::auth::keys::KeyIssuer;
use crate::auth::persistence::MemoryAuthStore;
use crate::auth::{AuthMode, SubjectTokens};

fn auth(store: Arc<MemoryAuthStore>) -> Arc<AuthState> {
    AuthState::with_store(
        AuthMode::Trusted {
            subject: "local".into(),
        },
        None,
        store,
    )
}

async fn store_owner_token(auth: &AuthState, owner: &SubjectId) {
    auth.store_tokens(
        owner,
        SubjectTokens {
            access_token: "owner-access".into(),
            refresh_token: None,
            expires_at: std::time::Instant::now() + Duration::from_secs(60),
        },
    )
    .await
    .expect("owner token persists");
}

fn current_state(session: &SessionId, owner: &SubjectId) -> SessionCredentialState {
    SessionCredentialState {
        session: session.clone(),
        generation: 0,
        current: Some(DurableCredential {
            id: "provider-key".into(),
            value: SecretValue::new("session-key".into()),
            owner: owner.clone(),
            issued_at: SystemTime::now(),
            expires_at: None,
            scope_fingerprint: "scope".into(),
        }),
        pending: None,
        revoke_debt: Vec::new(),
        lease: None,
        retired: false,
    }
}

fn debt_state(session: &SessionId, id: &str, owner: &SubjectId) -> SessionCredentialState {
    SessionCredentialState {
        session: session.clone(),
        generation: 0,
        current: None,
        pending: None,
        revoke_debt: vec![RevokeDebt {
            id: id.into(),
            owner: owner.clone(),
            queued_at: SystemTime::now(),
        }],
        lease: None,
        retired: true,
    }
}

#[derive(Clone)]
struct RevokePlatform {
    status: StatusCode,
    deleted: Arc<StdMutex<Vec<String>>>,
}

async fn delete_key(
    State(platform): State<RevokePlatform>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    platform.deleted.lock().expect("delete log lock").push(id);
    platform.status
}

async fn revocation_platform(
    status: StatusCode,
) -> (String, RevokePlatform, tokio::task::JoinHandle<()>) {
    let platform = RevokePlatform {
        status,
        deleted: Arc::new(StdMutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/api/v1/api-keys/{id}", delete(delete_key))
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind revoke platform");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve revoke platform");
    });
    (base, platform, server)
}

#[derive(Clone)]
struct ReconciliationPlatform {
    keys: Arc<StdMutex<Vec<(String, SessionId)>>>,
    deleted: Arc<StdMutex<Vec<String>>>,
    listings: Arc<AtomicUsize>,
}

impl ReconciliationPlatform {
    fn add(&self, id: impl Into<String>, session: SessionId) {
        self.keys
            .lock()
            .expect("provider key lock")
            .push((id.into(), session));
    }

    fn remaining_ids(&self) -> Vec<String> {
        let mut ids = self
            .keys
            .lock()
            .expect("provider key lock")
            .iter()
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn deleted_len(&self) -> usize {
        self.deleted.lock().expect("delete log lock").len()
    }

    fn take_listing_count(&self) -> usize {
        self.listings.swap(0, Ordering::SeqCst)
    }
}

async fn list_reconciliation_keys(
    State(platform): State<ReconciliationPlatform>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    platform.listings.fetch_add(1, Ordering::SeqCst);
    let keys = platform
        .keys
        .lock()
        .expect("provider key lock")
        .iter()
        .map(|(id, session)| {
            serde_json::json!({
                "id": id,
                "name": crate::auth::keys::key_name("test", session),
            })
        })
        .collect::<Vec<_>>();
    Json(serde_json::json!({ "keys": keys }))
}

async fn delete_reconciliation_key(
    State(platform): State<ReconciliationPlatform>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    platform
        .deleted
        .lock()
        .expect("delete log lock")
        .push(id.clone());
    platform
        .keys
        .lock()
        .expect("provider key lock")
        .retain(|(candidate, _)| candidate != &id);
    StatusCode::NO_CONTENT
}

async fn reconciliation_platform(
    keys: Vec<(String, SessionId)>,
) -> (String, ReconciliationPlatform, tokio::task::JoinHandle<()>) {
    let platform = ReconciliationPlatform {
        keys: Arc::new(StdMutex::new(keys)),
        deleted: Arc::new(StdMutex::new(Vec::new())),
        listings: Arc::new(AtomicUsize::new(0)),
    };
    let app = Router::new()
        .route("/api/v1/api-keys", get(list_reconciliation_keys))
        .route("/api/v1/api-keys/{id}", delete(delete_reconciliation_key))
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind reconciliation platform");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve reconciliation platform");
    });
    (base, platform, server)
}

async fn configure_issuer(auth: &AuthState, base: String) {
    auth.use_issuer(Arc::new(KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    )))
    .await;
}

#[tokio::test]
async fn retired_debt_survives_restart_and_another_instance_settles_it() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = auth(Arc::clone(&store));
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_restart_debt".into());
    store_owner_token(&first, &owner).await;
    store
        .insert_credential(current_state(&session, &owner))
        .await
        .expect("credential state persists");

    first.release(&session).await.expect("retirement persists");
    drop(first);

    let persisted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("retired state remains");
    assert!(persisted.retired);
    assert_eq!(persisted.revoke_debt.len(), 1);
    assert_eq!(persisted.revoke_debt[0].id, "provider-key");

    let (base, platform, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    let second = auth(Arc::clone(&store));
    configure_issuer(&second, base).await;
    assert_eq!(
        second
            .revoke_retired_credentials()
            .await
            .expect("debt sweep succeeds"),
        1
    );
    assert!(store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state remains")
        .revoke_debt
        .is_empty());
    assert_eq!(
        platform.deleted.lock().expect("delete log lock").as_slice(),
        ["provider-key"]
    );
    server.abort();
}

#[tokio::test]
async fn a_failed_provider_delete_leaves_durable_debt() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_failed_delete".into());
    store_owner_token(&auth, &owner).await;
    store
        .insert_credential(current_state(&session, &owner))
        .await
        .expect("credential state persists");
    auth.release(&session).await.expect("retirement persists");
    let (base, platform, server) = revocation_platform(StatusCode::SERVICE_UNAVAILABLE).await;
    configure_issuer(&auth, base).await;

    assert_eq!(
        auth.revoke_retired_credentials()
            .await
            .expect("failed provider response is retryable debt"),
        0
    );
    assert_eq!(
        store
            .credential(&session)
            .await
            .expect("store works")
            .expect("state remains")
            .revoke_debt
            .len(),
        1
    );
    assert_eq!(
        platform.deleted.lock().expect("delete log lock").as_slice(),
        ["provider-key"]
    );
    server.abort();
}

#[tokio::test]
async fn a_store_outage_during_debt_enumeration_leaves_debt_untouched() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_debt_store_outage".into());
    store_owner_token(&auth, &owner).await;
    store
        .insert_credential(current_state(&session, &owner))
        .await
        .expect("credential state persists");
    auth.release(&session).await.expect("retirement persists");
    let (base, platform, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    configure_issuer(&auth, base).await;

    store.set_unavailable(true);
    assert!(matches!(
        auth.revoke_retired_credentials().await,
        Err(StoreError::Unavailable(_))
    ));
    store.set_unavailable(false);

    assert_eq!(
        store
            .credential(&session)
            .await
            .expect("store works")
            .expect("state remains")
            .revoke_debt
            .len(),
        1
    );
    assert!(platform.deleted.lock().expect("delete log lock").is_empty());
    server.abort();
}

#[derive(Default)]
struct ObservedSessions {
    gets: AtomicUsize,
    unavailable: AtomicBool,
    failed: StdMutex<Option<SessionId>>,
}

impl ObservedSessions {
    fn unavailable() -> Self {
        Self {
            unavailable: AtomicBool::new(true),
            ..Self::default()
        }
    }

    fn fail(&self, session: Option<SessionId>) {
        *self.failed.lock().expect("failed session lock") = session;
    }

    fn take_gets(&self) -> usize {
        self.gets.swap(0, Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SessionStore for ObservedSessions {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst)
            || self.failed.lock().expect("failed session lock").as_ref() == Some(id)
        {
            return Err(StoreError::Unavailable(
                "injected session store outage".into(),
            ));
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

pub(super) fn unavailable_session_service() -> Arc<SessionService> {
    session_service(Arc::new(ObservedSessions::unavailable()))
}

#[tokio::test]
async fn an_unreadable_session_claim_causes_zero_provider_deletes() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_unreadable_claim".into());
    store
        .insert_credential(current_state(&session, &owner))
        .await
        .expect("credential state persists");
    let (base, platform, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    configure_issuer(&auth, base).await;

    assert!(matches!(
        auth.reclaim_unrecorded_credentials(&unavailable_session_service())
            .await,
        Err(StoreError::Unavailable(_))
    ));
    assert!(platform.deleted.lock().expect("delete log lock").is_empty());
    let unchanged = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state remains");
    assert!(unchanged.current.is_some());
    assert!(!unchanged.retired);
    assert!(unchanged.revoke_debt.is_empty());
    server.abort();
}

#[tokio::test]
async fn active_custody_reconciliation_is_bounded_and_a_failed_head_retries_after_wrap() {
    const ACTIVE_PER_PASS: usize = (crate::auth::AUTH_SWEEP_BATCH_SIZE / 2 - 2) / 5;
    const ACTIVE_RECORDS: usize = crate::auth::AUTH_SWEEP_BATCH_SIZE + 2;

    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    let owner = SubjectId("owner".into());
    for number in 0..ACTIVE_RECORDS {
        let session = SessionId(format!("sesn_active_{number:03}"));
        let mut custody = current_state(&session, &owner);
        custody.current.as_mut().unwrap().id = format!("provider-active-{number:03}");
        assert!(store
            .insert_credential(custody)
            .await
            .expect("active custody persists"));
    }

    let (base, platform, server) = reconciliation_platform(Vec::new()).await;
    configure_issuer(&auth, base).await;
    let sessions = Arc::new(ObservedSessions::default());
    let failed = SessionId("sesn_active_000".into());
    sessions.fail(Some(failed.clone()));
    let service = session_service(Arc::clone(&sessions) as Arc<dyn SessionStore>);

    assert!(matches!(
        auth.reclaim_unrecorded_credentials(&service).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(sessions.take_gets(), ACTIVE_PER_PASS);
    assert!(
        store
            .credential(&SessionId("sesn_active_001".into()))
            .await
            .expect("store works")
            .expect("later custody remains")
            .retired
    );
    assert!(
        !store
            .credential(&failed)
            .await
            .expect("store works")
            .expect("failed custody remains")
            .retired
    );
    sessions.fail(None);

    for _ in 1..ACTIVE_RECORDS.div_ceil(ACTIVE_PER_PASS) {
        auth.reclaim_unrecorded_credentials(&service)
            .await
            .expect("later active page reconciles");
        assert!(sessions.take_gets() <= ACTIVE_PER_PASS);
    }
    let through_sequence = store.auth_sweep_high_water().await.unwrap();
    let active = store
        .active_credentials_for_reconciliation(None, through_sequence, ACTIVE_RECORDS + 1)
        .await
        .expect("active custody can be counted");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].session, failed);

    auth.reclaim_unrecorded_credentials(&service)
        .await
        .expect("the finite epoch wraps to the failed head");
    assert_eq!(sessions.take_gets(), 1);
    assert!(
        store
            .credential(&failed)
            .await
            .expect("store works")
            .expect("failed custody remains")
            .retired
    );
    assert!(platform.deleted.lock().expect("delete log lock").is_empty());
    server.abort();
}

#[tokio::test]
async fn provider_reconciliation_caps_each_unpaginated_owner_listing() {
    const PROVIDER_KEYS: usize = crate::auth::AUTH_SWEEP_BATCH_SIZE + 5;
    const MAX_CANDIDATES_AFTER_RESUME: usize = (crate::auth::AUTH_SWEEP_BATCH_SIZE / 2 - 1) / 4;

    let keys = (0..PROVIDER_KEYS)
        .map(|number| {
            (
                format!("provider-{number:03}"),
                SessionId(format!("sesn_provider_{number:03}")),
            )
        })
        .collect();
    let (base, platform, server) = reconciliation_platform(keys).await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    store_owner_token(&auth, &SubjectId("owner".into())).await;
    configure_issuer(&auth, base).await;
    let sessions = Arc::new(ObservedSessions::default());
    let service = session_service(Arc::clone(&sessions) as Arc<dyn SessionStore>);

    let mut passes = 0;
    while !platform.remaining_ids().is_empty() {
        let deleted_before = platform.deleted_len();
        auth.reclaim_unrecorded_credentials(&service)
            .await
            .expect("provider reconciliation pass succeeds");
        let deleted = platform.deleted_len() - deleted_before;
        assert!(deleted <= MAX_CANDIDATES_AFTER_RESUME);
        assert_eq!(sessions.take_gets(), deleted);
        assert_eq!(
            platform.take_listing_count(),
            usize::from(passes == 0),
            "the bounded provider snapshot is retained across resumed passes"
        );
        passes += 1;
        assert!(passes < 10, "bounded pages must make forward progress");
    }

    assert_eq!(platform.deleted_len(), PROVIDER_KEYS);
    assert!(
        passes > 1,
        "one complete listing is never processed at once"
    );
    server.abort();
}

#[tokio::test]
async fn failed_provider_head_and_growing_tail_wait_for_the_finite_epoch_to_wrap() {
    let sessions_by_key = (0..20)
        .map(|number| {
            (
                format!("provider-{number:03}"),
                SessionId(format!("sesn_provider_{number:03}")),
            )
        })
        .collect::<Vec<_>>();
    let failed_session = sessions_by_key[0].1.clone();
    let (base, platform, server) = reconciliation_platform(sessions_by_key).await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    store_owner_token(&auth, &SubjectId("owner".into())).await;
    configure_issuer(&auth, base).await;
    let sessions = Arc::new(ObservedSessions::default());
    let service = session_service(Arc::clone(&sessions) as Arc<dyn SessionStore>);
    store
        .fail_credential_read(Some(failed_session.clone()))
        .await;

    assert!(matches!(
        auth.reclaim_unrecorded_credentials(&service).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.deleted_len(), 10);
    assert_eq!(sessions.take_gets(), 10);
    assert!(platform
        .remaining_ids()
        .contains(&"provider-000".to_string()));

    let tail_session = SessionId("sesn_provider_tail".into());
    platform.add("provider-999", tail_session);
    store.fail_credential_read(None).await;
    assert_eq!(
        auth.reclaim_unrecorded_credentials(&service)
            .await
            .expect("the original epoch finishes"),
        9
    );
    assert_eq!(sessions.take_gets(), 9);
    assert_eq!(
        platform.remaining_ids(),
        ["provider-000".to_string(), "provider-999".to_string()],
        "the failed head and post-high-water tail wait for wrap"
    );

    assert_eq!(
        auth.reclaim_unrecorded_credentials(&service)
            .await
            .expect("the next epoch retries both deferred keys"),
        2
    );
    assert_eq!(sessions.take_gets(), 2);
    assert!(platform.remaining_ids().is_empty());
    server.abort();
}

#[tokio::test]
async fn revoke_debt_failure_does_not_starve_a_full_page_or_a_growing_tail() {
    const ORIGINAL_DEBTS: usize = crate::auth::AUTH_SWEEP_BATCH_SIZE + 2;
    const DEBTS_PER_RESUMED_PASS: usize = crate::auth::AUTH_SWEEP_BATCH_SIZE / 8;

    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(Arc::clone(&store));
    let failed_owner = SubjectId("failed-owner".into());
    let healthy_owner = SubjectId("healthy-owner".into());
    store_owner_token(&auth, &failed_owner).await;
    store_owner_token(&auth, &healthy_owner).await;
    for number in 0..ORIGINAL_DEBTS {
        let session = SessionId(format!("sesn_debt_{number:03}"));
        let owner = if number == 0 {
            &failed_owner
        } else {
            &healthy_owner
        };
        assert!(store
            .insert_credential(debt_state(
                &session,
                &format!("provider-debt-{number:03}"),
                owner,
            ))
            .await
            .expect("revoke debt persists"));
    }
    let (base, platform, server) = revocation_platform(StatusCode::NO_CONTENT).await;
    configure_issuer(&auth, base).await;
    store.fail_subject_read(Some(failed_owner.clone())).await;

    assert!(matches!(
        auth.revoke_retired_credentials().await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.deleted.lock().expect("delete log lock").len(), 11);
    assert_eq!(
        store
            .credential(&SessionId("sesn_debt_001".into()))
            .await
            .expect("store works")
            .expect("later debt state remains")
            .revoke_debt
            .len(),
        0,
        "the deterministic first-row failure does not block later debt"
    );

    let tail_session = SessionId("sesn_debt_999".into());
    assert!(store
        .insert_credential(debt_state(
            &tail_session,
            "provider-debt-999",
            &healthy_owner,
        ))
        .await
        .expect("growing tail persists"));
    store.fail_subject_read(None).await;
    let mut settled_original = 11;
    while settled_original < ORIGINAL_DEBTS - 1 {
        let settled = auth
            .revoke_retired_credentials()
            .await
            .expect("the original high-water epoch advances");
        assert!(settled <= DEBTS_PER_RESUMED_PASS);
        assert!(settled > 0);
        settled_original += settled;
    }
    assert_eq!(
        store
            .credential(&tail_session)
            .await
            .expect("store works")
            .expect("tail state remains")
            .revoke_debt
            .len(),
        1,
        "work added beyond the epoch high water waits for wrap"
    );

    assert_eq!(
        auth.revoke_retired_credentials()
            .await
            .expect("the next epoch retries the failed head and growing tail"),
        2
    );
    assert!(store
        .credential(&SessionId("sesn_debt_000".into()))
        .await
        .expect("store works")
        .expect("failed debt state remains")
        .revoke_debt
        .is_empty());
    assert!(store
        .credential(&tail_session)
        .await
        .expect("store works")
        .expect("tail state remains")
        .revoke_debt
        .is_empty());
    assert_eq!(platform.deleted.lock().expect("delete log lock").len(), 103);
    server.abort();
}
