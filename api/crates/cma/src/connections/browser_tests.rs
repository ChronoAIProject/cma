use super::*;
use crate::connections::{
    subscription_store::{PendingLogin, SubscriptionRecord, SubscriptionStore},
    SubscriptionError,
};
use layer2_core::trigger_domain::{
    BrokerHandoff, ServiceConnectionError, ServiceConnectionMetadata, ServiceRequirement,
};
use layer2_core::{Authorization, SubjectId};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};

struct Targets;
#[async_trait::async_trait]
impl ConnectionTargets for Targets {
    async fn services(
        &self,
        caller: &Caller,
        _: &ConnectionTarget,
    ) -> Result<Vec<ServiceId>, ConnectionError> {
        if caller.subject.as_str() != "owner" {
            return Err(ConnectionError::NotFound);
        }
        Ok(["api-github", CODEX_SUBSCRIPTION_CONNECTION]
            .map(ServiceId::new)
            .to_vec())
    }
}

struct Broker {
    rows: Mutex<Vec<ServiceRequirement>>,
    starts: AtomicUsize,
    read_gate: Mutex<Option<Arc<ReadGate>>>,
    requirements_error: AtomicBool,
    metadata_error: AtomicBool,
    delays: Mutex<(Duration, Duration)>,
}

struct ReadGate {
    requirements_started: AtomicUsize,
    metadata_started: AtomicUsize,
    requirements_release: tokio::sync::Semaphore,
    metadata_release: tokio::sync::Semaphore,
}

impl ReadGate {
    async fn observe(&self, requirements: bool) {
        if requirements {
            &self.requirements_started
        } else {
            &self.metadata_started
        }
        .fetch_add(1, Ordering::SeqCst);
        let release = if requirements {
            &self.requirements_release
        } else {
            &self.metadata_release
        };
        release.acquire().await.unwrap().forget();
    }
}
#[async_trait::async_trait]
impl ServiceConnectionBroker for Broker {
    async fn requirements(
        &self,
        _: &SubjectId,
        _: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        let delay = self.delays.lock().unwrap().0;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let gate = self.read_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.observe(true).await;
        }
        if self.requirements_error.load(Ordering::SeqCst) {
            return Err(ServiceConnectionError::Refused);
        }
        Ok(self.rows.lock().unwrap().clone())
    }
    async fn describe_services(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceConnectionMetadata>, ServiceConnectionError> {
        let delay = self.delays.lock().unwrap().1;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let gate = self.read_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.observe(false).await;
        }
        if self.metadata_error.load(Ordering::SeqCst) {
            return Err(ServiceConnectionError::Refused);
        }
        Ok(services
            .iter()
            .map(|service| ServiceConnectionMetadata {
                service: service.clone(),
                display_name: Some("GitHub".into()),
                connection_mode: ServiceConnectionMode::Hosted,
                setup_url: None,
            })
            .collect())
    }
    async fn create_handoff(
        &self,
        _: &SubjectId,
        _: &ServiceId,
        intent: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(BrokerHandoff {
            external_id: intent.into(),
            hosted_url: "https://nyx.example/connect/test".into(),
            expires_at: layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(600)),
        })
    }
    async fn handoff_state(
        &self,
        _: &SubjectId,
        _: &str,
        _: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        Ok(BrokerHandoffState::Pending)
    }
}
struct Store(Mutex<SubscriptionRecord>);
#[async_trait::async_trait]
impl SubscriptionStore for Store {
    async fn get(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<SubscriptionRecord>, SubscriptionError> {
        Ok(Some(self.0.lock().unwrap().clone()).filter(|record| record.owner == *owner))
    }
    async fn insert(&self, _: SubscriptionRecord) -> Result<bool, SubscriptionError> {
        Err(SubscriptionError::Unavailable)
    }
    async fn replace(
        &self,
        _: &SubscriptionRecord,
        _: SubscriptionRecord,
    ) -> Result<bool, SubscriptionError> {
        Err(SubscriptionError::Unavailable)
    }
}
fn fixture() -> (BrowserConnections, Arc<Broker>, Arc<Store>, Caller) {
    let caller = Caller::new(SubjectId::new("owner"), Authorization::none());
    let mut record = SubscriptionRecord::empty(caller.subject.clone());
    record.pending=Some(PendingLogin {
        intent:"new-login".into(),device:serde_json::from_value(serde_json::json!({"login_id":"native-login", "verification_url":"https://auth.openai.com/codex/device", "user_code":"ABCD-1234"})).unwrap(),
        expires_at:SystemTime::now()+Duration::from_secs(600),heartbeat_at:SystemTime::now(),worker:"test".into(),state:BrokerHandoffState::Pending,
    });
    let store = Arc::new(Store(Mutex::new(record)));
    let broker = Arc::new(Broker {
        rows: Mutex::new(
            ["api-github", CODEX_SUBSCRIPTION_CONNECTION]
                .map(|service| ServiceRequirement {
                    service: ServiceId::new(service),
                    state: ServiceRequirementState::Satisfied,
                })
                .to_vec(),
        ),
        starts: AtomicUsize::new(0),
        read_gate: Mutex::new(None),
        requirements_error: AtomicBool::new(false),
        metadata_error: AtomicBool::new(false),
        delays: Mutex::new((Duration::ZERO, Duration::ZERO)),
    });
    (
        BrowserConnections {
            targets: Arc::new(Targets),
            broker: broker.clone(),
            subscriptions: CodexSubscriptions::new(store.clone()),
            repository_access: None,
            repository_scope_check: false,
            github_connection_approval: false,
        },
        broker,
        store,
        caller,
    )
}
fn target() -> ConnectionTarget {
    ConnectionTarget::Agent {
        id: layer2_core::AgentId("agent".into()),
    }
}

#[tokio::test]
async fn repository_scope_check_is_opt_in_and_requires_the_callers_target() {
    use crate::auth::AuthMode;
    use serde_json::{json, Value};

    let (mut connections, _, _, caller) = fixture();
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: caller.subject.to_string(),
        },
        None,
    );
    connections.repository_access = Some(Arc::new(
        crate::trigger::CmaServiceConnectionBroker::from_auth_config(
            auth.fixture_mode(),
            None,
            auth.clone(),
        )
        .unwrap(),
    ));
    let legacy = json!({"target": target()});
    let requested = json!({"target": target(), "github_operation":"public_repository_write"});
    let recovery_only = Arc::new(connections);
    assert!(repository_access::report(
        State(recovery_only.clone()),
        AuthenticatedCaller(caller.clone()),
        Json(serde_json::from_value(legacy.clone()).unwrap()),
    )
    .await
    .is_ok());
    assert!(matches!(
        repository_access::report(
            State(recovery_only.clone()),
            AuthenticatedCaller(caller.clone()),
            Json(serde_json::from_value(requested.clone()).unwrap()),
        )
        .await,
        Err(ConnectionError::NotFound)
    ));
    let mut connections = Arc::into_inner(recovery_only).expect("the test owns the state");
    connections.repository_scope_check = true;
    let state = Arc::new(connections);
    for (body, expected) in [
        (legacy, json!({"owner":"owner"})),
        (
            requested.clone(),
            json!({
                "owner":"owner",
                "github_scope_check": {
                    "operation":"public_repository_write", "status":"verification_unavailable"
                }
            }),
        ),
    ] {
        let response = repository_access::report(
            State(state.clone()),
            AuthenticatedCaller(caller.clone()),
            Json(serde_json::from_value(body).unwrap()),
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
    }
    let selection_without_operation = json!({"target":target(), "github_connection_id":"selected"});
    assert!(matches!(
        repository_access::report(
            State(state.clone()),
            AuthenticatedCaller(caller),
            Json(serde_json::from_value(selection_without_operation).unwrap()),
        )
        .await,
        Err(ConnectionError::Invalid)
    ));
    let stranger = Caller::new(SubjectId::new("stranger"), Authorization::none());
    assert!(matches!(
        repository_access::report(
            State(state),
            AuthenticatedCaller(stranger),
            Json(serde_json::from_value(requested).unwrap()),
        )
        .await,
        Err(ConnectionError::NotFound)
    ));
}

pub(crate) fn authorization_fixture() -> Arc<BrowserConnections> {
    struct OwnerTargets;

    #[async_trait::async_trait]
    impl ConnectionTargets for OwnerTargets {
        async fn services(
            &self,
            caller: &Caller,
            selected: &ConnectionTarget,
        ) -> Result<Vec<ServiceId>, ConnectionError> {
            if caller.subject.as_str() != "owner" || *selected != target() {
                return Err(ConnectionError::NotFound);
            }
            Ok(["api-drive", "api-github", CODEX_SUBSCRIPTION_CONNECTION]
                .map(ServiceId::new)
                .to_vec())
        }
    }

    let (mut connections, _, _, _) = fixture();
    connections.targets = Arc::new(OwnerTargets);
    Arc::new(connections)
}
fn active() -> ActiveConnection {
    ActiveConnection {
        service_id: ServiceId::new(CODEX_SUBSCRIPTION_CONNECTION),
        intent_id: "new-login".into(),
    }
}

#[tokio::test]
async fn pending_relogin_masks_older_usable_connection_until_exact_intent_completes() {
    let (service, broker, store, caller) = fixture();
    assert!(
        service
            .check(
                &caller,
                CheckRequest {
                    target: target(),
                    active: None
                }
            )
            .await
            .unwrap()
            .ready
    );
    let waiting = service
        .check(
            &caller,
            CheckRequest {
                target: target(),
                active: Some(active()),
            },
        )
        .await
        .unwrap();
    assert!(!waiting.ready);
    assert_eq!(waiting.active_state, Some(BrokerHandoffState::Pending));
    assert_eq!(waiting.services[0].kind, "nyxid");
    assert_eq!(waiting.services[1].kind, "codex");
    assert_eq!(waiting.services[1].display_name, "My Codex subscription");
    assert!(!serde_json::to_string(&waiting)
        .unwrap()
        .contains("ABCD-1234"));
    store.0.lock().unwrap().pending.as_mut().unwrap().state = BrokerHandoffState::Satisfied;
    assert!(
        service
            .check(
                &caller,
                CheckRequest {
                    target: target(),
                    active: Some(active())
                }
            )
            .await
            .unwrap()
            .ready
    );
    assert_eq!(broker.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_omissions_duplicates_and_foreign_intents_fail_closed() {
    let (service, broker, _, caller) = fixture();
    let mut foreign = active();
    foreign.intent_id = "another-intent".into();
    assert!(matches!(
        service
            .check(
                &caller,
                CheckRequest {
                    target: target(),
                    active: Some(foreign)
                }
            )
            .await,
        Err(ConnectionError::Changed)
    ));
    broker.rows.lock().unwrap().pop();
    assert!(matches!(
        service
            .check(
                &caller,
                CheckRequest {
                    target: target(),
                    active: None
                }
            )
            .await,
        Err(ConnectionError::Unavailable)
    ));
    let row = broker.rows.lock().unwrap()[0].clone();
    broker.rows.lock().unwrap().push(row);
    assert!(matches!(
        service
            .check(
                &caller,
                CheckRequest {
                    target: target(),
                    active: None
                }
            )
            .await,
        Err(ConnectionError::Unavailable)
    ));
}

#[tokio::test]
async fn connection_start_requires_owned_target_and_exact_required_service() {
    let (service, broker, _, caller) = fixture();
    let request = |service_id| StartRequest {
        target: target(),
        service_id: ServiceId::new(service_id),
        intent_id: uuid::Uuid::new_v4().to_string(),
        force: false,
    };
    let foreign = Caller::new(SubjectId::new("other"), Authorization::none());
    assert!(matches!(
        service.start(&foreign, request("api-github")).await,
        Err(ConnectionError::NotFound)
    ));
    assert!(matches!(
        service.start(&caller, request("unexpected-service")).await,
        Err(ConnectionError::Invalid)
    ));
    assert_eq!(broker.starts.load(Ordering::SeqCst), 0);
    let started = service.start(&caller, request("api-github")).await.unwrap();
    assert_eq!(started.owner, "owner");
    assert_eq!(started.service_id.as_str(), "api-github");
    assert_eq!(broker.starts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn full_connection_check_overlaps_both_reads_only_after_target_authorization() {
    let (browser, broker, _, caller) = fixture();
    let gate = Arc::new(ReadGate {
        requirements_started: AtomicUsize::new(0),
        metadata_started: AtomicUsize::new(0),
        requirements_release: tokio::sync::Semaphore::new(0),
        metadata_release: tokio::sync::Semaphore::new(0),
    });
    *broker.read_gate.lock().unwrap() = Some(gate.clone());
    let foreign = Caller::new(SubjectId::new("foreign"), Authorization::none());
    assert!(matches!(
        browser
            .check(
                &foreign,
                CheckRequest {
                    target: target(),
                    active: None
                }
            )
            .await,
        Err(ConnectionError::NotFound)
    ));
    assert_eq!(gate.requirements_started.load(Ordering::SeqCst), 0);
    assert_eq!(gate.metadata_started.load(Ordering::SeqCst), 0);

    let check = browser.check(
        &caller,
        CheckRequest {
            target: ConnectionTarget::Sandbox {
                id: layer2_core::SandboxId("sandbox".into()),
            },
            active: None,
        },
    );
    tokio::pin!(check);
    tokio::select! {
        outcome = &mut check => panic!("gated connection reads completed early: {}", outcome.is_ok()),
        reached = tokio::time::timeout(Duration::from_secs(1), async {
            while gate.requirements_started.load(Ordering::SeqCst) != 1 || gate.metadata_started.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        }) => reached.expect("both reads start before either is released"),
    }
    gate.requirements_release.add_permits(1);
    gate.metadata_release.add_permits(1);
    let report = check.await.unwrap();
    assert!(report.ready);
    assert_eq!(report.services.len(), 2);
    assert_eq!(broker.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_connection_requirements_win_before_active_intent_validation() {
    let (browser, broker, _, caller) = fixture();
    broker.rows.lock().unwrap().clear();
    let result = browser
        .check(
            &caller,
            CheckRequest {
                target: target(),
                active: Some(ActiveConnection {
                    service_id: ServiceId::new(CODEX_SUBSCRIPTION_CONNECTION),
                    intent_id: "unknown-intent".into(),
                }),
            },
        )
        .await;
    assert!(matches!(result, Err(ConnectionError::Unavailable)));
    assert_eq!(broker.starts.load(Ordering::SeqCst), 0);
}

#[path = "browser_ready_tests.rs"]
mod ready;
