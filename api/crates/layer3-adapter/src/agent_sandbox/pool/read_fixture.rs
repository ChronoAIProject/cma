//! One-shot response gates prove concurrency without wall-time comparisons.
use super::fixture::Fixture;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;

pub(super) struct Gate {
    started: Semaphore,
    release: Semaphore,
}
impl Gate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
    pub async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.started.acquire())
            .await
            .expect("independent read did not start")
            .unwrap()
            .forget();
    }
    pub fn respond(&self) {
        self.release.add_permits(1);
    }
    async fn block(&self) {
        self.started.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}

/// The same resource model with gated reads and observable real fixture writes.
pub(super) struct Effects {
    gates: Arc<Mutex<BTreeMap<String, Arc<Gate>>>>,
    pub client: kube::Client,
    pub calls: Arc<Mutex<Vec<String>>>,
}
impl Effects {
    pub fn new(fixture: &Fixture, gates: BTreeMap<String, Arc<Gate>>) -> Self {
        Self::with_delays(fixture, gates, BTreeMap::new())
    }
    pub fn with_delays(
        fixture: &Fixture,
        gates: BTreeMap<String, Arc<Gate>>,
        delays: BTreeMap<String, Duration>,
    ) -> Self {
        use http_body_util::BodyExt;
        let state = fixture.state.clone();
        let kubernetes = fixture.kubernetes.clone();
        let gates = Arc::new(Mutex::new(gates));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let pending_gates = gates.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            let method = request.method().to_string();
            let path = request.uri().path_and_query().unwrap().to_string();
            recorded
                .lock()
                .unwrap()
                .push(format!("{method} {}", request.uri().path()));
            let delay = if method == "GET" {
                delays.get(request.uri().path()).copied()
            } else {
                None
            };
            let gate = if method == "GET" {
                pending_gates.lock().unwrap().remove(request.uri().path())
            } else {
                None
            };
            let state = state.clone();
            let kubernetes = kubernetes.clone();
            async move {
                if let Some(gate) = gate {
                    gate.block().await;
                }
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
                // Keep bootstrap's real upgrade handshake on the TCP fixture;
                // ordinary resource reads remain independently gateable here.
                if request.uri().path().ends_with("/exec") {
                    return kubernetes.send(request).await;
                }
                let bytes = request.into_body().collect().await.unwrap().to_bytes();
                let body = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap()
                };
                let (status, body, lost) = super::fixture::respond(&state, &method, &path, body);
                assert!(!lost, "lost responses use the TCP fixture");
                Ok::<_, kube::Error>(
                    http::Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        Self {
            gates,
            client: kube::Client::new(service, "cma-test"),
            calls,
        }
    }
    pub fn arm_read_gates(&self, gates: BTreeMap<String, Arc<Gate>>) {
        self.gates.lock().unwrap().extend(gates);
    }
    pub fn pools(&self, fixture: &Fixture) -> super::AgentSandboxPools {
        super::AgentSandboxPools::new(self.client.clone(), fixture.client.config.clone())
    }
    pub fn assert_read_only(&self) {
        assert!(self
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.starts_with("GET ")));
    }
}

pub(super) struct Reads {
    pub client: kube::Client,
    pub calls: Arc<Mutex<Vec<String>>>,
}
impl Reads {
    pub fn new(
        fixture: &Fixture,
        overrides: BTreeMap<String, (u16, Value)>,
        gates: BTreeMap<String, Arc<Gate>>,
    ) -> Self {
        let mut replies: BTreeMap<_, _> = fixture
            .state
            .lock()
            .unwrap()
            .rows
            .iter()
            .map(|(path, row)| (path.clone(), (200, row.clone())))
            .collect();
        replies.extend(overrides);
        let gates = Arc::new(Mutex::new(gates));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let service = tower::service_fn(move |request: http::Request<kube::client::Body>| {
            assert_eq!(request.method(), http::Method::GET, "proof must not write");
            let path = request.uri().path().to_owned();
            recorded.lock().unwrap().push(path.clone());
            let (status, body) = replies.get(&path).expect("exact resource name").clone();
            let gate = gates.lock().unwrap().remove(&path);
            async move {
                if let Some(gate) = gate {
                    gate.started.add_permits(1);
                    gate.release.acquire().await.unwrap().forget();
                }
                Ok::<_, std::convert::Infallible>(
                    http::Response::builder()
                        .status(status)
                        .header(http::header::CONTENT_TYPE, "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        Self {
            client: kube::Client::new(service, "cma-test"),
            calls,
        }
    }
    pub fn pools(&self, fixture: &Fixture) -> super::AgentSandboxPools {
        super::AgentSandboxPools::new(self.client.clone(), fixture.client.config.clone())
    }
}

pub(super) fn failure(code: u16) -> (u16, Value) {
    (
        code,
        json!({"apiVersion":"v1","kind":"Status","status":"Failure",
        "reason":"Fixture","message":"fixture failure","code":code}),
    )
}
