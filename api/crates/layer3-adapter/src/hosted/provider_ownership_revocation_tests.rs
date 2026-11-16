//! A failed/ambiguous receiver revoke is never an acknowledgement of custody release.
use super::*;
use crate::owner_protocol::ReceiverRevocation;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn fixture(address: SocketAddr) -> (Arc<Mutex<TransportState>>, ReceiverFence) {
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "synthetic-test-key",
    ))
    .unwrap();
    let original_state = transport.state.clone();
    let mut template = transport;
    // Match production's cycle-free custody: the fence template owns a different
    // state, while the original live link is referenced only weakly.
    template.state = Arc::new(Mutex::new(TransportState::default()));
    let fence = ReceiverFence {
        identity: "fixture-exact-runtime".into(),
        template,
        state: Arc::downgrade(&original_state),
        endpoint: RuntimeEndpoint::parse(
            "http://unused.invalid",
            &format!("http://{address}/proxy"),
        )
        .unwrap(),
        owner: TransportOwner {
            protocol: OWNER_PROTOCOL_VERSION,
            agent_id: "fixture-agent".into(),
            instance_id: "fixture-member".into(),
            epoch: 7,
        },
    };
    (original_state, fence)
}

async fn listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}

async fn request(stream: &mut TcpStream) -> (String, serde_json::Value) {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut part = [0; 1_024];
        let count = stream.read(&mut part).await.unwrap();
        assert_ne!(count, 0, "client closed before headers");
        bytes.extend_from_slice(&part[..count]);
        assert!(bytes.len() < 16_384, "fixture request exceeded bound");
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .expect("JSON POST has content-length");
    assert!(length <= RESPONSE_LIMIT);
    while bytes.len() < header_end + length {
        let mut part = [0; 1_024];
        let count = stream.read(&mut part).await.unwrap();
        assert_ne!(count, 0, "client closed before body");
        bytes.extend_from_slice(&part[..count]);
    }
    (
        headers,
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
    )
}

fn response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len(),
    ).into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

fn rejected_kind(result: crate::Result<()>, kind: &str) {
    match result {
        Err(AdapterError::Rejected(message)) => {
            assert_eq!(
                message,
                format!("runtime receiver revocation failed: {kind}")
            );
            assert!(!message.contains("synthetic-test-key"));
            assert!(!message.contains("fixture-agent"));
        }
        other => panic!("expected typed revocation rejection {kind}, got {other:?}"),
    }
}

#[tokio::test]
async fn dropped_original_link_is_not_a_confirmed_receiver_revocation() {
    let listener = listener().await;
    let (state, fence) = fixture(listener.local_addr().unwrap());
    assert_eq!(
        Arc::strong_count(&state),
        1,
        "fence must not keep live renewal state alive"
    );
    drop(state);
    assert!(fence.state.upgrade().is_none());
    rejected_kind(fence.revoke().await, "local_state_lost");
    assert!(
        tokio::time::timeout(Duration::from_millis(80), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn broken_transport_is_rejected_without_claiming_a_remote_ack() {
    let listener = listener().await;
    let (_state, fence) = fixture(listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        // The receiver might have applied revoke, but its response is lost.
        stream.shutdown().await.unwrap();
    });
    rejected_kind(fence.revoke().await, "transport");
    server.await.unwrap();
}

#[tokio::test]
async fn only_exact_success_ack_confirms_revocation_and_errors_remain_retryable() {
    for kind in [
        "http_status(503)",
        "invalid_response",
        "protocol_mismatch",
        "owner_mismatch",
    ] {
        let listener = listener().await;
        let (state, fence) = fixture(listener.local_addr().unwrap());
        let expected = fence.owner.clone();
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (headers, body) = request(&mut stream).await;
                assert!(headers.starts_with(
                    "POST /proxy/runtime/agents/fixture-agent/manager-ownership/revoke HTTP/1.1"
                ));
                assert!(headers
                    .to_ascii_lowercase()
                    .contains("open-sandbox-api-key: synthetic-test-key"));
                assert_eq!(body, serde_json::to_value(&expected).unwrap());
                let mut owner = expected.clone();
                let mut protocol = OWNER_PROTOCOL_VERSION;
                if attempt == 0 && kind == "owner_mismatch" {
                    owner.epoch += 1;
                }
                if attempt == 0 && kind == "protocol_mismatch" {
                    protocol += 1;
                }
                let mut body =
                    serde_json::to_vec(&OwnerAcknowledgement { protocol, owner }).unwrap();
                let status = if attempt == 0 && kind == "http_status(503)" {
                    // Even a success-shaped body cannot override an HTTP refusal.
                    "503 Service Unavailable"
                } else {
                    "200 OK"
                };
                if attempt == 0 && kind == "invalid_response" {
                    body = b"{not-json".to_vec();
                }
                stream.write_all(&response(status, &body)).await.unwrap();
            }
            listener
        });
        rejected_kind(fence.revoke().await, kind);
        assert!(
            fence.state.upgrade().is_some(),
            "rejection cannot discard retained custody"
        );
        // A later cleanup attempt still addresses exactly the same captured token.
        fence.revoke().await.unwrap();
        let listener = server.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(80), listener.accept())
                .await
                .is_err()
        );
        assert_eq!(Arc::strong_count(&state), 1);
    }
}

#[tokio::test]
async fn a_higher_epoch_ack_is_rejected_and_captured_owner_is_unchanged() {
    let listener = listener().await;
    let (_state, fence) = fixture(listener.local_addr().unwrap());
    let original = fence.owner.clone();
    let mut successor = original.clone();
    successor.epoch += 1;
    successor.instance_id = "successor-member".into();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut stream).await;
        assert_eq!(body["epoch"], 7);
        let body = serde_json::to_vec(&OwnerAcknowledgement {
            protocol: OWNER_PROTOCOL_VERSION,
            owner: successor,
        })
        .unwrap();
        stream.write_all(&response("200 OK", &body)).await.unwrap();
    });
    rejected_kind(fence.revoke().await, "owner_mismatch");
    assert_eq!(fence.owner, original);
    server.await.unwrap();
}

#[tokio::test]
async fn silent_receiver_hits_bounded_timeout_without_a_success_ack() {
    let listener = listener().await;
    let (_state, fence) = fixture(listener.local_addr().unwrap());
    let (received, receipt) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        received.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let started = tokio::time::Instant::now();
    let result = tokio::time::timeout(
        NEGOTIATION_DEADLINE + Duration::from_secs(2),
        fence.revoke(),
    )
    .await;
    let elapsed = started.elapsed();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    receipt.await.unwrap();
    rejected_kind(
        result.expect("production timeout must be bounded"),
        "timeout",
    );
    assert!(elapsed >= NEGOTIATION_DEADLINE);
}
