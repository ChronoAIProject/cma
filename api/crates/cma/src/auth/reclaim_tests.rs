//! Durable revoke debt and reconciliation failure boundaries.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    routing::delete,
    Router,
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

struct UnavailableSessions;

#[async_trait::async_trait]
impl SessionStore for UnavailableSessions {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(&self, _id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "injected session store outage".into(),
        ))
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

fn unavailable_session_service() -> Arc<SessionService> {
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
        Arc::new(UnavailableSessions),
        SessionPorts::default(),
        environments,
        Deployment::default(),
    )
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
