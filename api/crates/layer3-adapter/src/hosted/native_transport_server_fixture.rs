use super::*;

pub(super) async fn serve_runtime(
    mut stream: tokio::io::DuplexStream,
    mode: Mode,
    state: Arc<State>,
    token: String,
) {
    loop {
        let Some(request) = read_request(&mut stream).await else {
            return;
        };
        assert!(request
            .to_ascii_lowercase()
            .contains(&format!("x-cma-runtime-token: {token}")));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
        assert!(!request.contains(API_TOKEN));
        state.requests.lock().unwrap().push(request.clone());
        while state.hold_response.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        match state.fault.swap(0, Ordering::SeqCst) {
            1 => return,
            2 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{}")
                    .await;
                return;
            }
            3 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 100\r\n\r\nprivate")
                    .await;
                return;
            }
            4 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad")
                    .await;
                return;
            }
            5 => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n")
                    .await;
                std::future::pending::<()>().await;
            }
            6 => {
                let body =
                    serde_json::to_string(&ResponseFrame::Failure(crate::wire::WireFailure {
                        operation: crate::wire::WireOperation::Install,
                        code: "mismatched".into(),
                        detail: "wrong operation".into(),
                        retryable: false,
                    }))
                    .unwrap();
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                continue;
            }
            7 => {
                let mut response: ResponseFrame =
                    serde_json::from_str(&persistent_response(&request)).unwrap();
                let ResponseFrame::OpenAgent(ref mut receipt) = response else {
                    panic!("invalid Open receipt fixture requires an actual Open");
                };
                receipt.authority_fence += 1;
                let body = serde_json::to_string(&response).unwrap();
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await;
                continue;
            }
            _ => {}
        }
        match mode {
            Mode::LostResponse => return,
            Mode::Stall => {
                std::future::pending::<()>().await;
                return;
            }
            Mode::HttpError => {
                let body = format!("{ERROR_SECRET} {token}");
                let _ = stream
                    .write_all(
                        format!(
                        "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                        .as_bytes(),
                    )
                    .await;
                return;
            }
            Mode::LargeBody => {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n40\r\n")
                    .await;
                let _ = stream.write_all(&[b'x'; 64]).await;
                let _ = stream.write_all(b"\r\n").await;
                std::future::pending::<()>().await;
            }
            _ => {}
        }
        if request.to_ascii_lowercase().contains("upgrade: websocket") {
            assert!(request.starts_with("GET /runtime/agents/agent-a/conversation?outputAck=1 "));
            let key = request
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-key"))
                        .map(|(_, v)| v.trim())
                })
                .unwrap();
            let accept =
                tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
            stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes()).await.unwrap();
            let mut socket = tokio_tungstenite::WebSocketStream::from_raw_socket(
                stream,
                tokio_tungstenite::tungstenite::protocol::Role::Server,
                None,
            )
            .await;
            while let Some(Ok(message)) = socket.next().await {
                if socket.send(message).await.is_err() {
                    return;
                }
            }
        } else {
            let body = if request.starts_with("POST /runtime/control ") {
                if matches!(mode, Mode::WireError) {
                    serde_json::to_string(&ResponseFrame::Failure(crate::wire::WireFailure {
                        operation: crate::wire::WireOperation::Resources,
                        code: "runtime_failure".into(),
                        detail: format!("echoed credential {token}"),
                        retryable: false,
                    }))
                    .unwrap()
                } else if matches!(mode, Mode::Persistent) {
                    persistent_response(&request)
                } else {
                    serde_json::to_string(&ResponseFrame::Resources(ResourceReading::default()))
                        .unwrap()
                }
            } else {
                assert!(request.starts_with("GET /runtime/agents/agent-a/history "));
                "{\"history\":[]}".into()
            };
            let connection = if matches!(mode, Mode::Persistent) {
                "keep-alive"
            } else {
                "close"
            };
            if stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}", body.len()).as_bytes()).await.is_err() { return; }
            if matches!(mode, Mode::Persistent) {
                continue;
            }
        }
        return;
    }
}

fn persistent_response(request: &str) -> String {
    let (_, body) = request.split_once("\r\n\r\n").unwrap();
    let response = match serde_json::from_str::<RequestFrame>(body).unwrap() {
        RequestFrame::OpenAgent(request) => {
            let launch = request.request;
            ResponseFrame::OpenAgent(FencedReceipt::new(request.authority_fence, AgentOpenReceipt {
                operation_id: launch.operation_id,
                request_sha256: launch.request_sha256,
                agent_id: launch.agent_id,
                cwd: launch.cwd,
                home: launch.home,
                conversation_id: None,
            }))
        },
        RequestFrame::Install(request) => ResponseFrame::Install(crate::InstallReceipt {
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
            commands: vec![],
        }),
        RequestFrame::RuntimeStatus(request) => ResponseFrame::RuntimeStatus(serde_json::from_value(json!({
            "sandbox_id":request.sandbox_id,"generation":request.generation,"home_identity":request.home_identity,
            "boot_id":"a".repeat(32),"installation_ready":true,"credential_ready":false,"restore_pending":false,"agent_processes":0
        })).unwrap()),
        RequestFrame::Resources => ResponseFrame::Resources(ResourceReading::default()),
        _ => panic!("unexpected persistent fixture operation"),
    };
    serde_json::to_string(&response).unwrap()
}

