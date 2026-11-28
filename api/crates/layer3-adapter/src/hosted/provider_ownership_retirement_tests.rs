use super::*;
use crate::receiver_failure::{self, ChallengeRefusal};

#[tokio::test]
async fn retirement_probe_rechecks_failure_without_confirmation_or_owner_renewal() {
    for (first, expected_refusal) in [
        ("404 Not Found", Some(ChallengeRefusal::NotFound)),
        (
            "503 Service Unavailable",
            Some(ChallengeRefusal::ServerError),
        ),
        ("409 Conflict", None),
        ("403 Forbidden", None),
    ] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let handle = RuntimeHandle::new("agent-sandbox:fixture:original");
        let binding = crate::agent_sandbox::PodBinding {
            sandbox_name: "fixture".into(),
            sandbox_uid: "original".into(),
            pod_name: "fixture-pod".into(),
            pod_uid: "original-pod".into(),
            pod_ip: address.ip(),
            runtime_container_id: Some("containerd://fixture".into()),
            runtime_restart_count: Some(0),
        };
        let endpoint =
            RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}"))
                .unwrap()
                .with_direct_receiver(&handle, &binding);
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
        let (finished, outcome) = tokio::sync::oneshot::channel();
        let (capture, attempt) = receiver_failure::capture(async {
            let result = receiver_failure::observe(scope(
                authority.clone(),
                transport.control(&endpoint, &RequestFrame::Resources),
            ))
            .await;
            assert!(finished.send(result).is_ok());
            std::future::pending::<()>().await;
        });
        let (result, observed) = tokio::select! {
            result = outcome => result.unwrap(),
            () = attempt => panic!("the scoped attempt must be dropped while pending"),
        };
        assert!(result.is_err());
        assert_eq!(
            capture.sealed().map(|failure| failure.refusal()),
            expected_refusal
        );
        if let Some(failure) = capture.sealed() {
            assert_eq!(failure.receiver().runtime, handle);
            assert_eq!(failure.receiver().pod.uid, "original-pod");
            assert_eq!(failure.receiver().owner, authority.owner());
        }
        if first.starts_with("404") {
            let receiver = observed.unwrap().into_receiver().unwrap();
            assert_eq!(receiver.runtime, handle);
            assert_eq!(receiver.pod.uid, "original-pod");
            let acting = ProviderHostedControlTransport::for_retained_opensandbox(
                SandboxClient::new("http://unused.invalid", "fixture-key"),
            )
            .unwrap();
            assert!(acting
                .challenge_only(&endpoint, &receiver.owner, false)
                .await
                .unwrap());
            assert!(acting
                .challenge_only(&endpoint, &receiver.owner, false)
                .await
                .is_err());
            let (compatible, probe) =
                receiver_failure::capture(acting.challenge_only(&endpoint, &receiver.owner, false));
            assert!(!probe.await.unwrap());
            assert!(compatible.sealed().is_none());
        } else {
            assert_eq!(observed.is_some(), expected_refusal.is_some());
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
        ("timeout", false),
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
        let (capture, attempt) =
            receiver_failure::capture(receiver_failure::observe(tokio::time::timeout(
                Duration::from_millis(80),
                transport.control(&endpoint, &request),
            )));
        let (_, failure) = attempt.await;
        assert_eq!(failure.is_some(), expected, "{status}");
        assert!(capture.sealed().is_none(), "{status}");
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

#[tokio::test]
async fn expanded_retirement_challenge_accepts_only_eligible_runtime_failures() {
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let owner = TestAuthority::new(3).owner();
    for (status, eligible) in [
        ("401 Unauthorized", true),
        ("403 Forbidden", true),
        ("405 Method Not Allowed", true),
        ("429 Too Many Requests", false),
        ("409 Conflict", false),
    ] {
        let listener = listener().await;
        let endpoint = RuntimeEndpoint::parse(
            "http://fixture.invalid",
            &format!("http://{}", listener.local_addr().unwrap()),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(request_text(read_request(&mut stream).await)
                .contains("/manager-ownership/challenge"));
            stream
                .write_all(&http_response(
                    status,
                    &[("Content-Length", "0".into())],
                    &[],
                ))
                .await
                .unwrap();
        });
        let result = transport.challenge_only(&endpoint, &owner, true).await;
        assert_eq!(matches!(result, Ok(true)), eligible, "{status}");
        server.await.unwrap();
    }
    let bound = listener().await;
    let endpoint = RuntimeEndpoint::parse(
        "http://fixture.invalid",
        &format!("http://{}", bound.local_addr().unwrap()),
    )
    .unwrap();
    drop(bound);
    assert!(transport
        .challenge_only(&endpoint, &owner, true)
        .await
        .unwrap());
    assert!(transport
        .challenge_only(&endpoint, &owner, false)
        .await
        .is_err());
}
