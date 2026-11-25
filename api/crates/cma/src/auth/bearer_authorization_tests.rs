use crate::auth::identity;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};

use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use tokio::sync::{RwLock, Semaphore};

use super::*;
use crate::auth::{permissions::PermissionEvidence, AuthMode};

#[path = "bearer_authorization_tests/budget_tests.rs"]
mod budget_tests;
#[path = "bearer_authorization_tests/capacity_diagnostic_tests.rs"]
mod capacity_diagnostic_tests;
#[path = "bearer_authorization_tests/epoch_tests.rs"]
mod epoch_tests;

struct Gate {
    release: Semaphore,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            release: Semaphore::new(0),
        }
    }
}

struct Control {
    userinfo_calls: AtomicUsize,
    introspection_calls: AtomicUsize,
    status: AtomicU16,
    active: AtomicBool,
    token_subjects: AtomicBool,
    gate: RwLock<Option<Arc<Gate>>>,
    arrivals: Semaphore,
    userinfo_body: RwLock<Option<serde_json::Value>>,
    introspection_body: RwLock<Option<serde_json::Value>>,
}

async fn userinfo(
    State(control): State<Arc<Control>>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    control.userinfo_calls.fetch_add(1, Ordering::SeqCst);
    let subject = if control.token_subjects.load(Ordering::SeqCst) {
        format!(
            "fixture-subject-{}",
            headers["authorization"]
                .to_str()
                .unwrap()
                .strip_prefix("Bearer ")
                .unwrap()
        )
    } else {
        "fixture-owner".into()
    };
    Json(
        control
            .userinfo_body
            .read()
            .await
            .clone()
            .unwrap_or_else(|| json!({"sub":subject})),
    )
}

async fn introspection(
    State(control): State<Arc<Control>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    control.introspection_calls.fetch_add(1, Ordering::SeqCst);
    control.arrivals.add_permits(1);
    let gate = control.gate.read().await.clone();
    if let Some(gate) = gate {
        gate.release.acquire().await.unwrap().forget();
    }
    let subject = if control.token_subjects.load(Ordering::SeqCst) {
        format!("fixture-subject-{}", form["token"])
    } else {
        "fixture-owner".into()
    };
    let status = StatusCode::from_u16(control.status.load(Ordering::SeqCst)).unwrap();
    let mut response = (
        status,
        Json(
            control
                .introspection_body
                .read()
                .await
                .clone()
                .unwrap_or_else(|| {
                    json!({"sub":subject, "active":control.active.load(Ordering::SeqCst),
            "permissions":["cma:sandbox:use"]})
                }),
        ),
    )
        .into_response();
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, "60".parse().unwrap());
    }
    response
}

struct Fixture {
    auth: Arc<AuthState>,
    control: Arc<Control>,
    metadata: ProviderMetadata,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let control = Arc::new(Control {
            userinfo_calls: AtomicUsize::new(0),
            introspection_calls: AtomicUsize::new(0),
            status: AtomicU16::new(200),
            active: AtomicBool::new(true),
            token_subjects: AtomicBool::new(false),
            gate: RwLock::new(None),
            arrivals: Semaphore::new(0),
            userinfo_body: RwLock::new(None),
            introspection_body: RwLock::new(None),
        });
        let router = Router::new()
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspection))
            .with_state(control.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let auth = AuthState::in_memory(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "fixture-client".into(),
                client_secret: "fixture-secret".into(),
                public_url: "https://cma.example.test".into(),
            },
            None,
        );
        let metadata = serde_json::from_value(json!({
            "authorization_endpoint":format!("{base}/authorize"),
            "token_endpoint":format!("{base}/token"),
            "userinfo_endpoint":format!("{base}/userinfo"),
            "introspection_endpoint":format!("{base}/introspect"),
        }))
        .unwrap();
        *auth.metadata.write().await = Some(metadata);
        let metadata = auth.metadata().await.unwrap();
        Self {
            auth,
            control,
            metadata,
            server,
        }
    }

    async fn block(&self) -> Arc<Gate> {
        let gate = Arc::new(Gate::default());
        *self.control.gate.write().await = Some(gate.clone());
        gate
    }

    fn verify(
        &self,
        credential: &'static str,
    ) -> tokio::task::JoinHandle<Result<Arc<VerifiedSubject>, ProviderVerificationError>> {
        let auth = self.auth.clone();
        let metadata = self.metadata.clone();
        tokio::spawn(async move { verify_subject(&auth, &metadata, credential).await })
    }

    async fn callers(&self, credential: &str, count: usize) {
        let key = SecretDigest::identify("independent-bearer-verification", credential);
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let joined = self
                    .auth
                    .bearer_verifications
                    .entries
                    .lock()
                    .await
                    .get(&key)
                    .is_some_and(|flight| flight.strong_count() >= count);
                if joined {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all concurrent callers join the pending verification");
    }

    async fn arrived(&self, count: u32) {
        tokio::time::timeout(
            Duration::from_secs(2),
            self.control.arrivals.acquire_many(count),
        )
        .await
        .unwrap()
        .unwrap()
        .forget();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn overlapping_public_reads_share_verification_and_later_revocation_is_fresh() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let requests: Vec<_> = (0..9)
        .map(|_| {
            let auth = f.auth.clone();
            tokio::spawn(async move {
                identity::request_for_bearer(&auth, "fixture-access".into()).await
            })
        })
        .collect();
    f.arrived(1).await;
    f.callers("fixture-access", 9).await;
    gate.release.add_permits(9);
    for request in requests {
        let verified = request.await.unwrap().unwrap().unwrap();
        assert_eq!(verified.subject.as_str(), "fixture-owner");
        assert!(verified
            .authorization
            .allows(layer2_core::Capability::UseOwnSandboxes));
    }
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 1);
    *f.control.gate.write().await = None;
    f.control.active.store(false, Ordering::SeqCst);
    assert!(
        identity::request_for_bearer(&f.auth, "fixture-access".into())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
    f.control.active.store(true, Ordering::SeqCst);
    assert!(
        identity::request_for_bearer(&f.auth, "fixture-access".into())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn different_bearers_never_share_provider_evidence() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let first = f.verify("fixture-first");
    let second = f.verify("fixture-second");
    f.arrived(2).await;
    gate.release.add_permits(9);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn overlapping_failures_share_flight_but_only_throttle_backoff_survives_completion() {
    let f = Fixture::new().await;
    for status in [429, 401] {
        f.control.status.store(status, Ordering::SeqCst);
        let gate = f.block().await;
        let first = f.verify("fixture-access");
        f.arrived(1).await;
        let second = f.verify("fixture-access");
        f.callers("fixture-access", 2).await;
        gate.release.add_permits(9);
        for request in [first, second] {
            let result = request.await.unwrap();
            if status == 401 {
                assert!(matches!(
                    result,
                    Err(ProviderVerificationError::Rejected(_))
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(ProviderVerificationError::RateLimited(_))
                ));
            }
        }
        *f.control.gate.write().await = None;
        f.control.status.store(200, Ordering::SeqCst);
        if status == 429 {
            let before = f.control.introspection_calls.load(Ordering::SeqCst);
            assert!(matches!(
                verify_subject(&f.auth, &f.metadata, "fixture-access").await,
                Err(ProviderVerificationError::RateLimited(_))
            ));
            assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), before);
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(61)).await;
            tokio::time::resume();
        }
        let result = verify_subject(&f.auth, &f.metadata, "fixture-access")
            .await
            .unwrap();
        assert!(matches!(result.permissions, PermissionEvidence::Active(_)));
        f.arrived(1).await;
    }
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn cancelled_initializer_does_not_strand_waiter_or_later_verification() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let first = f.verify("fixture-access");
    f.arrived(1).await;
    let second = f.verify("fixture-access");
    f.callers("fixture-access", 2).await;
    *f.control.gate.write().await = None;
    first.abort();
    assert!(matches!(first.await, Err(error) if error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    gate.release.add_permits(9);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
    assert!(f.auth.bearer_verifications.entries.lock().await.is_empty());
}

#[tokio::test]
async fn abandoning_all_callers_and_provider_deadlines_do_not_leave_cached_evidence() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let first = f.verify("fixture-access");
    f.arrived(1).await;
    first.abort();
    assert!(matches!(first.await, Err(error) if error.is_cancelled()));
    let result = verify_with_timeout(
        &f.auth,
        &f.metadata,
        "fixture-access",
        Duration::from_millis(20),
    )
    .await;
    // The caller's outer timeout does not know whether a body has started.
    // Only the provider reader may classify a pre-response timeout as transient.
    assert!(matches!(
        result,
        Err(ProviderVerificationError::Unavailable(_))
    ));
    *f.control.gate.write().await = None;
    gate.release.add_permits(9);
    verify_subject(&f.auth, &f.metadata, "fixture-access")
        .await
        .unwrap();
    assert!(f.auth.bearer_verifications.entries.lock().await.is_empty());
}

#[tokio::test]
async fn capacity_is_bounded_and_completed_flight_cannot_replace_new_pending_work() {
    let flights = VerificationFlights::default();
    let mut retained = Vec::new();
    for index in 0..MAX_VERIFICATION_FLIGHTS {
        let key = SecretDigest::identify("fixture", &index.to_string());
        retained.push(flights.acquire(&key).await);
    }
    let overflow = SecretDigest::identify("fixture", "overflow");
    let unshared = flights.acquire(&overflow).await;
    assert!(!Arc::ptr_eq(&unshared, &flights.acquire(&overflow).await));
    assert_eq!(flights.entries.lock().await.len(), MAX_VERIFICATION_FLIGHTS);
    let key = SecretDigest::identify("fixture", "0");
    assert!(retained[0]
        .set(Err(ProviderVerificationError::Rejected(
            "fixture refusal".into(),
        )
        .into()))
        .is_ok());
    let replacement = flights.acquire(&key).await;
    assert!(!Arc::ptr_eq(&retained[0], &replacement));
    flights.release(&key, &retained[0]).await;
    assert!(Arc::ptr_eq(&replacement, &flights.acquire(&key).await));
}

#[tokio::test]
async fn every_single_flight_waiter_keeps_its_final_failure_reason() {
    use layer2_core::dependency_diagnostics::{capture, Dependency, Failure, Reason};
    let f = Fixture::new().await;
    f.control.status.store(429, Ordering::SeqCst);
    let gate = f.block().await;
    let requests: Vec<_> = (0..3)
        .map(|_| {
            let auth = f.auth.clone();
            tokio::spawn(async move {
                capture(identity::request_for_bearer(&auth, "fixture-access".into())).await
            })
        })
        .collect();
    f.arrived(1).await;
    f.callers("fixture-access", 3).await;
    gate.release.add_permits(3);
    for request in requests {
        let (result, failure) = request.await.unwrap();
        assert!(result.is_err());
        assert_eq!(
            failure,
            Some(Failure::new(
                Dependency::IdentityProvider,
                Reason::RateLimited
            ))
        );
    }
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn waiters_receive_the_same_immutable_receipt_but_later_calls_verify_again() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let requests: Vec<_> = (0..9).map(|_| f.verify("fixture-access")).collect();
    f.arrived(1).await;
    f.callers("fixture-access", 9).await;
    gate.release.add_permits(9);
    let mut receipts = Vec::new();
    for request in requests {
        receipts.push(request.await.unwrap().unwrap());
    }
    assert!(receipts
        .iter()
        .all(|receipt| Arc::ptr_eq(receipt, &receipts[0])));
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 1);
    *f.control.gate.write().await = None;
    let later = f.verify("fixture-access").await.unwrap().unwrap();
    assert!(!Arc::ptr_eq(&later, &receipts[0]));
    assert!(crate::auth::identity_lifetime::current(
        &f.auth,
        &receipts[0]
    ));
    assert!(crate::auth::identity_lifetime::current(&f.auth, &later));
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
    drop(receipts);
    drop(later);
    assert_eq!(f.auth.identity_store.active_receipts(), 0);
}
