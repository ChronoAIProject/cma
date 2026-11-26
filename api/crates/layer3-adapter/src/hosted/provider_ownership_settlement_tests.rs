use super::super::tests::{json_response, listener, read_request};
use super::*;
use crate::owner_protocol::{
    test_support::TestAuthority, ReceiverRevocation, ReceiverSettlementFailure as Failure,
    ReceiverSettlementWatch,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::AsyncWriteExt;

fn fence(address: std::net::SocketAddr, authority: &Arc<TestAuthority>) -> Arc<ReceiverFence> {
    let template = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        "http://unused.invalid",
        "fixture-key",
    ))
    .unwrap();
    let state = Arc::downgrade(&template.state);
    let binding: Arc<dyn TransportAuthority> = authority.clone();
    Arc::new(ReceiverFence {
        identity: "synthetic-receiver".into(),
        template,
        state,
        endpoint: RuntimeEndpoint::parse("http://fixture.invalid", &format!("http://{address}"))
            .unwrap(),
        owner: authority.owner(),
        authority: Arc::downgrade(&binding),
        legacy_incarnation: None,
    })
}

async fn response(
    bytes: Vec<u8>,
    authority: &Arc<TestAuthority>,
) -> (
    Arc<ReceiverFence>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = listener().await;
    let receiver = fence(listener.local_addr().unwrap(), authority);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = String::from_utf8(read_request(&mut stream).await).unwrap();
        assert!(request.contains("/manager-ownership/revoke"));
        count.fetch_add(1, Ordering::SeqCst);
        stream.write_all(&bytes).await.unwrap();
    });
    (receiver, calls, server)
}

fn raw_body(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).into_bytes()
}

#[tokio::test]
async fn connection_refusal_and_deadline_recover_on_the_same_receiver() {
    for deadline in [false, true] {
        let authority = TestAuthority::new(3);
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let receiver = fence(address, &authority);
        let outcome = ReceiverSettlementWatch::default();
        let listener = if deadline {
            listener
        } else {
            drop(listener);
            assert_eq!(
                receiver
                    .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                    .await,
                Err(Failure::Unavailable)
            );
            assert!(outcome.terminal().is_none());
            tokio::net::TcpListener::bind(address).await.unwrap()
        };
        let reply = json_response(&OwnerAcknowledgement {
            owner: authority.owner(),
            protocol: 1,
        });
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let first_request = if deadline {
                let (mut first, _) = listener.accept().await.unwrap();
                let request = read_request(&mut first).await;
                wait.await.unwrap();
                drop(first);
                Some(request)
            } else {
                None
            };
            let (mut second, _) = listener.accept().await.unwrap();
            let second_request = read_request(&mut second).await;
            if let Some(first_request) = first_request {
                assert_eq!(first_request, second_request);
            }
            second.write_all(&reply).await.unwrap();
        });
        if deadline {
            assert_eq!(
                receiver
                    .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                    .await,
                Err(Failure::Deadline)
            );
            assert!(outcome.terminal().is_none());
            release.send(()).unwrap();
        }
        assert_eq!(
            receiver
                .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                .await,
            Ok(ReceiverCleanup::Acknowledged)
        );
        assert!(outcome.terminal().is_none());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn unclassified_reply_loss_remains_fatal() {
    let authority = TestAuthority::new(3);
    let (receiver, calls, server) = response(vec![], &authority).await;
    let outcome = ReceiverSettlementWatch::default();
    assert_eq!(
        receiver
            .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
            .await,
        Err(Failure::Unclassified)
    );
    assert_eq!(outcome.terminal(), Some(Failure::Unclassified));
    server.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn matching_ack_uses_one_remote_attempt_in_both_projections() {
    for typed in [false, true] {
        let authority = TestAuthority::new(3);
        let reply = json_response(&OwnerAcknowledgement {
            owner: authority.owner(),
            protocol: 1,
        });
        let (receiver, calls, server) = response(reply, &authority).await;
        if typed {
            assert_eq!(
                receiver
                    .settle_typed(
                        ReceiverConfirmationCustody::PossiblyDelivered,
                        &ReceiverSettlementWatch::default()
                    )
                    .await,
                Ok(ReceiverCleanup::Acknowledged)
            );
        } else {
            assert_eq!(
                receiver
                    .settle(ReceiverConfirmationCustody::PossiblyDelivered)
                    .await
                    .unwrap(),
                ReceiverCleanup::Acknowledged
            );
        }
        server.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(authority.failure.get().is_none());
    }
}

#[tokio::test]
async fn invalid_ack_and_status_never_acquire_a_transient_category() {
    for change in [
        "agent",
        "instance",
        "epoch",
        "owner_protocol",
        "protocol",
        "malformed",
        "oversized",
        "duplicate",
        "unknown",
        "auth",
        "missing",
        "refused",
    ] {
        let authority = TestAuthority::new(3);
        let mut reply = serde_json::to_value(OwnerAcknowledgement {
            owner: authority.owner(),
            protocol: 1,
        })
        .unwrap();
        let expected = match change {
            "agent" => {
                reply["owner"]["agent_id"] = "other".into();
                Failure::AuthorityLost
            }
            "instance" => {
                reply["owner"]["instance_id"] = "other".into();
                Failure::AuthorityLost
            }
            "epoch" => {
                reply["owner"]["epoch"] = 4.into();
                Failure::AuthorityLost
            }
            "owner_protocol" => {
                reply["owner"]["protocol"] = 2.into();
                Failure::AuthorityLost
            }
            "protocol" => {
                reply["protocol"] = 2.into();
                Failure::IncompatibleProtocol
            }
            _ => Failure::Invalid,
        };
        let bytes = match change {
            "malformed" => raw_body("not JSON"),
            "oversized" => raw_body(&" ".repeat(RESPONSE_LIMIT + 1)),
            "duplicate" => raw_body(&format!(
                "{{\"protocol\":1,\"protocol\":1,\"owner\":{}}}",
                reply["owner"]
            )),
            "unknown" => {
                reply["unexpected"] = true.into();
                json_response(&reply)
            }
            "auth" => {
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            }
            "missing" => {
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
            }
            "refused" => {
                b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec()
            }
            _ => json_response(&reply),
        };
        let (receiver, calls, server) = response(bytes, &authority).await;
        let outcome = ReceiverSettlementWatch::default();
        assert_eq!(
            receiver
                .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                .await,
            Err(expected),
            "{change}"
        );
        assert_eq!(outcome.terminal(), Some(expected), "{change}");
        server.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn current_exchange_classification_ignores_old_diagnostics_and_their_switch() {
    for enabled in [false, true] {
        for unavailable in [false, true] {
            let authority = TestAuthority::new(3);
            authority
                .diagnostics_enabled
                .store(enabled, Ordering::SeqCst);
            let previous = OwnershipFailure::new(
                Phase::Revoke,
                if unavailable {
                    Reason::ProtocolMismatch
                } else {
                    Reason::TransportUnavailable
                },
            );
            authority.failure.set(previous).unwrap();
            let outcome = ReceiverSettlementWatch::default();
            if unavailable {
                let listener = listener().await;
                let receiver = fence(listener.local_addr().unwrap(), &authority);
                drop(listener);
                assert_eq!(
                    receiver
                        .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                        .await,
                    Err(Failure::Unavailable)
                );
                assert_eq!(outcome.terminal(), None);
            } else {
                let (receiver, _, server) = response(
                    json_response(&OwnerAcknowledgement {
                        owner: authority.owner(),
                        protocol: 2,
                    }),
                    &authority,
                )
                .await;
                assert_eq!(
                    receiver
                        .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                        .await,
                    Err(Failure::IncompatibleProtocol)
                );
                assert_eq!(outcome.terminal(), Some(Failure::IncompatibleProtocol));
                server.await.unwrap();
            }
            assert_eq!(authority.failure.get(), Some(&previous));
        }
    }
}

#[tokio::test]
async fn terminal_validation_survives_slow_diagnostics_and_cancellation() {
    let authority = TestAuthority::new(3);
    *authority.diagnostic_gate.lock().unwrap() = Some(Arc::new(tokio::sync::Notify::new()));
    let (receiver, _, server) = response(
        json_response(&OwnerAcknowledgement {
            owner: authority.owner(),
            protocol: 2,
        }),
        &authority,
    )
    .await;
    let outcome = ReceiverSettlementWatch::default();
    let call = tokio::spawn({
        let outcome = outcome.clone();
        async move {
            receiver
                .settle_typed(ReceiverConfirmationCustody::PossiblyDelivered, &outcome)
                .await
        }
    });
    authority.diagnostic_entered.notified().await;
    assert_eq!(outcome.terminal(), Some(Failure::IncompatibleProtocol));
    assert!(!call.is_finished());
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert_eq!(outcome.terminal(), Some(Failure::IncompatibleProtocol));
    server.await.unwrap();
}
