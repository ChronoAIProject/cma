use super::super::tests::{json_response, listener, read_request};
use super::*;
use crate::owner_protocol::{scope, test_support::TestAuthority, ReceiverRevocation};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn first_revocation_failure_is_classified_and_success_has_no_failure() {
    for refuse in [false, true] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let endpoint =
            RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}")).unwrap();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new("http://unused.invalid", "fixture-key"),
        )
        .unwrap();
        let authority = TestAuthority::new(1);
        let owner = authority.owner();
        let server = tokio::spawn(async move {
            for action in ["challenge", "confirm", "revoke"] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = String::from_utf8(read_request(&mut stream).await).unwrap();
                assert!(request.contains(&format!("/manager-ownership/{action}")));
                let bytes = if action == "revoke" && refuse {
                    b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
                        .to_vec()
                } else if action == "challenge" {
                    json_response(
                        &serde_json::json!({"protocol":1,"nonce":uuid::Uuid::new_v4(),"valid_for_ms":10000}),
                    )
                } else {
                    json_response(&OwnerAcknowledgement {
                        owner: owner.clone(),
                        protocol: 1,
                    })
                };
                stream.write_all(&bytes).await.unwrap();
            }
        });
        let lease = scope(authority.clone(), transport.receiver_lease(&endpoint))
            .await
            .unwrap();
        assert!(authority.failure.get().is_none());
        let receiver: Arc<dyn ReceiverRevocation> = authority.receivers.lock().unwrap()[0].clone();
        assert_eq!(receiver.revoke().await.is_err(), refuse);
        server.await.unwrap();
        assert_eq!(
            authority.failure.get().copied(),
            refuse.then(|| OwnershipFailure::http(Phase::Revoke, 403))
        );
        drop(lease);
    }
}
