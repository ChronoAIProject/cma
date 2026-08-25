use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::{Duration, SystemTime};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use layer2_core::store::StoreError;
use layer2_core::{CallerCredential, CredentialSource as _, ServiceId, SessionId, SubjectId};

use super::*;
use crate::auth::persistence::{
    AuthStore, AuthSweepItem, BrowserSession, CredentialLeaseCursor, CredentialMutationCursor,
    CredentialReconciliationCursor, CredentialReissueCursor, LoginAttempt, MemoryAuthStore,
    RevokeDebtCursor, RevokeDebtWork, SecretDigest, SecretValue, SubjectAuthState,
    SubjectOwnerCursor, SubjectRefreshCursor,
};
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

fn credential(id: &str, value: &str, owner: &SubjectId, scope: &str) -> DurableCredential {
    DurableCredential {
        id: id.into(),
        value: SecretValue::new(value.into()),
        owner: owner.clone(),
        issued_at: SystemTime::now(),
        expires_at: None,
        scope_fingerprint: scope.into(),
    }
}

fn state(
    session: &SessionId,
    current: Option<DurableCredential>,
    pending: Option<DurableCredential>,
) -> SessionCredentialState {
    SessionCredentialState {
        session: session.clone(),
        generation: 0,
        request_bound_credential: None,
        current,
        pending,
        revoke_debt: Vec::new(),
        lease: None,
        retired: false,
    }
}

struct TerminalInsertRaceStore {
    inner: Arc<MemoryAuthStore>,
    owner: SubjectId,
    insert_calls: AtomicUsize,
    replace_calls: AtomicUsize,
}

impl TerminalInsertRaceStore {
    fn new(inner: Arc<MemoryAuthStore>, owner: SubjectId) -> Self {
        Self {
            inner,
            owner,
            insert_calls: AtomicUsize::new(0),
            replace_calls: AtomicUsize::new(0),
        }
    }

    fn raced_active_state(&self, session: &SessionId, generation: u64) -> SessionCredentialState {
        let now = SystemTime::now();
        let mut raced = state(
            session,
            Some(credential(
                "raced-current",
                "raced-current-value",
                &self.owner,
                "scope",
            )),
            Some(credential(
                "raced-pending",
                "raced-pending-value",
                &self.owner,
                "scope",
            )),
        );
        raced.generation = generation;
        raced.lease = Some(CredentialLease {
            holder: "raced-holder".into(),
            fence: generation + 1,
            acquired_at: now,
            expires_at: now + Duration::from_secs(30),
            scope_fingerprint: "scope".into(),
        });
        raced
    }
}

#[async_trait::async_trait]
impl AuthStore for TerminalInsertRaceStore {
    async fn auth_sweep_high_water(&self) -> Result<u64, StoreError> {
        self.inner.auth_sweep_high_water().await
    }

    async fn put_browser_session(&self, _session: BrowserSession) -> Result<(), StoreError> {
        unreachable!("terminal insert race does not use browser sessions")
    }

    async fn browser_session(
        &self,
        _id: &SecretDigest,
    ) -> Result<Option<BrowserSession>, StoreError> {
        unreachable!("terminal insert race does not use browser sessions")
    }

    async fn delete_browser_session(&self, _id: &SecretDigest) -> Result<(), StoreError> {
        unreachable!("terminal insert race does not use browser sessions")
    }

    async fn put_login_attempt(&self, _attempt: LoginAttempt) -> Result<(), StoreError> {
        unreachable!("terminal insert race does not use login attempts")
    }

    async fn take_login_attempt(
        &self,
        _id: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        unreachable!("terminal insert race does not use login attempts")
    }

    async fn subject(&self, _subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        unreachable!("terminal insert race does not use subject state")
    }

    async fn insert_subject(&self, _state: SubjectAuthState) -> Result<bool, StoreError> {
        unreachable!("terminal insert race does not use subject state")
    }

    async fn replace_subject(
        &self,
        _expected_revision: u64,
        _state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        unreachable!("terminal insert race does not use subject state")
    }

    async fn unleased_subjects_due_for_refresh(
        &self,
        _refresh_at: SystemTime,
        _after: Option<&SubjectRefreshCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError> {
        unreachable!("terminal insert race does not use subject state")
    }

    async fn subjects_with_expired_refresh_leases(
        &self,
        _now: SystemTime,
        _after: Option<&SubjectRefreshCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError> {
        unreachable!("terminal insert race does not use subject state")
    }

    async fn subjects_with_tokens(
        &self,
        _after: Option<&SubjectOwnerCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<SubjectOwnerCursor>, StoreError> {
        unreachable!("terminal insert race does not use subject state")
    }

    async fn credential(
        &self,
        session: &SessionId,
    ) -> Result<Option<SessionCredentialState>, StoreError> {
        self.inner.credential(session).await
    }

    async fn insert_credential(
        &self,
        terminal: SessionCredentialState,
    ) -> Result<bool, StoreError> {
        assert!(terminal.retired, "the missing observation stays terminal");
        assert_eq!(self.insert_calls.fetch_add(1, Ordering::SeqCst), 0);
        assert!(
            self.inner
                .insert_credential(self.raced_active_state(&terminal.session, 0))
                .await?
        );
        Ok(false)
    }

    async fn replace_credential(
        &self,
        expected_generation: u64,
        terminal: SessionCredentialState,
    ) -> Result<bool, StoreError> {
        let call = self.replace_calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            assert!(terminal.retired, "the failed insert stays terminal");
            let mut raced = self
                .inner
                .credential(&terminal.session)
                .await?
                .expect("the active insert won the first race");
            assert_eq!(raced.generation, expected_generation);
            raced.generation += 1;
            raced
                .lease
                .as_mut()
                .expect("raced state has a lease")
                .holder = "second-raced-holder".into();
            assert!(
                self.inner
                    .replace_credential(expected_generation, raced)
                    .await?
            );
            return Ok(false);
        }
        self.inner
            .replace_credential(expected_generation, terminal)
            .await
    }

    async fn current_credentials_due_for_reissue(
        &self,
        _issued_before: SystemTime,
        _after: Option<&CredentialReissueCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialReissueCursor, SessionCredentialState>>, StoreError>
    {
        unreachable!("terminal insert race does not scan credentials")
    }

    async fn pending_credentials_due_for_reissue(
        &self,
        _after: Option<&CredentialMutationCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialMutationCursor, SessionCredentialState>>, StoreError>
    {
        unreachable!("terminal insert race does not scan credentials")
    }

    async fn credentials_with_expired_mint_leases(
        &self,
        _now: SystemTime,
        _after: Option<&CredentialLeaseCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialLeaseCursor, SessionCredentialState>>, StoreError> {
        unreachable!("terminal insert race does not scan credentials")
    }

    async fn revoke_debts(
        &self,
        _after: Option<&RevokeDebtCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<RevokeDebtCursor, RevokeDebtWork>>, StoreError> {
        unreachable!("terminal insert race does not scan revoke debt")
    }

    async fn active_credentials_for_reconciliation(
        &self,
        _after: Option<&CredentialReconciliationCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<CredentialReconciliationCursor>, StoreError> {
        unreachable!("terminal insert race does not reconcile credentials")
    }
}

#[derive(Clone)]
struct MintPlatform {
    mints: Arc<AtomicUsize>,
}

async fn list_owner_services(headers: HeaderMap) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    Json(serde_json::json!({
        "services": [{
            "id": "resolved-model",
            "slug": "model",
            "is_active": true,
        }]
    }))
}

async fn mint_key(
    State(platform): State<MintPlatform>,
    headers: HeaderMap,
    Json(_body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    let mint = platform.mints.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::time::sleep(Duration::from_millis(50)).await;
    Json(serde_json::json!({
        "id": format!("provider-key-{mint}"),
        "full_key": format!("full-key-{mint}"),
    }))
}

async fn mint_platform() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let mints = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/api/v1/user-services", get(list_owner_services))
        .route("/api/v1/api-keys", post(mint_key))
        .with_state(MintPlatform {
            mints: Arc::clone(&mints),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mint platform");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve mint platform");
    });
    (base, mints, server)
}

#[derive(Clone)]
struct ResponseLossPlatform {
    mints: Arc<AtomicUsize>,
    delete_attempts: Arc<AtomicUsize>,
    keys: Arc<StdMutex<Vec<(String, String)>>>,
}

impl ResponseLossPlatform {
    fn add(&self, id: impl Into<String>, name: impl Into<String>) {
        self.keys
            .lock()
            .expect("provider key lock")
            .push((id.into(), name.into()));
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
}

async fn mint_response_loss_key(
    State(platform): State<ResponseLossPlatform>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    let mint = platform.mints.fetch_add(1, Ordering::SeqCst) + 1;
    let id = format!("provider-key-{mint}");
    let name = body
        .get("name")
        .and_then(serde_json::Value::as_str)
        .expect("mint request names its session")
        .to_string();
    platform.add(id.clone(), name);
    Json(serde_json::json!({
        "id": id,
        "full_key": format!("full-key-{mint}"),
    }))
}

async fn list_response_loss_keys(
    State(platform): State<ResponseLossPlatform>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    let keys = platform
        .keys
        .lock()
        .expect("provider key lock")
        .iter()
        .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
        .collect::<Vec<_>>();
    Json(serde_json::json!({ "keys": keys }))
}

async fn delete_response_loss_key(
    State(platform): State<ResponseLossPlatform>,
    Path(id): Path<String>,
) -> StatusCode {
    let attempt = platform.delete_attempts.fetch_add(1, Ordering::SeqCst) + 1;
    if attempt == 1 {
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    platform
        .keys
        .lock()
        .expect("provider key lock")
        .retain(|(stored, _)| stored != &id);
    StatusCode::NO_CONTENT
}

async fn response_loss_platform() -> (String, ResponseLossPlatform, tokio::task::JoinHandle<()>) {
    let platform = ResponseLossPlatform {
        mints: Arc::new(AtomicUsize::new(0)),
        delete_attempts: Arc::new(AtomicUsize::new(0)),
        keys: Arc::new(StdMutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/api/v1/user-services", get(list_owner_services))
        .route(
            "/api/v1/api-keys",
            get(list_response_loss_keys).post(mint_response_loss_key),
        )
        .route("/api/v1/api-keys/{id}", delete(delete_response_loss_key))
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind response-loss platform");
    let base = format!("http://{}", listener.local_addr().expect("local address"));
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve response-loss platform");
    });
    (base, platform, server)
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

#[test]
fn scope_fingerprints_are_order_independent_but_domain_specific() {
    let first = scope_fingerprint(&[ServiceId::new("b"), ServiceId::new("a")]);
    let second = scope_fingerprint(&[ServiceId::new("a"), ServiceId::new("b")]);
    assert_eq!(first, second);
    assert_ne!(first, scope_fingerprint(&[ServiceId::new("a")]))
}

async fn expire_mint_lease(store: &MemoryAuthStore, session: &SessionId) {
    let mut state = store
        .credential(session)
        .await
        .expect("store works")
        .expect("mint lease remains durable");
    let generation = state.generation;
    state.generation += 1;
    state
        .lease
        .as_mut()
        .expect("failed mint commit leaves its lease")
        .expires_at = SystemTime::now() - Duration::from_secs(1);
    assert!(store
        .replace_credential(generation, state)
        .await
        .expect("expired lease persists"));
}

#[tokio::test]
async fn a_lost_mongo_ack_does_not_revoke_or_remint_recorded_initial_custody() {
    let (base, platform, server) = response_loss_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let minting = auth(Arc::clone(&store));
    minting
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base.clone(),
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_lost_mongo_ack".into());
    let services = [ServiceId::new("model")];
    store_owner_token(&minting, &owner).await;

    store.lose_credential_replace_response_on_call(2);
    assert!(matches!(
        minting
            .credential_for(&session, &owner, &services, None)
            .await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.mints.load(Ordering::SeqCst), 1);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(platform.remaining_ids(), ["provider-key-1"]);
    let durable = store
        .credential(&session)
        .await
        .expect("confirmation read works")
        .expect("the acknowledged-lost write was applied");
    assert_eq!(
        durable.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert!(durable.lease.is_none());

    store.clear_credential_replace_failure();
    drop(minting);
    let restarted = auth(Arc::clone(&store));
    restarted
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
    assert_eq!(
        restarted
            .credential_for(&session, &owner, &services, None)
            .await
            .expect("restart reads recorded custody"),
        Some("full-key-1".into())
    );
    assert_eq!(platform.mints.load(Ordering::SeqCst), 1);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn a_lost_mongo_ack_does_not_revoke_or_remint_recorded_pending_custody() {
    let (base, platform, server) = response_loss_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let minting = auth(Arc::clone(&store));
    minting
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base.clone(),
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_lost_pending_mongo_ack".into());
    let services = [ServiceId::new("model")];
    store_owner_token(&minting, &owner).await;

    assert_eq!(
        minting
            .credential_for(&session, &owner, &services, None)
            .await
            .expect("initial custody persists"),
        Some("full-key-1".into())
    );
    store.lose_credential_replace_response_on_call(2);
    assert!(matches!(
        minting.reissue(&session, &owner, &services).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.mints.load(Ordering::SeqCst), 2);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(
        platform.remaining_ids(),
        ["provider-key-1", "provider-key-2"]
    );
    let durable = store
        .credential(&session)
        .await
        .expect("confirmation read works")
        .expect("the acknowledged-lost pending write was applied");
    assert_eq!(
        durable.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert_eq!(
        durable.pending.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-2")
    );
    assert!(durable.lease.is_none());

    store.clear_credential_replace_failure();
    drop(minting);
    let restarted = auth(Arc::clone(&store));
    restarted
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
    assert_eq!(
        restarted
            .reissue(&session, &owner, &services)
            .await
            .expect("restart reads recorded pending custody"),
        Some("full-key-2".into())
    );
    assert_eq!(platform.mints.load(Ordering::SeqCst), 2);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn lost_initial_mint_response_is_reclaimed_after_restart_and_lease_expiry() {
    let (base, platform, server) = response_loss_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let minting = auth(Arc::clone(&store));
    minting
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base.clone(),
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_lost_initial_commit".into());
    let services = [ServiceId::new("model")];
    store_owner_token(&minting, &owner).await;

    store.fail_credential_replace_on_call(2);
    assert!(matches!(
        minting
            .credential_for(&session, &owner, &services, None)
            .await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.remaining_ids(), ["provider-key-1"]);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 1);

    store.clear_credential_replace_failure();
    expire_mint_lease(&store, &session).await;
    drop(minting);
    let restarted = auth(Arc::clone(&store));
    restarted
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;

    assert!(matches!(
        restarted
            .reclaim_unrecorded_credentials(&super::reclaim_tests::unavailable_session_service(),)
            .await,
        Err(StoreError::Unavailable(_))
    ));
    assert!(platform.remaining_ids().is_empty());
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn lost_replacement_response_reclaims_only_the_unmatched_key_after_restart() {
    let (base, platform, server) = response_loss_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let minting = auth(Arc::clone(&store));
    minting
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base.clone(),
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_lost_replacement_commit".into());
    let services = [ServiceId::new("model")];
    let fingerprint = scope_fingerprint(&services);
    store_owner_token(&minting, &owner).await;
    assert!(store
        .insert_credential(state(
            &session,
            Some(credential(
                "provider-old",
                "old-value",
                &owner,
                &fingerprint,
            )),
            None,
        ))
        .await
        .expect("current custody persists"));
    platform.add(
        "provider-old",
        crate::auth::keys::key_name("test", &session),
    );

    store.fail_credential_replace_on_call(2);
    assert!(matches!(
        minting.reissue(&session, &owner, &services).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.remaining_ids(), ["provider-key-1", "provider-old"]);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 1);

    store.clear_credential_replace_failure();
    expire_mint_lease(&store, &session).await;
    drop(minting);
    let restarted = auth(Arc::clone(&store));
    restarted
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;

    assert!(matches!(
        restarted
            .reclaim_unrecorded_credentials(&super::reclaim_tests::unavailable_session_service(),)
            .await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(platform.remaining_ids(), ["provider-old"]);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 2);
    let durable = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("current custody remains");
    assert_eq!(
        durable.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-old")
    );
    assert!(durable.pending.is_none());
    server.abort();
}

#[tokio::test]
async fn a_mint_finishing_after_tombstone_cleanup_recreates_a_terminal_debt_record() {
    let store = Arc::new(MemoryAuthStore::default());
    let durable = store.clone() as Arc<dyn AuthStore>;
    let session = SessionId("sesn_late_after_cleanup".into());
    let owner = SubjectId("owner".into());
    let minted = credential("provider-late", "late-value", &owner, "scope");

    assert_eq!(
        commit_mint(&durable, &session, "expired-holder", 1, minted, false, None)
            .await
            .expect("late completion is recorded"),
        None
    );
    let recreated = store
        .credential(&session)
        .await
        .unwrap()
        .expect("terminal debt record is recreated");
    assert!(recreated.retired);
    assert!(recreated.current.is_none());
    assert!(recreated.pending.is_none());
    assert!(recreated.lease.is_none());
    assert_eq!(recreated.revoke_debt.len(), 1);
    assert_eq!(recreated.revoke_debt[0].id, "provider-late");

    let mut settled = recreated.clone();
    settled.generation += 1;
    settled.revoke_debt.clear();
    assert!(store
        .replace_credential(recreated.generation, settled)
        .await
        .unwrap());
    assert!(store
        .credential(&session)
        .await
        .unwrap()
        .unwrap()
        .is_settled_retired());
}

#[tokio::test]
async fn a_missing_terminal_fence_stays_sticky_across_insert_and_cas_races() {
    let inner = Arc::new(MemoryAuthStore::default());
    let owner = SubjectId("owner".into());
    let store = Arc::new(TerminalInsertRaceStore::new(
        Arc::clone(&inner),
        owner.clone(),
    ));
    let durable = store.clone() as Arc<dyn AuthStore>;
    let session = SessionId("sesn_terminal_insert_race".into());
    let minted = credential("provider-late", "late-value", &owner, "scope");

    assert_eq!(
        commit_mint(&durable, &session, "expired-holder", 1, minted, false, None)
            .await
            .expect("the late completion is durably retired"),
        None
    );

    assert_eq!(store.insert_calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.replace_calls.load(Ordering::SeqCst), 2);
    let terminal = inner
        .credential(&session)
        .await
        .expect("store works")
        .expect("terminal state survives both races");
    assert!(terminal.retired);
    assert!(terminal.current.is_none());
    assert!(terminal.pending.is_none());
    assert!(terminal.lease.is_none());
    let mut debt_ids: Vec<_> = terminal
        .revoke_debt
        .iter()
        .map(|debt| debt.id.as_str())
        .collect();
    debt_ids.sort_unstable();
    assert_eq!(
        debt_ids,
        ["provider-late", "raced-current", "raced-pending"]
    );
}

#[tokio::test]
async fn losing_only_a_mint_lease_does_not_retire_an_active_session() {
    let store = Arc::new(MemoryAuthStore::default());
    let durable = store.clone() as Arc<dyn AuthStore>;
    let session = SessionId("sesn_lost_lease".into());
    let owner = SubjectId("owner".into());
    let now = SystemTime::now();
    let mut active = state(
        &session,
        Some(credential(
            "provider-current",
            "current-value",
            &owner,
            "scope",
        )),
        None,
    );
    active.lease = Some(CredentialLease {
        holder: "winning-holder".into(),
        fence: 1,
        acquired_at: now,
        expires_at: now + Duration::from_secs(30),
        scope_fingerprint: "scope".into(),
    });
    store
        .insert_credential(active)
        .await
        .expect("active state is inserted");

    assert_eq!(
        commit_mint(
            &durable,
            &session,
            "losing-holder",
            1,
            credential("provider-late", "late-value", &owner, "scope"),
            false,
            None,
        )
        .await
        .expect("the losing mint is queued"),
        None
    );

    let active = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("active state remains");
    assert!(!active.retired);
    assert_eq!(
        active.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-current")
    );
    assert_eq!(
        active.lease.as_ref().map(|lease| lease.holder.as_str()),
        Some("winning-holder")
    );
    assert_eq!(active.revoke_debt.len(), 1);
    assert_eq!(active.revoke_debt[0].id, "provider-late");
}

#[tokio::test]
async fn another_instance_reads_the_same_current_credential() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = auth(store.clone());
    let second = auth(store.clone());
    second
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            "http://provider.invalid",
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_shared".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            Some(credential("provider-id", "session-secret", &owner, &scope)),
            None,
        ))
        .await
        .expect("store works");

    drop(first);
    assert_eq!(
        second
            .credential_for(&session, &owner, &services, None)
            .await
            .expect("credential loads")
            .as_deref(),
        Some("session-secret")
    );
}

#[tokio::test]
async fn a_request_bearer_mints_session_custody_without_becoming_subject_state() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    )))
    .await;
    let owner = SubjectId("bearer-owner".into());
    let session = SessionId("sesn_request_bearer".into());
    let services = [ServiceId::new("model")];

    assert_eq!(
        auth.credential_for(
            &session,
            &owner,
            &services,
            Some(&CallerCredential::request_bound("owner-access".into())),
        )
        .await
        .expect("request token mints custody")
        .as_deref(),
        Some("full-key-1")
    );
    assert_eq!(mints.load(Ordering::SeqCst), 1);
    assert!(
        store.subject(&owner).await.expect("store works").is_none(),
        "an externally minted bearer must not overwrite renewable browser state"
    );
    let custody = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("session custody persists");
    assert!(custody.current.is_some());
    assert_eq!(
        custody
            .request_bound_credential
            .expect("request provenance persists")
            .expose(),
        "owner-access"
    );
    assert_eq!(
        auth.reissue(&session, &owner, &services)
            .await
            .expect("reissue uses durable request provenance")
            .as_deref(),
        Some("full-key-2")
    );
    assert_eq!(mints.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn no_issuer_recovery_keeps_bearer_pinned_while_browser_sessions_follow_refresh() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = auth(store.clone());
    let owner = SubjectId("shared-subject".into());
    let bearer_session = SessionId("sesn_pinned_bearer".into());
    let browser_session = SessionId("sesn_renewable_browser".into());
    let services = [ServiceId::new("model")];
    first
        .store_tokens(
            &owner,
            SubjectTokens {
                access_token: "browser-old".into(),
                refresh_token: Some("browser-refresh".into()),
                expires_at: std::time::Instant::now() + Duration::from_secs(60),
            },
        )
        .await
        .expect("browser state persists");

    let bearer = CallerCredential::request_bound("bearer-exact".into());
    assert_eq!(
        first
            .credential_for(&bearer_session, &owner, &services, Some(&bearer))
            .await
            .expect("bearer custody persists")
            .as_deref(),
        Some("bearer-exact")
    );
    let browser = CallerCredential::from("browser-old");
    assert_eq!(
        first
            .credential_for(&browser_session, &owner, &services, Some(&browser))
            .await
            .expect("browser credential resolves")
            .as_deref(),
        Some("browser-old")
    );

    first
        .store_tokens(
            &owner,
            SubjectTokens {
                access_token: "browser-new".into(),
                refresh_token: Some("browser-refresh-new".into()),
                expires_at: std::time::Instant::now() + Duration::from_secs(60),
            },
        )
        .await
        .expect("browser refresh persists");
    drop(first);
    let restarted = auth(store);

    assert_eq!(
        restarted
            .credential_for(&bearer_session, &owner, &services, None)
            .await
            .expect("restarted bearer custody loads")
            .as_deref(),
        Some("bearer-exact"),
        "same-subject browser state must not replace request-bound authority"
    );
    assert_eq!(
        restarted
            .credential_for(&browser_session, &owner, &services, None)
            .await
            .expect("restarted browser credential loads")
            .as_deref(),
        Some("browser-new"),
        "browser sessions must continue following the refreshed subject token"
    );
    assert!(!restarted
        .tracks_subject_credential(&bearer_session)
        .await
        .expect("bearer provenance loads"));
    assert!(restarted
        .tracks_subject_credential(&browser_session)
        .await
        .expect("browser provenance loads"));
}

#[tokio::test]
async fn two_instances_racing_for_one_session_mint_exactly_one_key() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let first = auth(store.clone());
    let second = auth(store);
    let issuer = Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    ));
    first.use_issuer(Arc::clone(&issuer)).await;
    second.use_issuer(issuer).await;
    let owner = SubjectId("owner".into());
    store_owner_token(&first, &owner).await;
    let session = SessionId("sesn_race".into());
    let services = vec![ServiceId::new("model")];

    let one = tokio::spawn({
        let auth = Arc::clone(&first);
        let session = session.clone();
        let owner = owner.clone();
        let services = services.clone();
        async move {
            auth.credential_for(&session, &owner, &services, None)
                .await
                .expect("first replica completes")
        }
    });
    let two = tokio::spawn({
        let auth = Arc::clone(&second);
        let session = session.clone();
        let owner = owner.clone();
        let services = services.clone();
        async move {
            auth.credential_for(&session, &owner, &services, None)
                .await
                .expect("second replica completes")
        }
    });

    let first_value = one.await.expect("first task joins");
    let second_value = two.await.expect("second task joins");
    assert_eq!(first_value.as_deref(), Some("full-key-1"));
    assert_eq!(second_value, first_value);
    assert_eq!(mints.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn a_pending_replacement_survives_replica_restart_until_confirmation() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let first = auth(store.clone());
    let second = auth(store.clone());
    let issuer = Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    ));
    first.use_issuer(Arc::clone(&issuer)).await;
    second.use_issuer(issuer).await;
    let owner = SubjectId("owner".into());
    store_owner_token(&first, &owner).await;
    let session = SessionId("sesn_reissue".into());
    let services = [ServiceId::new("model")];

    assert_eq!(
        first
            .credential_for(&session, &owner, &services, None)
            .await
            .expect("initial mint succeeds")
            .as_deref(),
        Some("full-key-1")
    );
    assert_eq!(
        second
            .reissue(&session, &owner, &services)
            .await
            .expect("replacement persists")
            .as_deref(),
        Some("full-key-2")
    );
    drop(second);

    assert_eq!(
        first
            .reissue(&session, &owner, &services)
            .await
            .expect("pending value reloads")
            .as_deref(),
        Some("full-key-2")
    );
    assert_eq!(
        mints.load(Ordering::SeqCst),
        2,
        "pending is not minted twice"
    );
    assert!(first
        .confirm_reissue(&session)
        .await
        .expect("promotion persists"));
    let state = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert_eq!(state.current.unwrap().id, "provider-key-2");
    assert!(state.pending.is_none());
    assert_eq!(state.revoke_debt[0].id, "provider-key-1");
    server.abort();
}

#[tokio::test]
async fn an_expired_replacement_lease_is_recovered_after_restart() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let restarted = auth(store.clone());
    restarted
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    store_owner_token(&restarted, &owner).await;
    let session = SessionId("sesn_expired_replacement_lease".into());
    let services = vec![ServiceId::new("model")];
    let fingerprint = scope_fingerprint(&services);
    let now = SystemTime::now();
    let mut abandoned = state(
        &session,
        Some(credential(
            "provider-current",
            "current-value",
            &owner,
            &fingerprint,
        )),
        None,
    );
    abandoned.lease = Some(CredentialLease {
        holder: "crashed-worker".into(),
        fence: 1,
        acquired_at: now - Duration::from_secs(90),
        expires_at: now - Duration::from_secs(45),
        scope_fingerprint: fingerprint,
    });
    assert!(store
        .insert_credential(abandoned)
        .await
        .expect("abandoned lease persists"));

    let due = restarted
        .sessions_due_for_reissue(Duration::ZERO)
        .await
        .expect("expired replacement lease is scanned");
    assert_eq!(due, vec![(session.clone(), owner.clone())]);

    assert_eq!(
        restarted
            .reissue(&session, &owner, &services)
            .await
            .expect("replacement succeeds")
            .as_deref(),
        Some("full-key-1")
    );
    assert_eq!(mints.load(Ordering::SeqCst), 1);
    let recovered = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("credential remains");
    assert_eq!(
        recovered.pending.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert_eq!(
        recovered.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-current")
    );
    assert_eq!(
        recovered.generation, 2,
        "one CAS steals the stale lease and one commits the replacement"
    );
    assert!(recovered.lease.is_none());
    server.abort();
}

#[tokio::test]
async fn an_expired_initial_mint_lease_is_recovered_after_restart() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let restarted = auth(store.clone());
    restarted
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    store_owner_token(&restarted, &owner).await;
    let session = SessionId("sesn_expired_initial_mint_lease".into());
    let services = vec![ServiceId::new("model")];
    let fingerprint = scope_fingerprint(&services);
    let now = SystemTime::now();
    let mut abandoned = state(&session, None, None);
    abandoned.lease = Some(CredentialLease {
        holder: "crashed-worker".into(),
        fence: 1,
        acquired_at: now - Duration::from_secs(90),
        expires_at: now - Duration::from_secs(45),
        scope_fingerprint: fingerprint,
    });
    assert!(store
        .insert_credential(abandoned)
        .await
        .expect("abandoned initial lease persists"));

    assert_eq!(
        restarted
            .credential_for(&session, &owner, &services, None)
            .await
            .expect("initial mint resumes")
            .as_deref(),
        Some("full-key-1")
    );
    assert_eq!(mints.load(Ordering::SeqCst), 1);
    let recovered = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("credential remains");
    assert_eq!(
        recovered.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert!(recovered.pending.is_none());
    assert_eq!(
        recovered.generation, 2,
        "one CAS steals the stale lease and one commits the initial mint"
    );
    assert!(recovered.lease.is_none());
    server.abort();
}

#[tokio::test]
async fn release_fences_a_provider_mint_that_finishes_late() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let minting = auth(store.clone());
    let releasing = auth(store.clone());
    minting
        .use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
            reqwest::Client::new(),
            base,
            "test",
        )))
        .await;
    let owner = SubjectId("owner".into());
    store_owner_token(&minting, &owner).await;
    let session = SessionId("sesn_release_race".into());
    let services = vec![ServiceId::new("model")];

    let task = tokio::spawn({
        let auth = Arc::clone(&minting);
        let session = session.clone();
        let owner = owner.clone();
        async move { auth.credential_for(&session, &owner, &services, None).await }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while mints.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider mint starts");
    releasing
        .release(&session)
        .await
        .expect("retirement wins durably");

    assert_eq!(
        task.await.expect("mint task joins").expect("store works"),
        None
    );
    let state = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert!(state.retired);
    assert!(state.current.is_none());
    assert!(state.pending.is_none());
    assert_eq!(state.revoke_debt.len(), 1);
    assert_eq!(state.revoke_debt[0].id, "provider-key-1");
    server.abort();
}

#[tokio::test]
async fn release_atomically_retires_values_into_revoke_debt() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_ended".into());
    store
        .insert_credential(state(
            &session,
            Some(credential("current-id", "current-value", &owner, "scope")),
            Some(credential("pending-id", "pending-value", &owner, "scope")),
        ))
        .await
        .expect("store works");

    auth.release(&session).await.expect("retirement persists");
    let retired = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state remains");
    assert!(retired.retired);
    assert!(retired.current.is_none());
    assert!(retired.pending.is_none());
    assert!(retired.lease.is_none());
    let mut ids: Vec<_> = retired
        .revoke_debt
        .iter()
        .map(|debt| debt.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["current-id", "pending-id"]);
}

#[tokio::test]
async fn confirming_a_reissue_promotes_pending_and_queues_the_old_value() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_rotate".into());
    store
        .insert_credential(state(
            &session,
            Some(credential("old-id", "old", &owner, "scope")),
            Some(credential("new-id", "new", &owner, "scope")),
        ))
        .await
        .expect("store works");

    assert!(auth
        .confirm_reissue(&session)
        .await
        .expect("promotion persists"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert_eq!(promoted.current.unwrap().id, "new-id");
    assert!(promoted.pending.is_none());
    assert_eq!(promoted.revoke_debt[0].id, "old-id");
}

#[tokio::test]
async fn a_store_outage_is_an_error_not_an_absent_delegation() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    auth.store_tokens(
        &owner,
        SubjectTokens {
            access_token: "owner-access".into(),
            refresh_token: None,
            expires_at: std::time::Instant::now() + Duration::from_secs(60),
        },
    )
    .await
    .expect("tokens persist");
    store.set_unavailable(true);

    assert!(matches!(
        auth.credential_for(
            &SessionId("sesn_outage".into()),
            &owner,
            &[ServiceId::new("model")],
            None,
        )
        .await,
        Err(StoreError::Unavailable(_))
    ));
}

#[tokio::test]
async fn a_retired_session_never_returns_a_late_credential() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let session = SessionId("sesn_retired".into());
    let mut retired = state(&session, None, None);
    retired.retired = true;
    store.insert_credential(retired).await.expect("store works");
    assert_eq!(
        auth.mint_session_credential(
            &session,
            &SubjectId("owner".into()),
            &[ServiceId::new("model")],
            false,
        )
        .await
        .expect("state loads"),
        None
    );
}
