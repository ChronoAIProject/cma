use super::*;
use layer2_core::agent_ownership::OwnershipCoordinator;
use layer3_adapter::hosted::{HostedControlTransport, ProviderHostedControlTransport};
use layer3_adapter::{provider::SandboxClient, wire::RequestFrame, RuntimeHandle};
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) async fn runtime() -> (ProviderHostedControlTransport, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{address}"),
        "fixture-credential",
    ))
    .unwrap();
    let task = tokio::spawn(async move {
        for action in [
            "endpoints/9090",
            "manager-ownership/challenge",
            "manager-ownership/revoke",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).await.unwrap(), 1);
                bytes.push(byte[0]);
                if bytes.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8(bytes).unwrap();
            assert!(headers.lines().next().unwrap().contains(action));
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            let (status, body) = if action.starts_with("endpoints") {
                (
                    "200 OK",
                    serde_json::json!({"endpoint":format!("http://{address}")}).to_string(),
                )
            } else if action.ends_with("challenge") {
                ("404 Not Found", "private-response-sentinel".into())
            } else {
                ("403 Forbidden", "private-cleanup-sentinel".into())
            };
            stream.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    });
    (transport, task)
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn failed_handshake_survives_manager_restart_and_failed_cleanup_without_log_delivery() {
    // No tracing subscriber: durability cannot depend on an enabled log event.
    let mut store = fixture().await;
    store.receiver_diagnostics_enabled = true;
    let store = Arc::new(store);
    let manager = OwnershipCoordinator::register(store.clone(), member("diagnostic-member"))
        .await
        .unwrap();
    let authority = manager
        .acquire(&AgentId("diagnostic-agent".into()))
        .await
        .unwrap()
        .unwrap();
    let token = authority.token.clone();
    let before = row(&store, &token).await;
    let (transport, server) = runtime().await;
    let result = authority
        .run(async {
            let _ = transport
                .exchange(
                    &RuntimeHandle::new("fixture-runtime"),
                    RequestFrame::Resources,
                )
                .await;
            std::future::pending::<()>().await
        })
        .await;
    assert_eq!(result, Err(OwnershipError::Stale));
    assert!(authority.receiver_cancelled());
    let recorded = row(&store, &token).await;
    let diagnostic = recorded.get_document("receiver_failure").unwrap().clone();
    assert_eq!(diagnostic.get_str("phase").unwrap(), "challenge");
    assert_eq!(diagnostic.get_str("reason").unwrap(), "missing_route");
    assert_eq!(diagnostic.get_i32("http_status").unwrap(), 404);
    assert!(authority.revoke_transports().await.is_err());
    server.await.unwrap();
    assert_eq!(row(&store, &token).await, recorded);
    let name = store.database.name().to_owned();
    drop(authority);
    drop(manager);
    drop(transport);
    drop(store);
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let restarted = MongoAgentOwnership::open(&client, &name).await.unwrap();
    let mut recovered = row(&restarted, &token).await;
    assert_eq!(
        PredecessorLease::decode(&recovered).unwrap(),
        PredecessorLease::decode(&before).unwrap()
    );
    assert_eq!(
        recovered
            .remove("receiver_failure")
            .unwrap()
            .as_document()
            .unwrap(),
        &diagnostic
    );
    assert_eq!(
        recovered, before,
        "failed cleanup must not release or extend ownership"
    );
    restarted.database.drop().await.unwrap();
}
