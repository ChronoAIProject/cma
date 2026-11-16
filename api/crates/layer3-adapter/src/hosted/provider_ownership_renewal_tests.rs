use super::*;
use crate::owner_protocol::{
    scope, test_support::TestAuthority, TransportAuthority, OWNER_PROTOCOL_VERSION,
    RECEIVER_LEASE_MS,
};
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug)]
enum Failure {
    Challenge,
    DurableRenewal,
    Confirmation,
}

#[tokio::test]
async fn periodic_failure_revokes_an_established_receiver_and_blocks_later_control() {
    for native in [false, true] {
        for failure in [
            Failure::Challenge,
            Failure::DurableRenewal,
            Failure::Confirmation,
        ] {
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
            let authority = TestAuthority::new(3);
            let expected = authority.owner();
            let server = tokio::spawn(async move {
                let actions = if matches!(failure, Failure::Confirmation) {
                    vec!["challenge", "confirm", "challenge", "confirm"]
                } else {
                    vec!["challenge", "confirm", "challenge"]
                };
                for (index, action) in actions.into_iter().enumerate() {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let request = request_text(read_request(&mut stream).await);
                    assert!(request.starts_with(&format!(
                        "POST /runtime/agents/agent-1/manager-ownership/{action} "
                    )));
                    let auth = if native {
                        "x-cma-runtime-token: synthetic-control"
                    } else {
                        "open-sandbox-api-key: fixture-key"
                    };
                    assert!(request.to_ascii_lowercase().contains(auth));
                    let response = if index == 2 && matches!(failure, Failure::Challenge) {
                        http_response(
                            "503 Service Unavailable",
                            &[("Content-Length", "0".into())],
                            &[],
                        )
                    } else if action == "challenge" {
                        json_response(&serde_json::json!({
                            "protocol": OWNER_PROTOCOL_VERSION,
                            "nonce": uuid::Uuid::new_v4(),
                            "valid_for_ms": RECEIVER_LEASE_MS,
                        }))
                    } else {
                        let mut acknowledged = expected.clone();
                        if index == 3 {
                            acknowledged.epoch += 1;
                        }
                        json_response(&serde_json::json!({
                            "protocol": OWNER_PROTOCOL_VERSION,
                            "owner": acknowledged,
                        }))
                    };
                    stream.write_all(&response).await.unwrap();
                }
                listener
            });
            // Keep the successfully established lease alive through its actual
            // three-second background renewal; initial attach failure is separate.
            let lease = scope(authority.clone(), transport.receiver_lease(&endpoint))
                .await
                .unwrap()
                .unwrap();
            assert!(authority.is_current());
            assert_eq!(authority.renewals.load(Ordering::SeqCst), 1);
            if matches!(failure, Failure::DurableRenewal) {
                authority.fail_renewal.store(true, Ordering::SeqCst);
            }
            tokio::time::timeout(Duration::from_secs(8), authority.revoked())
                .await
                .unwrap_or_else(|_| {
                    panic!("periodic {failure:?} failure did not revoke authority")
                });
            let listener = tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .expect("expected negotiation requests completed")
                .unwrap();
            assert!(!authority.is_current());
            let renewals = if matches!(failure, Failure::Confirmation) {
                2
            } else {
                1
            };
            assert_eq!(authority.renewals.load(Ordering::SeqCst), renewals);
            // Restoring the store does not restore locally revoked authority.
            authority.fail_renewal.store(false, Ordering::SeqCst);
            assert!(scope(
                authority.clone(),
                transport.control(&endpoint, &RequestFrame::Resources),
            )
            .await
            .is_err());
            assert_eq!(authority.renewals.load(Ordering::SeqCst), renewals);
            assert!(
                tokio::time::timeout(Duration::from_millis(80), listener.accept())
                    .await
                    .is_err()
            );
            drop(lease);
        }
    }
}
