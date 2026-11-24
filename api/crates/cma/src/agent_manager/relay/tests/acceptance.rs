//! HTTP relay acceptance with independently observed source and receiver policy.
//! The store and caller are synthetic; these timings measure the local relay only.
use super::*;
use axum::{body::Bytes, extract::State, Extension};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Probe {
    allowed: AtomicBool,
    authorizations: AtomicUsize,
    effects: AtomicUsize,
}

#[async_trait::async_trait]
impl public_access::PublicAuthorization for Probe {
    async fn authorize(
        &self,
        request: public_access::PublicRequest<'_>,
    ) -> Result<
        Option<layer2_core::sandbox::agent_filesystem::FileWriteTarget>,
        public_access::PublicRefusal,
    > {
        self.authorizations.fetch_add(1, Ordering::SeqCst);
        if self.allowed.load(Ordering::SeqCst)
            && request.context.caller.subject.as_str() == "synthetic-caller"
            && request
                .headers
                .get("authorization")
                .is_some_and(|value| value == "Bearer synthetic-authorized")
        {
            Ok(None)
        } else {
            Err(StatusCode::FORBIDDEN.into())
        }
    }
}

async fn accept(
    State(probe): State<Arc<Probe>>,
    peer: Option<Extension<http::VerifiedPeer>>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let owner = layer2_core::agent_ownership::execution_owner()
        .expect("acceptance must run under current ownership");
    probe.effects.fetch_add(1, Ordering::SeqCst);
    (
        StatusCode::ACCEPTED,
        axum::Json(json!({
            "status": "accepted",
            "owner": owner.instance_id,
            "epoch": owner.epoch,
            "relayed": peer.is_some(),
            "uri": uri.to_string(),
            "body": body.to_vec(),
            "request": headers.get("x-request-id").unwrap().to_str().unwrap(),
            "idempotency": headers.get("idempotency-key").unwrap().to_str().unwrap(),
        })),
    )
}

async fn replicas() -> (Arc<MemoryOwnershipStore>, Vec<Instance>, Vec<Arc<Probe>>) {
    let store = Arc::new(MemoryOwnershipStore::default());
    let mut instances = Vec::new();
    let mut probes = Vec::new();
    for index in 0..2 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            store.client(),
            Member {
                instance_id: format!("acceptance-{index}"),
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 16,
            },
        )
        .await
        .unwrap();
        let updates = DurableAgentUpdateRecorder::new(Arc::new(InMemorySandboxStore::default()));
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"acceptance-fixture-key"),
        )
        .unwrap();
        let probe = Arc::new(Probe {
            allowed: AtomicBool::new(true),
            authorizations: AtomicUsize::new(0),
            effects: AtomicUsize::new(0),
        });
        assert!(relay.public_access.set(probe.clone()).is_ok());
        let router = relay
            .routes(
                Router::new()
                    .route("/api/v1/agents/{agent}/files", axum::routing::post(accept))
                    .with_state(probe.clone()),
            )
            .layer(axum::middleware::from_fn(test_auth));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        instances.push(Instance {
            coordinator,
            relay,
            updates,
            address,
            server,
        });
        probes.push(probe);
    }
    (store, instances, probes)
}

/// Canonical, so the request context preserves it across the relay.
const ACCEPTANCE_REQUEST: &str = "00000000-0000-4000-8000-0000000000a1";

fn headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("authorization", "Bearer synthetic-authorized"),
        ("content-type", "application/json"),
        ("x-request-id", ACCEPTANCE_REQUEST),
        ("idempotency-key", "acceptance-operation"),
    ] {
        headers.insert(name, value.parse().unwrap());
    }
    headers
}

#[tokio::test]
async fn non_owner_acceptance_preserves_authorization_body_and_request_identity() {
    let (store, instances, probes) = replicas().await;
    let id = AgentId("agt-acceptance".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let peer = 1 - owner;
    let client = client();
    let path = format!("/api/v1/agents/{id}/files?path=fixture.txt");
    let body = b"{\"content\":\"synthetic\"}";
    let mut direct = Vec::new();
    let mut relayed = Vec::new();
    // Alternate order to avoid assigning all connection warmup to one path.
    // Preserve every measured sample; this is not a production latency gate.
    for round in 0..40 {
        let order = if round % 2 == 0 {
            [peer, owner]
        } else {
            [owner, peer]
        };
        for ingress in order {
            let start = std::time::Instant::now();
            let response = client
                .post(format!("{}{path}", instances[ingress].address))
                .headers(headers())
                .body(body.to_vec())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::ACCEPTED);
            let accepted: Value = response.json().await.unwrap();
            let elapsed_us = start.elapsed().as_micros();
            assert_eq!(accepted["status"], "accepted");
            assert_eq!(accepted["owner"], instances[owner].coordinator.instance_id);
            assert_eq!(accepted["epoch"], authority.token.epoch);
            assert_eq!(accepted["relayed"], ingress == peer);
            assert_eq!(accepted["uri"], path);
            assert_eq!(accepted["body"], json!(body.to_vec()));
            assert_eq!(accepted["request"], ACCEPTANCE_REQUEST);
            assert_eq!(accepted["idempotency"], "acceptance-operation");
            if ingress == peer {
                relayed.push(elapsed_us);
            } else {
                direct.push(elapsed_us);
            }
        }
    }
    assert_eq!(probes[peer].authorizations.load(Ordering::SeqCst), 40);
    assert_eq!(probes[owner].authorizations.load(Ordering::SeqCst), 80);
    assert_eq!(probes[peer].effects.load(Ordering::SeqCst), 0);
    assert_eq!(probes[owner].effects.load(Ordering::SeqCst), 80);
    println!(
        "relay_acceptance_latency {}",
        json!({
            "unit": "microseconds", "samples_per_path": 40,
            "direct": direct, "relayed": relayed, "failed": 0,
            "scope": "loopback_http_synthetic_policy_and_memory_ownership",
        })
    );
}

#[tokio::test]
async fn receiver_denial_and_missing_caller_never_reach_acceptance() {
    let (store, instances, probes) = replicas().await;
    let id = AgentId("agt-acceptance-denied".into());
    let (owner, _) = acquire(&store, &instances, &id).await;
    let peer = 1 - owner;
    let url = format!("{}/api/v1/agents/{id}/files", instances[peer].address);
    let client = client();
    assert_eq!(
        client.post(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(probes[peer].authorizations.load(Ordering::SeqCst), 0);
    assert_eq!(probes[owner].authorizations.load(Ordering::SeqCst), 0);
    probes[owner].allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        client
            .post(url)
            .headers(headers())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(probes[peer].authorizations.load(Ordering::SeqCst), 1);
    assert_eq!(probes[owner].authorizations.load(Ordering::SeqCst), 1);
    assert!(probes
        .iter()
        .all(|probe| probe.effects.load(Ordering::SeqCst) == 0));
}

#[tokio::test]
async fn receiver_rejects_tampered_and_replayed_peer_proofs() {
    let (store, instances, probes) = replicas().await;
    let id = AgentId("agt-acceptance-proof".into());
    let (owner, authority) = acquire(&store, &instances, &id).await;
    let peer = 1 - owner;
    let uri: Uri = format!("/api/v1/agents/{id}/files").parse().unwrap();
    let body = b"{\"content\":\"synthetic\"}";
    let proof = instances[peer]
        .relay
        .authentication
        .sign(
            &instances[peer].coordinator.instance_id,
            &authority.token,
            &Method::POST,
            &uri,
            &headers(),
            body,
        )
        .unwrap();
    let client = client();
    let url = format!("{}{uri}", instances[owner].address);
    for (sent, expected) in [
        (b"changed".as_slice(), StatusCode::FORBIDDEN),
        (body.as_slice(), StatusCode::ACCEPTED),
        (body.as_slice(), StatusCode::FORBIDDEN),
    ] {
        let response = client
            .post(&url)
            .headers(headers())
            .header(PROOF, &proof)
            .body(sent.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        response.bytes().await.unwrap();
    }
    assert_eq!(probes[owner].effects.load(Ordering::SeqCst), 1);
    assert_eq!(probes[peer].effects.load(Ordering::SeqCst), 0);
}
