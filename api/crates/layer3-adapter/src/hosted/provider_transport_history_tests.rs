use super::*;

const HISTORY_LIMIT: usize = 64 * 1024 * 1024;

async fn transport_for_runtime(
    runtime: std::net::SocketAddr,
) -> (
    ProviderHostedControlTransport,
    tokio::task::JoinHandle<Vec<u8>>,
) {
    let provider = listener().await;
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{}", provider.local_addr().unwrap()),
        "fixture-history-key",
    ))
    .unwrap();
    let task = spawn_http(
        provider,
        json_response(&serde_json::json!({
            "endpoint": format!("{runtime}/proxy/3001"),
        })),
    );
    (transport, task)
}

#[tokio::test]
async fn history_download_preserves_jsonl_and_authenticates_without_archive_storage() {
    let bytes = b"{\"type\":\"session_meta\"}\n{\"type\":\"event_msg\"}\n";
    let runtime = listener().await;
    let (transport, provider) = transport_for_runtime(runtime.local_addr().unwrap()).await;
    let served = spawn_http(
        runtime,
        http_response(
            "200 OK",
            &[
                ("Content-Type", "application/x-ndjson".into()),
                ("Content-Length", bytes.len().to_string()),
            ],
            bytes,
        ),
    );
    let handle = RuntimeHandle::new("runtime-one");
    transport
        .bind_runtime(&handle, "sandbox-one")
        .await
        .unwrap();
    assert_eq!(
        transport
            .export_agent_history(&handle, &AgentSlot::new("agt_history"))
            .await
            .unwrap(),
        bytes
    );
    assert!(matches!(
        transport.archive(&handle).await,
        Err(AdapterError::Unsupported(_))
    ));
    provider.await.unwrap();
    let request = request_text(served.await.unwrap());
    assert!(request.starts_with("GET /proxy/3001/runtime/agents/agt_history/history HTTP/1.1"));
    assert!(request
        .to_ascii_lowercase()
        .contains("open-sandbox-api-key: fixture-history-key"));
}

#[tokio::test]
async fn history_rejects_invalid_agent_paths_before_provider_io() {
    let provider = listener().await;
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{}", provider.local_addr().unwrap()),
        "fixture",
    ))
    .unwrap();
    for value in [
        "",
        ".",
        "..",
        "../another",
        "a/b",
        "a?b",
        "a\nb",
        &"a".repeat(65),
    ] {
        assert!(matches!(
            transport
                .export_agent_history(&RuntimeHandle::new("runtime"), &AgentSlot::new(value))
                .await,
            Err(AdapterError::Rejected(_))
        ));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), provider.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn history_refuses_redirects_and_oversized_declared_bodies_without_reading_them() {
    let forbidden = listener().await;
    for response in [
        http_response(
            "302 Found",
            &[
                (
                    "Location",
                    format!("http://{}/stolen", forbidden.local_addr().unwrap()),
                ),
                ("Content-Length", "0".into()),
            ],
            b"",
        ),
        http_response(
            "200 OK",
            &[("Content-Length", (HISTORY_LIMIT + 1).to_string())],
            b"",
        ),
    ] {
        let runtime = listener().await;
        let (transport, provider) = transport_for_runtime(runtime.local_addr().unwrap()).await;
        let served = spawn_http(runtime, response);
        let result = transport
            .export_agent_history(
                &RuntimeHandle::new("runtime"),
                &AgentSlot::new("agt_history"),
            )
            .await;
        assert!(matches!(
            result,
            Err(AdapterError::Unavailable(_) | AdapterError::Rejected(_))
        ));
        provider.await.unwrap();
        served.await.unwrap();
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), forbidden.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn history_stops_an_oversized_chunked_body_before_waiting_for_eof() {
    let runtime = listener().await;
    let (transport, provider) = transport_for_runtime(runtime.local_addr().unwrap()).await;
    let served = tokio::spawn(async move {
        let (mut socket, _) = runtime.accept().await.unwrap();
        read_request(&mut socket).await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        let chunk = vec![b'x'; 1024 * 1024];
        for _ in 0..=HISTORY_LIMIT / chunk.len() {
            if socket.write_all(b"100000\r\n").await.is_err()
                || socket.write_all(&chunk).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        // No terminating chunk: the size bound must end the read without EOF.
        std::future::pending::<()>().await;
    });
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        transport.export_agent_history(
            &RuntimeHandle::new("runtime"),
            &AgentSlot::new("agt_history"),
        ),
    )
    .await;
    served.abort();
    provider.await.unwrap();
    assert!(
        matches!(result.unwrap(), Err(AdapterError::Rejected(detail)) if detail.contains("size limit"))
    );
}
