use super::super::tests::{json_response, listener, read_request, request_text};
use super::*;
use crate::SandboxAdapter;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;

struct OwnedHandshake(String);

impl tokio_tungstenite::tungstenite::handshake::server::Callback for OwnedHandshake {
    fn on_request(
        self,
        request: &tokio_tungstenite::tungstenite::handshake::server::Request,
        response: tokio_tungstenite::tungstenite::handshake::server::Response,
    ) -> Result<
        tokio_tungstenite::tungstenite::handshake::server::Response,
        tokio_tungstenite::tungstenite::handshake::server::ErrorResponse,
    > {
        assert_eq!(request.uri().path(), "/captured/runtime/desktop");
        assert_eq!(request.headers()[DESKTOP_OWNER_HEADER], self.0);
        Ok(response)
    }
}

fn owner() -> DesktopOwnership {
    DesktopOwnership {
        sandbox_id: "logical-sandbox".into(),
        runtime_id: "runtime-handle".into(),
        session_id: "desktop-session".into(),
        boot_id: None,
        epoch: 1,
        issued_at_ms: 1000,
        expires_at_ms: 16000,
    }
}

struct Routes;
#[async_trait::async_trait]
impl crate::hosted::RuntimeRoutingStore for Routes {
    async fn select(&self, _: &str, _: u64, _: bool) -> crate::Result<bool> {
        Ok(false)
    }
}

#[tokio::test]
async fn routed_concrete_adapter_pins_endpoint_and_controls_independent_owned_socket() {
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    let expected = owner();
    let server = tokio::spawn(async move {
        for action in ["endpoint", "challenge", "confirm"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_text(read_request(&mut stream).await);
            let response = match action {
                "endpoint" => {
                    assert!(request.starts_with("GET /v1/sandboxes/runtime-handle/endpoints/9090"));
                    serde_json::json!({"endpoint":format!("http://{address}/captured")})
                }
                "challenge" => {
                    assert!(
                        request.starts_with("POST /captured/runtime/desktop/ownership/challenge")
                    );
                    assert!(
                        request.contains("logical-sandbox") && request.contains("runtime-handle")
                    );
                    serde_json::json!({"nonce":"initial-nonce","valid_for_ms":10000})
                }
                _ => {
                    assert!(request.starts_with("POST /captured/runtime/desktop/ownership/confirm"));
                    serde_json::to_value(DesktopAcknowledgement {
                        ownership: expected.clone(),
                    })
                    .unwrap()
                }
            };
            stream.write_all(&json_response(&response)).await.unwrap();
        }
        let (stream, _) = listener.accept().await.unwrap();
        let expected_header = serde_json::to_string(&expected).unwrap();
        let mut socket =
            tokio_tungstenite::accept_hdr_async(stream, OwnedHandshake(expected_header))
                .await
                .unwrap();
        let echo = tokio::spawn(async move {
            let frame = socket.next().await.unwrap().unwrap();
            socket.send(frame).await.unwrap();
            let _ = socket.next().await;
        });
        for action in ["challenge", "confirm", "revoke"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let raw = read_request(&mut stream).await;
            let request = request_text(raw);
            // A second provider discovery or another endpoint is forbidden.
            assert!(request.starts_with(&format!(
                "POST /captured/runtime/desktop/ownership/{action}"
            )));
            let body = request.split_once("\r\n\r\n").unwrap().1;
            let response = match action {
                "challenge" => serde_json::json!({"nonce":"renew-nonce","valid_for_ms":10000}),
                "confirm" => serde_json::to_value(DesktopAcknowledgement {
                    ownership: serde_json::from_str::<DesktopConfirmation>(body)
                        .unwrap()
                        .ownership,
                })
                .unwrap(),
                _ => serde_json::to_value(DesktopAcknowledgement {
                    ownership: serde_json::from_str(body).unwrap(),
                })
                .unwrap(),
            };
            stream.write_all(&json_response(&response)).await.unwrap();
        }
        echo.abort();
        let _ = echo.await;
    });
    let client = SandboxClient::new(
        format!("http://{address}"),
        uuid::Uuid::new_v4().to_string(),
    );
    let transport =
        Arc::new(ProviderHostedControlTransport::for_retained_opensandbox(client.clone()).unwrap());
    let handle = RuntimeHandle::new("runtime-handle");
    transport
        .bind_sandbox(&handle, "logical-sandbox")
        .await
        .unwrap();
    let backend =
        crate::hosted::ProviderHostedBackend::new(client, transport, "control-plane").unwrap();
    let routes = crate::hosted::RuntimeBackends {
        agent_sandbox: None,
        opensandbox: Some(Arc::new(backend)),
        allocate_agent_sandbox: false,
        routing: Arc::new(Routes),
        home_routes: None,
    };
    let adapter = crate::codex::CodexAdapter::with_backend(routes, Default::default());
    let challenge = adapter.desktop_ownership_challenge(&handle).await.unwrap();
    let mut connection = adapter
        .open_desktop_owned(&handle, &owner(), &challenge)
        .await
        .unwrap();
    connection
        .send(crate::DesktopFrame::Binary(vec![1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(
        connection.recv().await.unwrap(),
        Some(crate::DesktopFrame::Binary(vec![1, 2, 3]))
    );
    let control = connection.ownership_control().unwrap();
    let challenge = control.ownership_challenge().await.unwrap();
    let mut renewed = owner();
    renewed.issued_at_ms += 1000;
    renewed.expires_at_ms += 1000;
    control.renew_ownership(&renewed, &challenge).await.unwrap();
    control.revoke_ownership(&renewed).await.unwrap();
    assert!(control.ownership_challenge().await.is_err());
    server.await.unwrap();
}

async fn captured(transport: &ProviderHostedControlTransport, endpoint: &str) -> Captured {
    let handle = RuntimeHandle::new("runtime-handle");
    transport
        .bind_sandbox(&handle, "logical-sandbox")
        .await
        .unwrap();
    let (identity, _) = transport.native_endpoint_state(&handle).await;
    Captured {
        handle,
        identity,
        endpoint: RuntimeEndpoint::parse("http://fixture.invalid", endpoint).unwrap(),
        pod: None,
    }
}

#[tokio::test]
async fn failed_desktop_requires_an_exact_terminal_handshake_refusal() {
    for (status, headers, terminal) in [
        (503, vec![DESKTOP_STATE_FAILED], true),
        (503, vec![], false),
        (503, vec!["restarting"], false),
        (503, vec![DESKTOP_STATE_FAILED, DESKTOP_STATE_FAILED], false),
        (404, vec![DESKTOP_STATE_FAILED], false),
    ] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new("http://unused.invalid", uuid::Uuid::new_v4().to_string()),
        )
        .unwrap();
        let capture = captured(&transport, &format!("http://{address}")).await;
        let authority = owner();
        let control = Arc::new(control::Control::new(
            transport.clone(),
            capture.clone(),
            authority.clone(),
            Instant::now() + Duration::from_secs(3),
        ));
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_text(read_request(&mut stream).await);
            assert!(request.starts_with("GET /runtime/desktop HTTP/1.1"));
            let mut response = format!("HTTP/1.1 {status} Refused\r\nContent-Length: 0\r\n");
            for state in headers {
                response.push_str(&format!("{DESKTOP_STATE_HEADER}: {state}\r\n"));
            }
            response.push_str("\r\n");
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let result = capture.connect(&transport, &authority, control).await;
        let Err(error) = result else {
            panic!("fixture refused the upgrade")
        };
        assert_eq!(
            error,
            if terminal {
                AdapterError::DesktopFailed
            } else {
                refused()
            }
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn changed_assignment_or_claim_refuses_before_confirm_and_lost_ack_is_not_replayed() {
    for fault in ["assignment", "runtime", "sandbox", "lost-ack"] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new("http://unused.invalid", uuid::Uuid::new_v4().to_string()),
        )
        .unwrap();
        let capture = captured(&transport, &format!("http://{address}")).await;
        let challenge = DesktopOwnershipChallenge {
            nonce: "single-use".into(),
            valid_for_ms: 10000,
        };
        transport.state.lock().await.desktop_challenges.insert(
            challenge.nonce.clone(),
            Pending {
                captured: capture.clone(),
                challenge: challenge.clone(),
                deadline: Instant::now() + Duration::from_secs(10),
            },
        );
        let mut ownership = owner();
        match fault {
            "assignment" => {
                transport
                    .bind_native_identity(&capture.handle, "logical-sandbox", &"a".repeat(64))
                    .await
                    .unwrap();
            }
            "runtime" => ownership.runtime_id = "replacement".into(),
            "sandbox" => ownership.sandbox_id = "other".into(),
            _ => {}
        }
        let server = tokio::spawn(async move {
            if fault == "lost-ack" {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = request_text(read_request(&mut stream).await);
                assert!(request.starts_with("POST /runtime/desktop/ownership/confirm"));
                drop(stream);
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        assert!(
            DesktopOwnedRuntime::open(&transport, &capture.handle, &ownership, &challenge)
                .await
                .is_err()
        );
        assert!(
            DesktopOwnedRuntime::open(&transport, &capture.handle, &ownership, &challenge)
                .await
                .is_err()
        );
        server.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn expired_control_does_not_poll_work_and_wrong_duration_is_unsupported() {
    for valid_for_ms in [0, 10001, 20000] {
        assert!(deadline(
            Instant::now(),
            &DesktopOwnershipChallenge {
                nonce: "nonce".into(),
                valid_for_ms
            }
        )
        .is_err());
    }
    let work = std::sync::atomic::AtomicUsize::new(0);
    assert!(bounded(Instant::now(), async {
        work.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
    .await
    .is_err());
    assert_eq!(work.load(std::sync::atomic::Ordering::SeqCst), 0);
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        uuid::Uuid::new_v4().to_string(),
    ))
    .unwrap();
    let capture = captured(&transport, "http://127.0.0.1:1").await;
    let control = control::Control::new(transport, capture, owner(), Instant::now());
    assert!(control.ownership_challenge().await.is_err());
}
