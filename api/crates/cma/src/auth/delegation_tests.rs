use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime};

use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap},
    routing::{get, post},
    Json, Router,
};
use layer2_core::store::StoreError;
use layer2_core::{CredentialSource as _, ServiceId, SessionId, SubjectId};

use super::*;
use crate::auth::persistence::{MemoryAuthStore, SecretValue};
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
        current,
        pending,
        revoke_debt: Vec::new(),
        lease: None,
        retired: false,
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
            .credential_for(&session, &owner, &services)
            .await
            .expect("credential loads")
            .as_deref(),
        Some("session-secret")
    );
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
            auth.credential_for(&session, &owner, &services)
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
            auth.credential_for(&session, &owner, &services)
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
            .credential_for(&session, &owner, &services)
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
        async move { auth.credential_for(&session, &owner, &services).await }
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
