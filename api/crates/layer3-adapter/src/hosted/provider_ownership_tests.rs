use super::*;
use crate::owner_protocol::{
    scope, test_support::TestAuthority, TransportAuthority, OWNER_PROTOCOL_VERSION,
    RECEIVER_LEASE_MS,
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
            assert_eq!(authority.renewals.load(Ordering::SeqCst), 0);
            assert!(!authority.is_current());
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn retirement_probe_rechecks_failure_without_confirmation_or_owner_renewal() {
    for first in ["404 Not Found", "409 Conflict", "403 Forbidden"] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let endpoint =
            RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new("http://unused.invalid", "fixture-key"),
        )
        .unwrap();
        let authority = TestAuthority::new(3);
        let server = tokio::spawn(async move {
            let replies = if first.starts_with("404") {
                vec![first, "404 Not Found", "409 Conflict", "200 OK"]
            } else {
                vec![first]
            };
            for status in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = request_text(read_request(&mut stream).await);
                assert!(request.contains("/manager-ownership/challenge"));
                let response = if status == "200 OK" {
                    json_response(&serde_json::json!({
                        "protocol": OWNER_PROTOCOL_VERSION, "nonce": uuid::Uuid::new_v4(), "valid_for_ms": RECEIVER_LEASE_MS
                    }))
                } else {
                    http_response(status, &[("Content-Length", "0".into())], &[])
                };
                stream.write_all(&response).await.unwrap();
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(80), listener.accept())
                    .await
                    .is_err()
            );
        });
        let (result, observed) = crate::receiver_failure::observe(scope(
            authority.clone(),
            transport.control(&endpoint, &RequestFrame::Resources),
        ))
        .await;
        assert!(result.is_err());
        if first.starts_with("404") {
            let probe = observed.unwrap().into_receiver().unwrap();
            assert!(probe.still_unavailable().await);
            assert!(!probe.still_unavailable().await); // a stale token is not unreachability
            assert!(!probe.still_unavailable().await); // a recovered receiver is not unreachability
        } else {
            assert!(observed.is_none());
        }
        assert_eq!(authority.renewals.load(Ordering::SeqCst), 0);
        assert!(!authority.is_current());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn retirement_status_failure_is_typed_and_scoped_to_an_actual_read_attempt() {
    for (status, expected) in [
        ("404 Not Found", true),
        ("403 Forbidden", false),
        ("timeout", true),
    ] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let endpoint =
            RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new("http://unused.invalid", "fixture-key"),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let text = request_text(read_request(&mut stream).await);
            assert!(text.contains("/runtime/control"));
            if status == "timeout" {
                tokio::time::sleep(Duration::from_millis(120)).await;
            } else {
                stream
                    .write_all(&http_response(
                        status,
                        &[("Content-Length", "0".into())],
                        &[],
                    ))
                    .await
                    .unwrap();
            }
        });
        let request = RequestFrame::RuntimeStatus(crate::RuntimeStatusRequest {
            sandbox_id: "retiring".into(),
            generation: 1,
            home_identity: "a".repeat(64),
        });
        let (_, failure) = crate::receiver_failure::observe(tokio::time::timeout(
            Duration::from_millis(80),
            transport.control(&endpoint, &request),
        ))
        .await;
        assert_eq!(failure.is_some(), expected, "{status}");
        server.await.unwrap();
        // A timeout before the adapter attempts runtime control has no evidence.
        let (_, absent) = crate::receiver_failure::observe(tokio::time::timeout(
            Duration::from_millis(1),
            std::future::pending::<()>(),
        ))
        .await;
        assert!(absent.is_none());
    }
}
