use super::*;
use tokio::io::AsyncRead;

pub(super) async fn serve_runtime(
    mut stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
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
        let ownership_request = request
            .lines()
            .next()
            .unwrap()
            .contains("/manager-ownership/");
        while if ownership_request {
            state.hold_ownership_response.load(Ordering::SeqCst)
        } else {
            state.hold_response.load(Ordering::SeqCst)
        } {
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
            fault
                if fault >= 1000
                    && ownership_failures::respond(&mut stream, &request, &state, fault).await =>
            {
                return;
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
            if request.starts_with("GET /runtime/desktop ") {
                assert!(request
                    .to_ascii_lowercase()
                    .contains("x-cma-desktop-owner:"));
            } else {
                assert!(
                    request.starts_with("GET /runtime/agents/agent-a/conversation?outputAck=1 ")
                );
            }
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
            let mut content_type = "application/json";
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
                    let response = persistent_response(&request);
                    let policy = *state.startup_policy.lock().unwrap();
                    let policy_accept = format!(
                        "accept: {}",
                        crate::RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE
                    );
                    if let Some(enabled) = policy.filter(|_| {
                        request
                            .lines()
                            .any(|line| line.eq_ignore_ascii_case(&policy_accept))
                    }) {
                        let ResponseFrame::RuntimeStatus(runtime) =
                            serde_json::from_str(&response).unwrap()
                        else {
                            panic!("expected runtime status")
                        };
                        content_type = crate::RUNTIME_STARTUP_POLICY_STATUS_MEDIA_TYPE;
                        serde_json::to_string(&crate::RuntimeStartupPolicyObservation {
                            runtime,
                            legacy_journal_recovery_enabled: enabled,
                        })
                        .unwrap()
                    } else {
                        response
                    }
                } else {
                    serde_json::to_string(&ResponseFrame::Resources(ResourceReading::default()))
                        .unwrap()
                }
            } else if request.starts_with("POST /runtime/agents/agent/capacity-recovery ") {
                let request =
                    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
                serde_json::to_string(
                    &crate::agent_capacity_recovery::CapacityRecoveryNativeProof {
                        confirmation: crate::agent_capacity_recovery::tests::confirmation(&request),
                        cwd: "/workspace".into(),
                    },
                )
                .unwrap()
            } else if ownership_request {
                use crate::owner_protocol::*;
                let body = request.split_once("\r\n\r\n").unwrap().1;
                if request.lines().next().unwrap().contains("/challenge ") {
                    let _: TransportOwner = serde_json::from_str(body).unwrap();
                    serde_json::to_string(&OwnerChallenge {
                        protocol: OWNER_PROTOCOL_VERSION,
                        nonce: uuid::Uuid::new_v4().to_string(),
                        valid_for_ms: RECEIVER_LEASE_MS,
                    })
                    .unwrap()
                } else {
                    let owner = if request.lines().next().unwrap().contains("/confirm ") {
                        state.ownership_confirmations.fetch_add(1, Ordering::SeqCst);
                        serde_json::from_str::<OwnerConfirmation>(body)
                            .unwrap()
                            .owner
                    } else {
                        assert!(request.lines().next().unwrap().contains("/revoke "));
                        serde_json::from_str(body).unwrap()
                    };
                    serde_json::to_string(&OwnerAcknowledgement {
                        protocol: OWNER_PROTOCOL_VERSION,
                        owner,
                    })
                    .unwrap()
                }
            } else if request.starts_with("POST /runtime/desktop/ownership/") {
                use crate::desktop_ownership::wire::*;
                let body = request.split_once("\r\n\r\n").unwrap().1;
                if request.starts_with("POST /runtime/desktop/ownership/challenge ") {
                    let _: DesktopChallengeRequest = serde_json::from_str(body).unwrap();
                    serde_json::to_string(&crate::DesktopOwnershipChallenge {
                        nonce: uuid::Uuid::new_v4().to_string(),
                        valid_for_ms: 10000,
                    })
                    .unwrap()
                } else {
                    let ownership =
                        if request.starts_with("POST /runtime/desktop/ownership/confirm ") {
                            serde_json::from_str::<DesktopConfirmation>(body)
                                .unwrap()
                                .ownership
                        } else {
                            assert!(request.starts_with("POST /runtime/desktop/ownership/revoke "));
                            serde_json::from_str(body).unwrap()
                        };
                    serde_json::to_string(&DesktopAcknowledgement { ownership }).unwrap()
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
            if stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n{body}", body.len()).as_bytes()).await.is_err() { return; }
            if matches!(mode, Mode::Persistent) {
                continue;
            }
        }
        return;
    }
}

async fn read_request(stream: &mut (impl AsyncRead + Unpin)) -> Option<String> {
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        request.extend_from_slice(&buffer[..count]);
        assert!(request.len() < 64 * 1024);
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..end]);
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .map(|s| s.parse::<usize>().unwrap())
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                return String::from_utf8(request).ok();
            }
        }
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
        RequestFrame::CloseAgent(request) => ResponseFrame::CloseAgent(FencedReceipt::new(
            request.authority_fence,
            crate::wire::AgentCloseReceipt {
                operation_id: request.request.operation_id,
                request_sha256: request.request.request_sha256,
                agent_id: request.request.agent_id,
            },
        )),
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

#[path = "provider_ownership_native_tests.rs"]
mod ownership_failures;
