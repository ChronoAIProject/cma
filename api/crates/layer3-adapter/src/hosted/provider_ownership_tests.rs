use super::*;
use crate::owner_protocol::{
    scope, test_support::TestAuthority, OwnershipFailure, OwnershipFailureReason as Reason,
    OwnershipPhase as Phase, TransportAuthority, OWNER_PROTOCOL_VERSION, RECEIVER_LEASE_MS,
};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn owned_control_challenges_then_renews_then_confirms_before_effect() {
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    let endpoint =
        RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}/proxy"))
            .unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let authority = TestAuthority::new(3);
    let events = authority.events.clone();
    let expected = authority.owner();
    let server = tokio::spawn(async move {
        for action in ["challenge", "confirm", "control"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_text(read_request(&mut stream).await);
            assert!(request.starts_with(&format!(
                "POST /proxy/runtime/{}",
                if action == "control" {
                    "control".into()
                } else {
                    format!("agents/agent-1/manager-ownership/{action}")
                }
            )));
            assert!(request
                .to_ascii_lowercase()
                .contains("open-sandbox-api-key: fixture-key"));
            let response = match action {
                "challenge" => {
                    events.lock().unwrap().push("challenge");
                    serde_json::json!({"protocol": OWNER_PROTOCOL_VERSION, "nonce": uuid::Uuid::new_v4(), "valid_for_ms": RECEIVER_LEASE_MS})
                }
                "confirm" => {
                    assert_eq!(&*events.lock().unwrap(), &["challenge", "renew"]);
                    events.lock().unwrap().push("confirm");
                    serde_json::json!({"protocol": OWNER_PROTOCOL_VERSION, "owner": expected})
                }
                _ => {
                    assert!(request.contains(&expected.header().unwrap()));
                    events.lock().unwrap().push("control");
                    serde_json::to_value(ResponseFrame::Resources(ResourceReading::default()))
                        .unwrap()
                }
            };
            stream.write_all(&json_response(&response)).await.unwrap();
        }
    });
    scope(
        authority.clone(),
        transport.control(&endpoint, &RequestFrame::Resources),
    )
    .await
    .unwrap();
    server.await.unwrap();
    assert_eq!(
        &*authority.events.lock().unwrap(),
        &["challenge", "renew", "confirm", "control"]
    );
    assert_eq!(authority.renewals.load(Ordering::SeqCst), 1);
    assert!(authority.failure.get().is_none());
    assert_eq!(authority.confirmed_bindings.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn missing_capability_and_partition_never_send_confirmation_or_effect() {
    for partition in [false, true] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let endpoint =
            RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new("http://unused.invalid", "fixture-key"),
        )
        .unwrap();
        let authority = TestAuthority::new(1);
        authority.fail_renewal.store(partition, Ordering::SeqCst);
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_text(read_request(&mut stream).await);
            assert!(request.contains("/manager-ownership/challenge"));
            let response = if partition {
                json_response(
                    &serde_json::json!({"protocol":1,"nonce":uuid::Uuid::new_v4(),"valid_for_ms":RECEIVER_LEASE_MS}),
                )
            } else {
                http_response("404 Not Found", &[("Content-Length", "0".into())], &[])
            };
            stream.write_all(&response).await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(80), listener.accept())
                    .await
                    .is_err()
            );
        });
        assert!(scope(
            authority.clone(),
            transport.control(&endpoint, &RequestFrame::Resources)
        )
        .await
        .is_err());
        assert!(authority.check().await.is_err());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn switch_off_never_contacts_ownership_endpoint() {
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let endpoint = RuntimeEndpoint::parse("http://fixture.invalid", "http://127.0.0.1:1").unwrap();
    assert!(transport.receiver_lease(&endpoint).await.unwrap().is_none());
}

#[tokio::test]
async fn retained_receiver_with_incompatible_lifetime_never_renews_or_confirms() {
    for native in [false, true] {
        for valid_for_ms in [0_u64, 10_001, 15_000, 20_000, u64::MAX] {
            let listener = listener().await;
            let address = listener.local_addr().unwrap();
            let endpoint =
                RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}"))
                    .unwrap();
            let (transport, endpoint) = if native {
                (
                    super::agent_sandbox::native_transport(address),
                    endpoint.with_control_token("synthetic-control".into()),
                )
            } else {
                (
                    ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
                        "http://unused.invalid",
                        "fixture-key",
                    ))
                    .unwrap(),
                    endpoint,
                )
            };
            let authority = TestAuthority::new(1);
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = request_text(read_request(&mut stream).await);
                assert!(request.contains("/manager-ownership/challenge"));
                stream
                    .write_all(&json_response(&serde_json::json!({
                        "protocol": OWNER_PROTOCOL_VERSION,
                        "nonce": uuid::Uuid::new_v4(),
                        "valid_for_ms": valid_for_ms,
                    })))
                    .await
                    .unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(80), listener.accept())
                        .await
                        .is_err()
                );
            });
            assert!(scope(
                authority.clone(),
                transport.control(&endpoint, &RequestFrame::Resources)
            )
            .await
            .is_err());
            assert_eq!(
                authority.failure.get(),
                Some(&OwnershipFailure {
                    phase: Phase::Challenge,
                    reason: Reason::LeaseMismatch,
                    http_status: Some(200),
                    protocol: Some(OWNER_PROTOCOL_VERSION),
                    lease_ms: Some(valid_for_ms),
                })
            );
            assert!(authority.failure_before_revocation.load(Ordering::SeqCst));
            assert_eq!(authority.renewals.load(Ordering::SeqCst), 0);
            assert!(!authority.is_current());
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn rejection_records_safe_phase_and_reason_before_revocation() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        for (status, body, reason, protocol) in [
            (
                "404 Not Found",
                "private-response-marker",
                Reason::MissingRoute,
                None,
            ),
            (
                "401 Unauthorized",
                "private-response-marker",
                Reason::HttpAuthRefused,
                None,
            ),
            (
                "403 Forbidden",
                "private-response-marker",
                Reason::HttpAuthRefused,
                None,
            ),
            (
                "409 Conflict",
                "private-response-marker",
                Reason::HttpRefused,
                None,
            ),
            (
                "503 Unavailable",
                "private-response-marker",
                Reason::HttpRefused,
                None,
            ),
            (
                "200 OK",
                "private-response-marker",
                Reason::MalformedReply,
                None,
            ),
            (
                "200 OK",
                r#"{"protocol":1,"valid_for_ms":10000}"#,
                Reason::MalformedReply,
                Some(1),
            ),
            (
                "200 OK",
                r#"{"protocol":1,"protocol":1,"nonce":"00000000-0000-0000-0000-000000000001","valid_for_ms":10000}"#,
                Reason::MalformedReply,
                Some(1),
            ),
            (
                "200 OK",
                r#"{"protocol":99,"nonce":"00000000-0000-0000-0000-000000000001","valid_for_ms":10000}"#,
                Reason::ProtocolMismatch,
                Some(99),
            ),
        ] {
            // The protocol mismatch must have the acknowledgement shape at confirmation.
            let body = if phase == Phase::Confirm && protocol == Some(99) {
                serde_json::json!({"protocol":99,"owner":TestAuthority::new(1).owner()}).to_string()
            } else {
                body.into()
            };
            let expected_status = status[..3].parse().unwrap();
            let failure = failed_exchange(phase, Some((status, body)), false).await;
            assert_eq!(failure.phase, phase);
            assert_eq!(failure.reason, reason);
            assert_eq!(failure.http_status, Some(expected_status));
            assert_eq!(failure.protocol, protocol);
            let serialized = serde_json::to_string(&failure).unwrap();
            assert!(!serialized.contains("private-response-marker"));
            assert!(!serialized.contains("fixture-key"));
            assert!(!serialized.contains("nonce"));
        }
    }
}

#[tokio::test]
async fn deadline_and_store_failure_name_the_phase_without_sending_effects() {
    for phase in [Phase::Challenge, Phase::Confirm] {
        let failure = failed_exchange(phase, None, false).await;
        assert_eq!(failure, OwnershipFailure::new(phase, Reason::Deadline));
    }
    assert_eq!(
        failed_exchange(Phase::Renew, None, true).await,
        OwnershipFailure::new(Phase::Renew, Reason::AuthorityLost)
    );
}

#[tokio::test]
async fn invalid_nonce_and_wrong_acknowledged_owner_are_distinct_safe_failures() {
    let challenge =
        serde_json::json!({"protocol":1,"nonce":"private-response-marker","valid_for_ms":10000});
    let failure = failed_exchange(
        Phase::Challenge,
        Some(("200 OK", challenge.to_string())),
        false,
    )
    .await;
    assert_eq!(failure.reason, Reason::MalformedReply);
    assert_eq!(failure.lease_ms, Some(10000));
    let mut owner = TestAuthority::new(1).owner();
    owner.epoch += 1;
    let confirmation = serde_json::json!({"protocol":1,"owner":owner});
    let failure = failed_exchange(
        Phase::Confirm,
        Some(("200 OK", confirmation.to_string())),
        false,
    )
    .await;
    assert_eq!(failure.reason, Reason::AuthorityLost);
    assert_eq!(failure.protocol, Some(1));
}

async fn failed_exchange(
    phase: Phase,
    response: Option<(&str, String)>,
    fail_renewal: bool,
) -> OwnershipFailure {
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    let endpoint =
        RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let authority = TestAuthority::new(1);
    authority.fail_renewal.store(fail_renewal, Ordering::SeqCst);
    let response = response.map(|(status, body)| {
        http_response(
            status,
            &[("Content-Length", body.len().to_string())],
            body.as_bytes(),
        )
    });
    let server = tokio::spawn(async move {
        if phase != Phase::Challenge {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(request_text(read_request(&mut stream).await)
                .contains("/manager-ownership/challenge"));
            stream
                .write_all(&json_response(&serde_json::json!({
                    "protocol":1,"nonce":uuid::Uuid::new_v4(),"valid_for_ms":10000
                })))
                .await
                .unwrap();
        }
        if phase != Phase::Renew {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(request_text(read_request(&mut stream).await)
                .contains(&format!("/manager-ownership/{}", phase.as_str())));
            if let Some(response) = response {
                stream.write_all(&response).await.unwrap();
            } else {
                // Keep the response pending until the negotiation deadline closes it.
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
            }
        }
        listener
    });
    assert!(scope(
        authority.clone(),
        transport.control(&endpoint, &RequestFrame::Resources)
    )
    .await
    .is_err());
    let listener = server.await.unwrap();
    assert!(!authority.is_current());
    assert!(authority.confirmed_bindings.lock().unwrap().is_empty());
    assert!(authority.failure_before_revocation.load(Ordering::SeqCst));
    let failure = *authority.failure.get().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    // Cleanup has its own failure, but cannot replace the initiating observation.
    let cleanup = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(request_text(read_request(&mut stream).await).contains("/manager-ownership/revoke"));
        stream
            .write_all(&http_response(
                "403 Forbidden",
                &[("Content-Length", "0".into())],
                &[],
            ))
            .await
            .unwrap();
    });
    let receiver = authority.receivers.lock().unwrap()[0].clone();
    assert!(receiver.revoke().await.is_err());
    cleanup.await.unwrap();
    assert_eq!(authority.failure.get(), Some(&failure));
    failure
}

#[tokio::test]
async fn refused_connection_is_transport_unavailable() {
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    drop(listener);
    let endpoint =
        RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let authority = TestAuthority::new(1);
    assert!(
        scope(authority.clone(), transport.receiver_lease(&endpoint))
            .await
            .is_err()
    );
    assert_eq!(
        authority.failure.get(),
        Some(&OwnershipFailure::new(
            Phase::Challenge,
            Reason::TransportUnavailable
        ))
    );
    assert!(authority.failure_before_revocation.load(Ordering::SeqCst));
}

#[tokio::test]
async fn deadline_after_response_headers_retains_the_http_status() {
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    let endpoint =
        RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let authority = TestAuthority::new(1);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n")
            .await
            .unwrap();
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
    });
    assert!(
        scope(authority.clone(), transport.receiver_lease(&endpoint))
            .await
            .is_err()
    );
    server.await.unwrap();
    assert_eq!(
        authority.failure.get(),
        Some(&OwnershipFailure::new(Phase::Challenge, Reason::Deadline).with_status(200))
    );
    assert!(authority.failure_before_revocation.load(Ordering::SeqCst));
}

#[path = "provider_ownership_retirement_tests.rs"]
mod retirement;
