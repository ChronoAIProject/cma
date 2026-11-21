use super::*;
use axum::{extract::Request, middleware::Next, response::Response};
use serde_json::Value;
use std::{io::Write, sync::Mutex};
use tokio::sync::Notify;
use tracing::instrument::WithSubscriber;

const DIAGNOSTIC_TARGET: &str = "cma::agent_manager::environment_relay";

#[derive(Clone, Default)]
struct RelayLog(Arc<Mutex<Vec<u8>>>);

impl Write for RelayLog {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl RelayLog {
    fn records(&self) -> Vec<Value> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["target"] == DIAGNOSTIC_TARGET)
            .map(|event| event["fields"].clone())
            .collect()
    }
}

#[derive(Default)]
struct PeerGate {
    waiting: std::sync::atomic::AtomicBool,
    entered: Notify,
    resume: Notify,
}

impl PeerGate {
    async fn before_handler(&self, request: Request, next: Next) -> Response {
        use std::sync::atomic::Ordering::SeqCst;
        if request.headers().contains_key(PROOF) && self.waiting.swap(false, SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        next.run(request).await
    }
}

pub(super) async fn ready_fixture_read(headers: HeaderMap) -> axum::response::Response {
    if headers
        .get("authorization")
        .is_none_or(|value| value != "Bearer synthetic-authorized")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    assert!(layer2_core::agent_ownership::execution_owner().is_none());
    axum::Json(serde_json::json!({"status":"ready","completed_turn":true})).into_response()
}

async fn probe_ready_fixture(instance: &Instance, id: &AgentId) -> reqwest::Response {
    let client = client();
    let base = format!("{}/api/v1/agents/{id}", instance.address);
    for suffix in ["", "/transcript"] {
        let response = client
            .get(format!("{base}{suffix}"))
            .bearer_auth("synthetic-authorized")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["status"], "ready");
        assert_eq!(body["completed_turn"], true);
    }
    client
        .get(format!("{base}/environment"))
        .header("x-request-id", "synthetic-environment-request")
        .bearer_auth("synthetic-authorized")
        .send()
        .await
        .unwrap()
}

async fn fixture_owner_unavailable(response: reqwest::Response) -> serde_json::Value {
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "1");
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "agent_owner_unavailable");
    body
}

#[tokio::test]
async fn environment_read_refusals_keep_the_public_contract_and_recover_after_handoff() {
    let log = RelayLog::default();
    let output = log.clone();
    let tracing = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .without_time()
            .with_writer(move || output.clone())
            .finish(),
    );
    let gate = Arc::new(PeerGate::default());
    let (store, mut instances) = cluster_with_middleware(2, Default::default(), |router| {
        let tracing = tracing.clone();
        let gate = gate.clone();
        router
            .layer(axum::middleware::from_fn(crate::diagnostic_request::scope))
            .layer(axum::middleware::from_fn(move |request, next| {
                let gate = gate.clone();
                let tracing = tracing.clone();
                async move {
                    gate.before_handler(request, next)
                        .with_subscriber(tracing)
                        .await
                }
            }))
    })
    .await;
    let id = AgentId("agt-synthetic-ready".into());
    let (owner, first) = acquire(&store, &instances, &id).await;
    let peer = 1 - owner;
    let before = probe_ready_fixture(&instances[peer], &id).await;
    assert_eq!(before.status(), StatusCode::OK);
    let body: serde_json::Value = before.json().await.unwrap();
    assert_eq!(body["epoch"], first.token.epoch);

    // Hold the signed hop at the receiver and reacquire on a newer epoch.
    gate.waiting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let request = probe_ready_fixture(&instances[peer], &id);
    let rotate = async {
        tokio::time::timeout(Duration::from_secs(2), gate.entered.notified())
            .await
            .expect("the ingress must send to the selected peer");
        instances[owner]
            .coordinator
            .handoff(&first, first.revoke_transports())
            .await
            .unwrap();
        let (replacement_owner, replacement) = acquire(&store, &instances, &id).await;
        assert_eq!(replacement_owner, owner);
        assert!(replacement.token.epoch > first.token.epoch);
        gate.resume.notify_one();
        replacement
    };
    let (late, first) = tokio::join!(request, rotate);
    let late = fixture_owner_unavailable(late).await;
    let after_rotation = probe_ready_fixture(&instances[peer], &id).await;
    assert_eq!(after_rotation.status(), StatusCode::OK);
    assert_eq!(
        after_rotation.json::<Value>().await.unwrap()["epoch"],
        first.token.epoch
    );

    // The peer can resolve the current owner. Close only the owner's listener.
    instances[owner].server.abort();
    let stopped = tokio::time::timeout(Duration::from_secs(2), &mut instances[owner].server)
        .await
        .unwrap()
        .unwrap_err();
    assert!(stopped.is_cancelled());
    let destination = instances[peer].relay.destination(&id).await.unwrap();
    assert_eq!(destination.token, first.token);
    let refused_forward =
        fixture_owner_unavailable(probe_ready_fixture(&instances[peer], &id).await).await;
    assert_eq!(late, refused_forward);

    // Keep the same durable owner token but mark its membership draining.
    let retiring = instances[owner].coordinator.begin_drain().await;
    assert!(retiring
        .iter()
        .any(|authority| authority.token == first.token));
    assert_eq!(store.owner(&id).await.unwrap().unwrap().token, first.token);
    assert!(store
        .membership()
        .await
        .unwrap()
        .members
        .iter()
        .any(|member| { member.instance_id == first.token.instance_id && member.draining }));
    assert!(instances[peer].relay.destination(&id).await.is_err());
    let refused_member =
        fixture_owner_unavailable(probe_ready_fixture(&instances[peer], &id).await).await;
    assert_eq!(refused_member, refused_forward);

    // No runtime receiver was registered in this transport-only fixture.
    instances[owner]
        .coordinator
        .handoff(&first, first.revoke_transports())
        .await
        .unwrap();
    assert!(store.owner(&id).await.unwrap().is_none());
    let refused_owner =
        fixture_owner_unavailable(probe_ready_fixture(&instances[peer], &id).await).await;
    assert_eq!(refused_owner, refused_forward);

    let (next_owner, second) = acquire(&store, &instances, &id).await;
    assert_eq!(next_owner, peer);
    assert!(second.token.epoch > first.token.epoch);
    let recovered = probe_ready_fixture(&instances[peer], &id).await;
    assert_eq!(recovered.status(), StatusCode::OK);
    let body: serde_json::Value = recovered.json().await.unwrap();
    assert_eq!(body["epoch"], second.token.epoch);
    let unauthenticated = client()
        .get(format!(
            "{}/api/v1/agents/{id}/environment",
            instances[peer].address
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let records = log.records();
    let reasons: Vec<_> = records
        .iter()
        .map(|record| record["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        [
            "authority_changed",
            "peer_response",
            "peer_transport",
            "owner_member_unavailable",
            "owner_missing",
        ]
    );
    assert_eq!(records[0]["hop_hash"], records[1]["hop_hash"]);
    assert_eq!(records[1]["hop_hash"].as_str().unwrap().len(), 64);
    assert_ne!(records[1]["hop_hash"], records[2]["hop_hash"]);
    assert_eq!(records[1]["request_hash"].as_str().unwrap().len(), 64);
    let recorded = serde_json::to_string(&records).unwrap();
    for secret in [
        "synthetic-environment-request",
        "synthetic-authorized",
        id.as_str(),
        "instance-",
    ] {
        assert!(!recorded.contains(secret));
    }
}
