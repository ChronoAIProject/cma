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
use layer2_core::{
    CallerCredential, CredentialDeliveryTarget, CredentialSource as _, ServiceId, SessionId,
    SubjectId,
};

use super::*;
use crate::auth::persistence::{
    AuthStore, AuthSweepItem, BrowserSession, CredentialExpiryCursor, CredentialLeaseCursor,
    CredentialMutationCursor, CredentialReconciliationCursor, CredentialReissueCursor,
    LoginAttempt, MemoryAuthStore, RevokeDebtCursor, RevokeDebtWork, SecretDigest, SecretValue,
    SubjectAuthState, SubjectAuthorizationCursor, SubjectOwnerCursor, SubjectRefreshCursor,
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

async fn configure_test_issuer(auth: &AuthState) {
    auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        "http://platform.invalid",
        "test",
    )))
    .await;
}

fn credential(id: &str, value: &str, owner: &SubjectId, scope: &str) -> DurableCredential {
    DurableCredential {
        id: id.into(),
        value: SecretValue::new(value.into()),
        owner: owner.clone(),
        issued_at: SystemTime::now(),
        expires_at: None,
        scope_fingerprint: scope.into(),
        custody_generation: 0,
        provider_state_version: 0,
        grant_fingerprint: String::new(),
        revocation_credential: None,
    }
}

fn credential_expiring_at(
    id: &str,
    value: &str,
    owner: &SubjectId,
    scope: &str,
    expires_at: SystemTime,
) -> DurableCredential {
    let mut credential = credential(id, value, owner, scope);
    credential.expires_at = Some(expires_at);
    credential
}

fn delivery_target(generation: u64, runtime_id: &str) -> CredentialDeliveryTarget {
    CredentialDeliveryTarget {
        runtime_generation: generation,
        runtime_id: runtime_id.into(),
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
        pending_target: None,
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
        _browser_binding: &SecretDigest,
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
        _expected_incarnation: &str,
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

    async fn subjects_due_for_authorization_check(
        &self,
        _now: SystemTime,
        _after: Option<&SubjectAuthorizationCursor>,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectAuthorizationCursor, SubjectAuthState>>, StoreError> {
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

    async fn current_credentials_expiring_before(
        &self,
        _expires_before: SystemTime,
        _after: Option<&CredentialExpiryCursor>,
        _through_sequence: u64,
        _limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialExpiryCursor, SessionCredentialState>>, StoreError>
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
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    assert_eq!(
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer owner-access")
    );
    let expires_at = body
        .get("expires_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| mongodb::bson::DateTime::parse_rfc3339_str(value).ok())
        .expect("mint requests a valid absolute provider expiry")
        .to_system_time();
    assert!(
        expires_at > SystemTime::now() + Duration::from_secs(29 * 24 * 60 * 60),
        "the provider expiry leaves the documented replacement margin"
    );
    let mint = platform.mints.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::time::sleep(Duration::from_millis(50)).await;
    Json(serde_json::json!({
        "id": format!("provider-key-{mint}"),
        "full_key": format!("full-key-{mint}"),
        "scopes": "proxy read",
        "state_version": mint,
        "allowed_service_ids": ["resolved-model"],
        "allowed_node_ids": [],
        "allow_all_services": false,
        "allow_all_nodes": false,
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
        "scopes": "proxy read",
        "state_version": mint,
        "allowed_service_ids": ["resolved-model"],
        "allowed_node_ids": [],
        "allow_all_services": false,
        "allow_all_nodes": false,
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

#[test]
fn credential_expiry_cutoff_is_inclusive_and_legacy_values_remain_unbounded() {
    let owner = SubjectId("owner".into());
    let cutoff = SystemTime::UNIX_EPOCH + Duration::from_secs(1_900_000_000);
    let mut held = credential("provider-key", "value", &owner, "scope");

    assert!(credential_is_deliverable(&held, cutoff));
    held.expires_at = Some(cutoff);
    assert!(
        !credential_is_deliverable(&held, cutoff),
        "the exact replacement boundary is already nondeliverable"
    );
    held.expires_at = Some(cutoff + Duration::from_nanos(1));
    assert!(credential_is_deliverable(&held, cutoff));
    held.expires_at = Some(cutoff - Duration::from_nanos(1));
    assert!(!credential_is_deliverable(&held, cutoff));
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
    let held = durable.current.as_ref().unwrap();
    assert_ne!(held.custody_generation, 0);
    assert_eq!(held.provider_state_version, 1);
    assert_eq!(held.grant_fingerprint.len(), 64);
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
    let authority = restarted
        .credential_authority(&session)
        .await
        .expect("authority custody read works")
        .expect("the exact current provider authority survives restart");
    assert_eq!(authority.custody_generation, held.custody_generation);
    assert_eq!(authority.provider_key_id, "provider-key-1");
    assert_eq!(authority.provider_state_version, 1);
    assert_eq!(authority.grant_fingerprint, held.grant_fingerprint);
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
    assert!(
        durable.pending.as_ref().unwrap().custody_generation
            > durable.current.as_ref().unwrap().custody_generation
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
    assert_eq!(platform.remaining_ids(), ["provider-key-1"]);
    assert_eq!(
        platform.delete_attempts.load(Ordering::SeqCst),
        1,
        "an unreadable session cannot prove the unmatched key is safe to reclaim"
    );
    assert_eq!(
        restarted
            .reclaim_unrecorded_credentials(&super::reclaim_tests::empty_session_service())
            .await
            .expect("a later readable reconciliation reclaims the unmatched key"),
        1
    );
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
    assert_eq!(platform.remaining_ids(), ["provider-key-1", "provider-old"]);
    assert_eq!(
        platform.delete_attempts.load(Ordering::SeqCst),
        1,
        "an unreadable session cannot confirm which sibling reached its runtime"
    );
    assert_eq!(
        restarted
            .reclaim_unrecorded_credentials(&super::reclaim_tests::empty_session_service())
            .await
            .expect("a later readable reconciliation reclaims the unmatched key"),
        1
    );
    assert_eq!(platform.remaining_ids(), ["provider-old"]);
    assert_eq!(platform.delete_attempts.load(Ordering::SeqCst), 2);
    let durable = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("current custody remains");
    assert!(durable.retired);
    assert!(durable.current.is_none());
    assert!(durable.pending.is_none());
    assert_eq!(durable.revoke_debt.len(), 1);
    assert_eq!(durable.revoke_debt[0].id, "provider-old");
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
async fn issuer_removal_keeps_the_scoped_current_credential() {
    let store = Arc::new(MemoryAuthStore::default());
    let first = auth(store.clone());
    let second = auth(store.clone());
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
            .credential_for(
                &session,
                &owner,
                &services,
                Some(&CallerCredential::from("subject-wide")),
            )
            .await
            .expect("credential loads")
            .as_deref(),
        Some("session-secret"),
        "removing the issuer must not widen an existing scoped session"
    );
    assert!(!second
        .tracks_subject_credential(&session)
        .await
        .expect("scoped custody provenance loads"));
}

#[tokio::test]
async fn issuer_removal_rejects_scoped_custody_for_another_owner() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let recorded_owner = SubjectId("recorded-owner".into());
    let requested_owner = SubjectId("requested-owner".into());
    let session = SessionId("sesn_wrong_scoped_owner".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            Some(credential(
                "provider-id",
                "wrong-owner-secret",
                &recorded_owner,
                &scope,
            )),
            None,
        ))
        .await
        .expect("malformed custody fixture persists");

    assert!(matches!(
        auth.credential_for(&session, &requested_owner, &services, None)
            .await,
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        auth.reissue(&session, &requested_owner, &services).await,
        Err(StoreError::Malformed(_))
    ));

    let wrong_scope_session = SessionId("sesn_wrong_scoped_grant".into());
    store
        .insert_credential(state(
            &wrong_scope_session,
            Some(credential(
                "wrong-scope-provider-id",
                "wrong-scope-secret",
                &requested_owner,
                "not-the-frozen-grant",
            )),
            None,
        ))
        .await
        .expect("wrong-scope custody fixture persists");
    assert!(matches!(
        auth.credential_for(
            &wrong_scope_session,
            &requested_owner,
            &services,
            Some(&CallerCredential::from("wider-subject-token")),
        )
        .await,
        Err(StoreError::Malformed(_))
    ));
    assert!(matches!(
        auth.reissue(&wrong_scope_session, &requested_owner, &services)
            .await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn issuer_removal_does_not_widen_an_incomplete_scoped_transition() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_incomplete_scoped_transition".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let mut custody = state(
        &session,
        None,
        Some(credential(
            "provider-pending",
            "pending-session-secret",
            &owner,
            &scope,
        )),
    );
    custody.request_bound_credential = Some(SecretValue::new("subject-wide".into()));
    store
        .insert_credential(custody)
        .await
        .expect("incomplete scoped custody persists");

    assert_eq!(
        auth.credential_for(
            &session,
            &owner,
            &services,
            Some(&CallerCredential::from("new-subject-wide")),
        )
        .await
        .expect("incomplete transition fails closed"),
        None
    );
    assert!(!auth
        .tracks_subject_credential(&session)
        .await
        .expect("pending scoped provenance loads"));
}

#[tokio::test]
async fn issuer_removal_keeps_a_recorded_transition_mode_correct() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_transition_without_issuer".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            Some(credential("current-id", "current-secret", &owner, &scope)),
            Some(credential("pending-id", "pending-secret", &owner, &scope)),
        ))
        .await
        .expect("recorded transition persists");

    assert_eq!(
        auth.credential_for(&session, &owner, &services, None)
            .await
            .expect("ordinary lookup uses current custody")
            .as_deref(),
        Some("current-secret")
    );
    assert_eq!(
        auth.reissue(&session, &owner, &services)
            .await
            .expect("replacement lookup resumes recorded custody")
            .as_deref(),
        Some("pending-secret")
    );
}

#[tokio::test]
async fn configured_issuer_rejects_pending_custody_without_a_current_credential() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    configure_test_issuer(&auth).await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_pending_without_current".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            None,
            Some(credential("pending-id", "pending-secret", &owner, &scope)),
        ))
        .await
        .expect("incomplete custody fixture persists");

    assert_eq!(
        auth.credential_for(&session, &owner, &services, None)
            .await
            .expect("ordinary credential lookup fails closed"),
        None
    );
    assert_eq!(
        auth.reissue(&session, &owner, &services)
            .await
            .expect("replacement lookup fails closed"),
        None
    );
}

#[tokio::test]
async fn configured_issuer_rejects_a_pending_credential_owned_by_another_subject() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    configure_test_issuer(&auth).await;
    let owner = SubjectId("owner".into());
    let other = SubjectId("other".into());
    let session = SessionId("sesn_wrong_pending_owner".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            Some(credential("current-id", "current-secret", &owner, &scope)),
            Some(credential("pending-id", "pending-secret", &other, &scope)),
        ))
        .await
        .expect("wrong-owner pending fixture persists");

    assert!(matches!(
        auth.credential_for(&session, &owner, &services, None).await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn configured_issuer_rejects_a_current_credential_owned_by_another_subject() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    configure_test_issuer(&auth).await;
    let owner = SubjectId("owner".into());
    let other = SubjectId("other".into());
    let session = SessionId("sesn_wrong_current_owner".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            Some(credential("current-id", "current-secret", &other, &scope)),
            Some(credential("pending-id", "pending-secret", &owner, &scope)),
        ))
        .await
        .expect("wrong-owner current fixture persists");

    assert!(matches!(
        auth.credential_for(&session, &owner, &services, None).await,
        Err(StoreError::Malformed(_))
    ));
}

#[tokio::test]
async fn configured_issuer_selects_current_or_pending_by_operation_mode() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    configure_test_issuer(&auth).await;
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_valid_transition".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    store
        .insert_credential(state(
            &session,
            Some(credential("current-id", "current-secret", &owner, &scope)),
            Some(credential("pending-id", "pending-secret", &owner, &scope)),
        ))
        .await
        .expect("valid transition fixture persists");

    assert_eq!(
        auth.credential_for(&session, &owner, &services, None)
            .await
            .expect("ordinary lookup validates both credentials")
            .as_deref(),
        Some("current-secret")
    );
    assert_eq!(
        auth.reissue(&session, &owner, &services)
            .await
            .expect("replacement lookup validates both credentials")
            .as_deref(),
        Some("pending-secret")
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
    assert!(
        custody
            .current
            .as_ref()
            .and_then(|held| held.expires_at)
            .is_some_and(|expires_at| expires_at > SystemTime::now()),
        "CMA retains the exact provider-enforced expiry with custody"
    );
    assert_eq!(
        custody
            .current
            .as_ref()
            .and_then(|held| held.revocation_credential.as_ref())
            .expect("minted key owns its bearer-only revoke authority")
            .expose(),
        "owner-access"
    );
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
    let pending_custody_generation = store
        .credential(&session)
        .await
        .expect("store works")
        .and_then(|state| state.pending.map(|held| held.custody_generation))
        .expect("replacement has exact pending custody metadata");
    assert!(first
        .confirm_reissue(&session)
        .await
        .expect("promotion persists"));
    let state = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    let current = state.current.unwrap();
    assert_eq!(current.id, "provider-key-2");
    assert_eq!(
        current.custody_generation, pending_custody_generation,
        "promotion must not replace stable credential identity with the later document CAS"
    );
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
    assert_eq!(due, vec![session.clone()]);

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
async fn release_settlement_requires_retirement_with_no_durable_revoke_debt() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let settled_session = SessionId("sesn_settled_retirement".into());
    store
        .insert_credential(state(&settled_session, None, None))
        .await
        .expect("store works");

    assert!(!auth
        .release_settled(&settled_session)
        .await
        .expect("custody can be inspected"));
    auth.release(&settled_session)
        .await
        .expect("empty custody retires durably");
    assert!(auth
        .release_settled(&settled_session)
        .await
        .expect("settled retirement is positive evidence"));

    let debt_session = SessionId("sesn_unsettled_retirement".into());
    store
        .insert_credential(state(
            &debt_session,
            Some(credential("provider-key", "value", &owner, "scope")),
            None,
        ))
        .await
        .expect("store works");
    auth.release(&debt_session)
        .await
        .expect("credential moves into durable revoke custody");
    assert!(!auth
        .release_settled(&debt_session)
        .await
        .expect("unsettled custody is reported conservatively"));
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
async fn runtime_delivery_prepare_retargets_pending_to_h2_and_fences_h1_confirmation() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_h2_retargets_pending".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let h1 = delivery_target(7, "runtime-h1");
    let h2 = delivery_target(8, "runtime-h2");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, &scope)),
        Some(credential("pending-b", "value-b", &owner, &scope)),
    );
    custody.pending_target = Some(h1.clone());
    store.insert_credential(custody).await.expect("store works");

    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &h2)
            .await
            .expect("H2 preparation persists its exact target"),
        Some("value-b".into()),
        "a successor runtime selects pending B instead of ordinary current A"
    );
    let rebound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(rebound.pending_target.as_ref(), Some(&h2));
    assert_eq!(rebound.generation, 1, "retargeting is one durable CAS");

    assert!(!auth
        .confirm_credential_delivery(&session, &owner, &services, "value-b", &h1)
        .await
        .expect("the stale acknowledgement fails closed"));
    let unchanged = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        unchanged.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert_eq!(
        unchanged.pending.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert_eq!(unchanged.pending_target.as_ref(), Some(&h2));
    assert!(unchanged.revoke_debt.is_empty());
    assert_eq!(unchanged.generation, 1);
}

#[tokio::test]
async fn runtime_delivery_prepare_returns_current_b_after_h1_promotion_wins() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_h1_promotion_wins".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let h1 = delivery_target(10, "runtime-h1");
    let h2 = delivery_target(11, "runtime-h2");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, &scope)),
        Some(credential("pending-b", "value-b", &owner, &scope)),
    );
    custody.pending_target = Some(h1.clone());
    store.insert_credential(custody).await.expect("store works");

    assert!(auth
        .confirm_credential_delivery(&session, &owner, &services, "value-b", &h1)
        .await
        .expect("H1 acknowledgement promotes its correlated value"));
    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &h2)
            .await
            .expect("H2 preparation re-reads custody after the winning promotion"),
        Some("value-b".into()),
        "H2 sees newly current B instead of stale A"
    );

    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(promoted.pending.is_none());
    assert!(promoted.pending_target.is_none());
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
    assert_eq!(
        promoted.generation, 1,
        "preparing already-current B performs no second CAS"
    );
}

#[tokio::test]
async fn runtime_delivery_h2_confirmation_queues_a_only_after_correlated_acknowledgement() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_h2_correlated_ack".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let h1 = delivery_target(13, "runtime-h1");
    let h2 = delivery_target(14, "runtime-h2");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, &scope)),
        Some(credential("pending-b", "value-b", &owner, &scope)),
    );
    custody.pending_target = Some(h1);
    store.insert_credential(custody).await.expect("store works");

    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &h2)
            .await
            .expect("H2 preparation succeeds"),
        Some("value-b".into())
    );
    let prepared = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        prepared.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a"),
        "selection alone does not promote B"
    );
    assert_eq!(
        prepared.pending.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert_eq!(prepared.pending_target.as_ref(), Some(&h2));
    assert!(prepared.revoke_debt.is_empty());
    assert_eq!(prepared.generation, 1);

    assert!(auth
        .confirm_credential_delivery(&session, &owner, &services, "value-b", &h2)
        .await
        .expect("H2's correlated acknowledgement promotes B"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(promoted.pending.is_none());
    assert!(promoted.pending_target.is_none());
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
    assert_eq!(promoted.generation, 2);
}

#[tokio::test]
async fn abandoned_h1_binding_is_fenced_before_fresh_c_is_delivered_to_h2() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    )))
    .await;
    let owner = SubjectId("owner".into());
    store_owner_token(&auth, &owner).await;
    let session = SessionId("sesn_stale_b_to_fresh_c".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let h1 = delivery_target(20, "runtime-h1");
    let h2 = delivery_target(21, "runtime-h2");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, &scope)),
        Some(credential_expiring_at(
            "pending-b",
            "value-b",
            &owner,
            &scope,
            SystemTime::now() + CREDENTIAL_EXPIRY_MARGIN,
        )),
    );
    custody.pending_target = Some(h1.clone());
    store.insert_credential(custody).await.expect("store works");

    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &h2)
            .await
            .expect("H2 authority fences stale B before selecting a value"),
        Some("full-key-1".into())
    );
    assert_eq!(mints.load(Ordering::SeqCst), 1);
    let reminted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        reminted.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a"),
        "A stays current until exact delivery of C"
    );
    assert_eq!(
        reminted.pending.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert_eq!(reminted.pending_target.as_ref(), Some(&h2));
    assert_eq!(reminted.revoke_debt.len(), 1);
    assert_eq!(reminted.revoke_debt[0].id, "pending-b");

    assert!(!auth
        .confirm_credential_delivery(&session, &owner, &services, "value-b", &h1)
        .await
        .expect("an abandoned H1 acknowledgement cannot promote retired B"));
    let bound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(bound.pending_target.as_ref(), Some(&h2));
    assert_eq!(
        bound.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );

    assert!(auth
        .confirm_credential_delivery(&session, &owner, &services, "full-key-1", &h2)
        .await
        .expect("H2 acknowledgement promotes exact C"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert!(promoted.pending.is_none());
    assert!(promoted.pending_target.is_none());
    let debt_ids: Vec<_> = promoted
        .revoke_debt
        .iter()
        .map(|debt| debt.id.as_str())
        .collect();
    assert_eq!(debt_ids, ["current-a", "pending-b"]);
    server.abort();
}

#[tokio::test]
async fn expired_current_is_never_selected_and_remains_current_until_fresh_c_lands() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    )))
    .await;
    let owner = SubjectId("owner".into());
    store_owner_token(&auth, &owner).await;
    let session = SessionId("sesn_expired_current".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let h2 = delivery_target(31, "runtime-h2");
    store
        .insert_credential(state(
            &session,
            Some(credential_expiring_at(
                "current-a",
                "value-a",
                &owner,
                &scope,
                SystemTime::now() - Duration::from_secs(1),
            )),
            None,
        ))
        .await
        .expect("store works");

    let expired_confirmation_budget = AuthOperationBudget::standalone(10);
    assert!(!auth
        .confirm_reissue_delivery_with_budget(
            &session,
            "value-a",
            &h2,
            credential_replace_before(),
            &expired_confirmation_budget,
        )
        .await
        .expect("an idempotent retry refuses an expired exact current value"));
    let still_current = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        still_current.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert!(still_current.pending.is_none());
    assert!(still_current.revoke_debt.is_empty());

    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &h2)
            .await
            .expect("an expired current value causes a fresh replacement"),
        Some("full-key-1".into())
    );
    assert_eq!(mints.load(Ordering::SeqCst), 1);
    let prepared = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        prepared.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a"),
        "expired A remains custody evidence until C promotion"
    );
    assert_eq!(
        prepared.pending.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert_eq!(prepared.pending_target.as_ref(), Some(&h2));
    assert!(prepared.revoke_debt.is_empty());
    server.abort();
}

#[tokio::test]
async fn binding_rechecks_unbound_pending_expiry() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let target = delivery_target(40, "runtime-h2");
    let session = SessionId("sesn_expiry_recheck_bind".into());
    store
        .insert_credential(state(
            &session,
            Some(credential("current-a", "value-a", &owner, "scope")),
            Some(credential_expiring_at(
                "pending-b",
                "value-b",
                &owner,
                "scope",
                SystemTime::now() + CREDENTIAL_EXPIRY_MARGIN,
            )),
        ))
        .await
        .expect("store works");
    let budget = AuthOperationBudget::standalone(10);
    assert!(!auth
        .prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &budget,
        )
        .await
        .expect("binding recheck settles stale unbound B"));

    let settled = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        settled.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert!(settled.pending.is_none());
    assert!(settled.pending_target.is_none());
    assert_eq!(settled.revoke_debt.len(), 1);
    assert_eq!(settled.revoke_debt[0].id, "pending-b");
}

#[tokio::test]
async fn newer_reissue_cutoff_between_binding_and_ack_cannot_retire_pending() {
    let store = Arc::new(MemoryAuthStore::default());
    let delivering = auth(store.clone());
    let sweeping = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_bound_pending_cutoff_race".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let target = delivery_target(41, "runtime-h2");
    let first_cutoff = credential_replace_before();
    store
        .insert_credential(state(
            &session,
            Some(credential("current-a", "value-a", &owner, &scope)),
            Some(credential_expiring_at(
                "pending-b",
                "value-b",
                &owner,
                &scope,
                first_cutoff + Duration::from_millis(1),
            )),
        ))
        .await
        .expect("store works");

    let delivery_budget = AuthOperationBudget::standalone(10);
    assert!(delivering
        .prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            first_cutoff,
            &delivery_budget,
        )
        .await
        .expect("B binds before leaving custody"));

    let newer_cutoff = first_cutoff + Duration::from_secs(1);
    let sweep_budget = AuthOperationBudget::standalone(crate::auth::AUTH_REISSUE_ATTEMPT_BUDGET);
    assert_eq!(
        sweeping
            .reissue_with_budget(&session, &owner, &services, newer_cutoff, sweep_budget,)
            .await
            .expect("the newer sweep defers to B's durable target binding"),
        Some("value-b".into())
    );
    let still_bound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(still_bound.pending_target.as_ref(), Some(&target));
    assert!(still_bound.revoke_debt.is_empty());

    assert!(delivering
        .confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            first_cutoff,
            &delivery_budget,
        )
        .await
        .expect("the exact acknowledgement remains promotable"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(promoted.pending.is_none());
    assert!(promoted.pending_target.is_none());
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
}

#[tokio::test]
async fn same_runtime_retry_finishes_bound_pending_across_a_newer_cutoff() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_same_target_cutoff_retry".into());
    let target = delivery_target(42, "runtime-h2");
    let first_cutoff = credential_replace_before();
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, "scope")),
        Some(credential_expiring_at(
            "pending-b",
            "value-b",
            &owner,
            "scope",
            first_cutoff + Duration::from_millis(1),
        )),
    );
    custody.pending_target = Some(target.clone());
    store.insert_credential(custody).await.expect("store works");

    let newer_cutoff = first_cutoff + Duration::from_secs(1);
    let budget = AuthOperationBudget::standalone(10);
    assert!(auth
        .prepare_reissue_delivery_with_budget(&session, "value-b", &target, newer_cutoff, &budget,)
        .await
        .expect("the same runtime target may retry its bound B"));
    let still_bound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(still_bound.pending_target.as_ref(), Some(&target));
    assert!(still_bound.revoke_debt.is_empty());

    assert!(auth
        .confirm_reissue_delivery_with_budget(&session, "value-b", &target, newer_cutoff, &budget,)
        .await
        .expect("the same target's exact ACK settles hard-valid B"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(promoted.pending.is_none());
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
}

#[tokio::test]
async fn one_frozen_cutoff_carries_from_binding_through_promotion() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_frozen_delivery_cutoff".into());
    let target = delivery_target(45, "runtime-h2");
    let replace_before = credential_replace_before();
    store
        .insert_credential(state(
            &session,
            Some(credential("current-a", "value-a", &owner, "scope")),
            Some(credential_expiring_at(
                "pending-b",
                "value-b",
                &owner,
                "scope",
                replace_before + Duration::from_nanos(1),
            )),
        ))
        .await
        .expect("store works");

    let budget = AuthOperationBudget::standalone(10);
    assert!(
        auth.prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            replace_before,
            &budget,
        )
        .await
        .expect("B is eligible at the attempt's frozen boundary")
    );
    let rebound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert!(
        !credential_is_deliverable(
            rebound.pending.as_ref().expect("B remains pending"),
            credential_replace_before(),
        ),
        "a separately recomputed cutoff would create the post-ACK revocation race"
    );

    assert!(
        auth.confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            replace_before,
            &budget,
        )
        .await
        .expect("promotion reuses the pre-send boundary")
    );
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
}

#[tokio::test]
async fn runtime_confirmation_uses_its_recorded_prepare_cutoff() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_runtime_delivery_cutoff".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let target = delivery_target(46, "runtime-h2");
    store
        .insert_credential(state(
            &session,
            Some(credential("current-a", "value-a", &owner, &scope)),
            Some(credential_expiring_at(
                "pending-b",
                "value-b",
                &owner,
                &scope,
                SystemTime::now() + CREDENTIAL_EXPIRY_MARGIN + Duration::from_secs(30),
            )),
        ))
        .await
        .expect("store works");

    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &target)
            .await
            .expect("pending B is eligible and bound at preparation"),
        Some("value-b".into())
    );
    let replace_before = auth
        .credential_delivery_cutoff(&session, &target)
        .await
        .expect("preparation records pending B's frozen cutoff");
    let mut custody = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    let generation = custody.generation;
    custody.generation += 1;
    custody
        .pending
        .as_mut()
        .expect("B remains pending")
        .expires_at = Some(replace_before + Duration::from_millis(1));
    assert!(store
        .replace_credential(generation, custody)
        .await
        .expect("boundary expiry persists"));
    tokio::time::sleep(Duration::from_millis(5)).await;

    let latest = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert!(
        !credential_is_deliverable(
            latest.pending.as_ref().expect("B remains pending"),
            credential_replace_before(),
        ),
        "a newly computed confirmation cutoff would reject B"
    );

    assert!(auth
        .confirm_credential_delivery(&session, &owner, &services, "value-b", &target)
        .await
        .expect("runtime acknowledgement uses its preparation boundary"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(auth
        .credential_delivery_cutoff(&session, &target)
        .await
        .is_none());
}

#[tokio::test]
async fn runtime_current_confirmation_uses_its_recorded_prepare_cutoff() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_runtime_current_delivery_cutoff".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let target = delivery_target(47, "runtime-h2");
    store
        .insert_credential(state(
            &session,
            Some(credential_expiring_at(
                "current-a",
                "value-a",
                &owner,
                &scope,
                SystemTime::now() + CREDENTIAL_EXPIRY_MARGIN + Duration::from_secs(30),
            )),
            None,
        ))
        .await
        .expect("store works");

    assert_eq!(
        auth.prepare_credential_delivery(&session, &owner, &services, None, &target)
            .await
            .expect("current A is eligible at preparation"),
        Some("value-a".into())
    );
    let replace_before = auth
        .credential_delivery_cutoff(&session, &target)
        .await
        .expect("preparation records current A's frozen cutoff");
    let mut custody = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    let generation = custody.generation;
    custody.generation += 1;
    custody
        .current
        .as_mut()
        .expect("A remains current")
        .expires_at = Some(replace_before + Duration::from_millis(1));
    assert!(store
        .replace_credential(generation, custody)
        .await
        .expect("boundary expiry persists"));
    tokio::time::sleep(Duration::from_millis(5)).await;

    let latest = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert!(
        !credential_is_deliverable(
            latest.current.as_ref().expect("A remains current"),
            credential_replace_before(),
        ),
        "a newly computed confirmation cutoff would reject A"
    );
    assert!(auth
        .confirm_credential_delivery(&session, &owner, &services, "value-a", &target)
        .await
        .expect("confirmation reuses A's preparation boundary"));
    assert!(auth
        .credential_delivery_cutoff(&session, &target)
        .await
        .is_none());
}

#[tokio::test]
async fn runtime_fallback_delivery_does_not_record_an_expiry_cutoff() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store);
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_runtime_fallback_delivery".into());
    let services = [ServiceId::new("model")];
    let target = delivery_target(48, "runtime-h2");

    assert_eq!(
        auth.prepare_credential_delivery(
            &session,
            &owner,
            &services,
            Some(&CallerCredential::from("subject-wide")),
            &target,
        )
        .await
        .expect("uncustodied fallback remains available"),
        Some("subject-wide".into())
    );
    assert!(
        auth.credential_delivery_cutoff(&session, &target)
            .await
            .is_none(),
        "fallback values do not participate in expiring durable custody"
    );
}

#[tokio::test]
async fn stale_pending_cleanup_errors_return_no_value_and_preserve_current_a() {
    let (base, mints, server) = mint_platform().await;
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    auth.use_issuer(Arc::new(crate::auth::keys::KeyIssuer::new(
        reqwest::Client::new(),
        base,
        "test",
    )))
    .await;
    let owner = SubjectId("owner".into());
    store_owner_token(&auth, &owner).await;
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);

    let failed_session = SessionId("sesn_stale_cleanup_store_failure".into());
    let failed = state(
        &failed_session,
        Some(credential("failed-current-a", "failed-a", &owner, &scope)),
        Some(credential_expiring_at(
            "failed-pending-b",
            "failed-b",
            &owner,
            &scope,
            SystemTime::now() - Duration::from_secs(1),
        )),
    );
    store.insert_credential(failed).await.expect("store works");
    store.fail_credential_replace_on_call(1);

    assert!(matches!(
        auth.reissue(&failed_session, &owner, &services).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(mints.load(Ordering::SeqCst), 0, "an error mints nothing");
    let unchanged = store
        .credential(&failed_session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        unchanged.current.as_ref().map(|held| held.id.as_str()),
        Some("failed-current-a")
    );
    assert_eq!(
        unchanged.pending.as_ref().map(|held| held.id.as_str()),
        Some("failed-pending-b")
    );
    assert!(unchanged.pending_target.is_none());
    assert!(unchanged.revoke_debt.is_empty());

    store.clear_credential_replace_failure();
    let ambiguous_session = SessionId("sesn_stale_cleanup_lost_response".into());
    let ambiguous = state(
        &ambiguous_session,
        Some(credential(
            "ambiguous-current-a",
            "ambiguous-a",
            &owner,
            &scope,
        )),
        Some(credential_expiring_at(
            "ambiguous-pending-b",
            "ambiguous-b",
            &owner,
            &scope,
            SystemTime::now() - Duration::from_secs(1),
        )),
    );
    store
        .insert_credential(ambiguous)
        .await
        .expect("store works");
    store.lose_credential_replace_response_on_call(1);

    assert!(matches!(
        auth.reissue(&ambiguous_session, &owner, &services).await,
        Err(StoreError::Unavailable(_))
    ));
    assert_eq!(
        mints.load(Ordering::SeqCst),
        0,
        "ambiguous cleanup sends and mints nothing in the same attempt"
    );
    let applied = store
        .credential(&ambiguous_session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        applied.current.as_ref().map(|held| held.id.as_str()),
        Some("ambiguous-current-a")
    );
    assert!(applied.pending.is_none());
    assert!(applied.pending_target.is_none());
    assert_eq!(applied.revoke_debt.len(), 1);
    assert_eq!(applied.revoke_debt[0].id, "ambiguous-pending-b");
    assert_eq!(
        applied.generation, 1,
        "stale B retirement is one atomic generation fence"
    );

    store.clear_credential_replace_failure();
    assert_eq!(
        auth.reissue(&ambiguous_session, &owner, &services)
            .await
            .expect("retry rereads the applied cleanup and mints C"),
        Some("full-key-1".into())
    );
    let retried = store
        .credential(&ambiguous_session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        retried.current.as_ref().map(|held| held.id.as_str()),
        Some("ambiguous-current-a")
    );
    assert_eq!(
        retried.pending.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert_eq!(
        retried
            .revoke_debt
            .iter()
            .filter(|debt| debt.id == "ambiguous-pending-b")
            .count(),
        1,
        "retry does not duplicate stale-B cleanup debt"
    );
    server.abort();
}

#[tokio::test]
async fn competing_replicas_cleanup_unbound_stale_b_and_mint_exactly_one_c() {
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
    let session = SessionId("sesn_competing_stale_cleanup".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, &scope)),
        Some(credential_expiring_at(
            "pending-b",
            "value-b",
            &owner,
            &scope,
            SystemTime::now() - Duration::from_secs(1),
        )),
    );
    store.insert_credential(custody).await.expect("store works");

    let (first_result, second_result) = tokio::join!(
        first.reissue(&session, &owner, &services),
        second.reissue(&session, &owner, &services),
    );
    assert_eq!(
        first_result.expect("first replica converges"),
        Some("full-key-1".into())
    );
    assert_eq!(
        second_result.expect("second replica rereads the winner"),
        Some("full-key-1".into())
    );
    assert_eq!(mints.load(Ordering::SeqCst), 1);

    let converged = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        converged.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert_eq!(
        converged.pending.as_ref().map(|held| held.id.as_str()),
        Some("provider-key-1")
    );
    assert!(converged.pending_target.is_none());
    assert_eq!(
        converged
            .revoke_debt
            .iter()
            .filter(|debt| debt.id == "pending-b")
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn delivery_confirmation_promotes_only_the_exact_pending_value() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_exact_delivery".into());
    let target = delivery_target(4, "runtime-h1");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, "scope")),
        Some(credential("pending-b", "value-b", &owner, "scope")),
    );
    custody.pending_target = Some(target.clone());
    store.insert_credential(custody).await.expect("store works");

    let budget = AuthOperationBudget::standalone(10);
    assert!(!auth
        .confirm_reissue_delivery_with_budget(
            &session,
            "different-value",
            &target,
            credential_replace_before(),
            &budget,
        )
        .await
        .expect("mismatch is a closed answer"));
    let unchanged = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert_eq!(
        unchanged.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert_eq!(
        unchanged.pending.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert_eq!(unchanged.pending_target.as_ref(), Some(&target));
    assert!(unchanged.revoke_debt.is_empty());
    assert_eq!(unchanged.generation, 0);
}

#[tokio::test]
async fn pending_delivery_binding_persists_and_retargets_only_the_same_value() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_target_binding".into());
    store
        .insert_credential(state(
            &session,
            Some(credential("current-a", "value-a", &owner, "scope")),
            Some(credential("pending-b", "value-b", &owner, "scope")),
        ))
        .await
        .expect("store works");

    let h1 = delivery_target(3, "runtime-h1");
    let first_budget = AuthOperationBudget::standalone(10);
    assert!(auth
        .prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &h1,
            credential_replace_before(),
            &first_budget,
        )
        .await
        .expect("H1 target binds"));
    let bound_h1 = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(bound_h1.pending_target.as_ref(), Some(&h1));
    assert_eq!(bound_h1.generation, 1);

    let h2 = delivery_target(4, "runtime-h2");
    let retry_budget = AuthOperationBudget::standalone(10);
    assert!(auth
        .prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &h2,
            credential_replace_before(),
            &retry_budget,
        )
        .await
        .expect("the same pending value can move to H2"));
    assert!(!auth
        .prepare_reissue_delivery_with_budget(
            &session,
            "different-value",
            &h1,
            credential_replace_before(),
            &retry_budget,
        )
        .await
        .expect("a different value fails closed"));
    let rebound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(rebound.pending_target.as_ref(), Some(&h2));
    assert_eq!(rebound.generation, 2);
}

#[tokio::test]
async fn a_lost_target_binding_response_is_idempotent_on_exact_retry() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_lost_target_binding_response".into());
    store
        .insert_credential(state(
            &session,
            Some(credential("current-a", "value-a", &owner, "scope")),
            Some(credential("pending-b", "value-b", &owner, "scope")),
        ))
        .await
        .expect("store works");
    store.lose_credential_replace_response_on_call(1);

    let target = delivery_target(3, "runtime-h1");
    let first_budget = AuthOperationBudget::standalone(10);
    assert!(matches!(
        auth.prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &first_budget,
        )
        .await,
        Err(StoreError::Unavailable(_))
    ));
    store.clear_credential_replace_failure();

    let retry_budget = AuthOperationBudget::standalone(10);
    assert!(auth
        .prepare_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &retry_budget,
        )
        .await
        .expect("already-bound exact target confirms idempotently"));
    let bound = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(bound.generation, 1, "retry performs no second CAS");
    assert_eq!(bound.pending_target.as_ref(), Some(&target));
}

#[tokio::test]
async fn acknowledgement_for_another_runtime_target_cannot_promote_pending() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_wrong_runtime_ack".into());
    let h1 = delivery_target(5, "runtime-h1");
    let h2 = delivery_target(6, "runtime-h2");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, "scope")),
        Some(credential("pending-b", "value-b", &owner, "scope")),
    );
    custody.pending_target = Some(h2.clone());
    store.insert_credential(custody).await.expect("store works");

    let budget = AuthOperationBudget::standalone(10);
    assert!(!auth
        .confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &h1,
            credential_replace_before(),
            &budget,
        )
        .await
        .expect("wrong-target acknowledgement fails closed"));
    let unchanged = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("custody remains");
    assert_eq!(
        unchanged.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert_eq!(
        unchanged.pending.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert_eq!(unchanged.pending_target.as_ref(), Some(&h2));
    assert!(unchanged.revoke_debt.is_empty());
    assert_eq!(unchanged.generation, 0);
}

#[tokio::test]
async fn failed_exact_promotion_leaves_b_pending_and_queues_no_predecessor_debt() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_failed_exact_promotion".into());
    let services = [ServiceId::new("model")];
    let scope = scope_fingerprint(&services);
    let target = delivery_target(7, "runtime-h1");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, &scope)),
        Some(credential("pending-b", "value-b", &owner, &scope)),
    );
    custody.pending_target = Some(target.clone());
    store.insert_credential(custody).await.expect("store works");
    store.fail_credential_replace_on_call(1);

    let failed_budget = AuthOperationBudget::standalone(10);
    assert!(matches!(
        auth.confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &failed_budget,
        )
        .await,
        Err(StoreError::Unavailable(_))
    ));
    let failed = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert_eq!(
        failed.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert_eq!(
        failed.pending.as_ref().map(|held| held.id.as_str()),
        Some("pending-b"),
        "the retry must resend B rather than selecting ordinary current A"
    );
    assert_eq!(failed.pending_target.as_ref(), Some(&target));
    assert!(failed.revoke_debt.is_empty());
    assert_eq!(failed.generation, 0);
    assert_eq!(
        auth.reissue(&session, &owner, &services)
            .await
            .expect("retry selection reads pending custody"),
        Some("value-b".into()),
        "retry resends pending B rather than ordinary current A"
    );

    store.clear_credential_replace_failure();
    let retry_budget = AuthOperationBudget::standalone(10);
    assert!(auth
        .confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &retry_budget,
        )
        .await
        .expect("the same delivered B remains promotable"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(promoted.pending.is_none());
    assert!(promoted.pending_target.is_none());
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
}

#[tokio::test]
async fn a_lost_promotion_response_is_idempotent_on_exact_value_retry() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_lost_promotion_response".into());
    let target = delivery_target(9, "runtime-h1");
    let mut custody = state(
        &session,
        Some(credential("current-a", "value-a", &owner, "scope")),
        Some(credential("pending-b", "value-b", &owner, "scope")),
    );
    custody.pending_target = Some(target.clone());
    store.insert_credential(custody).await.expect("store works");
    store.lose_credential_replace_response_on_call(1);

    let first_budget = AuthOperationBudget::standalone(10);
    assert!(matches!(
        auth.confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &first_budget,
        )
        .await,
        Err(StoreError::Unavailable(_))
    ));
    store.clear_credential_replace_failure();

    let retry_budget = AuthOperationBudget::standalone(10);
    assert!(auth
        .confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &retry_budget,
        )
        .await
        .expect("already-current exact B confirms idempotently"));
    let promoted = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert_eq!(promoted.generation, 1, "retry performs no second CAS");
    assert_eq!(
        promoted.current.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert!(promoted.pending.is_none());
    assert_eq!(promoted.revoke_debt.len(), 1);
    assert_eq!(promoted.revoke_debt[0].id, "current-a");
}

#[tokio::test]
async fn retired_custody_refuses_delivery_confirmation_without_mutation() {
    let store = Arc::new(MemoryAuthStore::default());
    let auth = auth(store.clone());
    let owner = SubjectId("owner".into());
    let session = SessionId("sesn_retired_delivery".into());
    let mut retired = state(
        &session,
        Some(credential("current-a", "value-a", &owner, "scope")),
        Some(credential("pending-b", "value-b", &owner, "scope")),
    );
    let target = delivery_target(11, "runtime-h1");
    retired.pending_target = Some(target.clone());
    retired.retired = true;
    store.insert_credential(retired).await.expect("store works");

    let budget = AuthOperationBudget::standalone(10);
    assert!(!auth
        .confirm_reissue_delivery_with_budget(
            &session,
            "value-b",
            &target,
            credential_replace_before(),
            &budget,
        )
        .await
        .expect("retired state fails closed"));
    let unchanged = store
        .credential(&session)
        .await
        .expect("store works")
        .expect("state exists");
    assert!(unchanged.retired);
    assert_eq!(
        unchanged.current.as_ref().map(|held| held.id.as_str()),
        Some("current-a")
    );
    assert_eq!(
        unchanged.pending.as_ref().map(|held| held.id.as_str()),
        Some("pending-b")
    );
    assert_eq!(unchanged.pending_target.as_ref(), Some(&target));
    assert!(unchanged.revoke_debt.is_empty());
    assert_eq!(unchanged.generation, 0);
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
