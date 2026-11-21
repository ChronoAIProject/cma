use super::super::tests::{json_response, listener, read_request};
use super::*;
use crate::owner_protocol::test_support::TestAuthority;
use crate::{ConversationInput, ResourceInput};
use tokio::io::AsyncWriteExt;

fn id(value: &str) -> OpaqueId {
    OpaqueId::new(value).unwrap()
}
fn digest(value: char) -> Digest {
    Digest::new(&value.to_string().repeat(64)).unwrap()
}
fn generation(value: u64) -> Generation {
    Generation::new(value).unwrap()
}

async fn fixture() -> (
    ResourceTransport,
    tokio::net::TcpListener,
    Arc<TestAuthority>,
) {
    let listener = listener().await;
    let base = format!("http://{}", listener.local_addr().unwrap());
    let authority = TestAuthority::new(1);
    let client = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        &base,
        uuid::Uuid::new_v4().to_string(),
    ))
    .unwrap();
    let transport = ResourceTransport {
        client,
        endpoint: RuntimeEndpoint::parse(&base, &format!("{base}/proxy/9090")).unwrap(),
        authority: authority.clone(),
        receiver: ReceiverKey {
            kind: ReceiverKind::Runtime,
            instance: id("runtime"),
            incarnation: digest('a'),
            boot: id("boot"),
        },
        capabilities: ExecutionCapabilities {
            protocol: 1,
            targeted_native_stop: true,
            mediated_effects: true,
            mixed_principal_control: false,
        },
        _ownership: None,
    };
    (transport, listener, authority)
}

fn dispatch(transport: &ResourceTransport) -> ResourceDispatch {
    let input = ResourceInput::admit(ConversationInput::Prompt(vec![])).unwrap();
    input
        .clone()
        .bind(ExecutionTicketV1 {
            protocol: 1,
            execution: ExecutionKey {
                command_id: id("command"),
                command_fingerprint: digest('b'),
                dispatch_sequence: generation(1),
                prompt_id: id("prompt"),
                turn_id: id("turn"),
                agent_id: id("agent-1"),
                receiver: transport.receiver.clone(),
                attempt: generation(1),
            },
            basis: ExecutionBasisV1::IndependentOwner {
                owner: digest('c'),
                actor: digest('d'),
                target_ancestry: digest('e'),
                capability: ProtectedCapability::Prompt,
            },
            operation: ExecutionOperation::Prompt,
            input_digest: input.digest().clone(),
            manager: ManagerKey {
                instance: id("member-1"),
                epoch: generation(1),
                agent_id: id("agent-1"),
            },
            challenge: ReceiverChallengeV1 {
                protocol: 1,
                receiver: transport.receiver.clone(),
                nonce: id("challenge"),
                created_tick_ms: 100,
                deadline_tick_ms: 3100,
            },
            nonce: id("ticket"),
            registration: digest('f'),
        })
        .unwrap()
}

#[tokio::test]
async fn resource_start_carries_authentication_and_semantic_input_and_correlates_observation() {
    let (transport, listener, authority) = fixture().await;
    let dispatch = dispatch(&transport);
    let expected = StartObservationV1 {
        protocol: 1,
        execution: dispatch.ticket().execution.clone(),
        registration: dispatch.ticket().registration.clone(),
        nonce: dispatch.ticket().nonce.clone(),
        state: NativeStartState::Entered,
        sequence: generation(2),
    };
    let reply = expected.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        stream.write_all(&json_response(&reply)).await.unwrap();
        request
    });
    let ResourceSendOutcome::Observed(observed) = transport.send_resource(&dispatch).await else {
        panic!("expected correlated receipt")
    };
    assert_eq!(*observed, expected);
    let request = server.await.unwrap();
    let text = String::from_utf8(request).unwrap();
    assert!(text.starts_with("POST /proxy/9090/runtime/resource-execution/v1/agent-1/start "));
    let (headers, body) = text.split_once("\r\n\r\n").unwrap();
    assert!(headers.to_lowercase().contains("x-cma-manager-ownership:"));
    let (name, token) = transport.client.auth_header(&transport.endpoint);
    assert!(headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(header, value)| header.eq_ignore_ascii_case(name) && value.trim() == token));
    assert!(headers.contains(&authority.owner.header().unwrap()));
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        body["ticket"],
        serde_json::to_value(dispatch.ticket()).unwrap()
    );
    assert_eq!(
        body["input"],
        serde_json::from_slice::<serde_json::Value>(dispatch.input().bytes()).unwrap()
    );
}

#[tokio::test]
async fn lost_or_mismatched_start_reply_is_uncertain_without_retry() {
    for mismatch in [false, true] {
        let (transport, listener, _) = fixture().await;
        let dispatch = dispatch(&transport);
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            if mismatch {
                stream
                    .write_all(&json_response(&serde_json::json!({"protocol":1})))
                    .await
                    .unwrap();
            }
            drop(stream);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        assert!(matches!(
            transport.send_resource(&dispatch).await,
            ResourceSendOutcome::Uncertain(_)
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn wrong_boot_or_lost_owner_is_refused_before_network_write() {
    let (transport, listener, authority) = fixture().await;
    let dispatch = dispatch(&transport);
    let mut ticket = dispatch.ticket().clone();
    ticket.execution.receiver.boot = id("wrong-boot");
    ticket.challenge.receiver.boot = id("wrong-boot");
    let wrong = dispatch.input().clone().bind(ticket).unwrap();
    assert!(matches!(
        transport.send_resource(&wrong).await,
        ResourceSendOutcome::Refused(_)
    ));
    authority.revoke();
    assert!(transport.capabilities().is_none());
    assert!(matches!(
        transport.send_resource(&dispatch).await,
        ResourceSendOutcome::Refused(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn oversized_success_or_untrusted_error_body_cannot_escape_the_closed_error_projection() {
    for status in ["200 OK", "503 Service Unavailable"] {
        let (transport, listener, _) = fixture().await;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            let body = "private-remote-detail".repeat(MAX_MESSAGE_BYTES);
            let headers = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(headers.as_bytes()).await;
            let _ = stream.write_all(body.as_bytes()).await;
        });
        let result: ProtocolResult<Continued> = transport
            .exchange("continue", &serde_json::json!({"protocol":1}))
            .await;
        let error = result.err().unwrap();
        assert!(matches!(
            error,
            ProtocolError::Capacity | ProtocolError::Unknown
        ));
        assert!(!error.to_string().contains("private-remote-detail"));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn resource_control_progresses_while_a_start_response_is_held() {
    let (transport, listener, _) = fixture().await;
    let transport = Arc::new(transport);
    let dispatch = dispatch(&transport);
    let (held, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut data, _) = listener.accept().await.unwrap();
        read_request(&mut data).await;
        held.send(()).unwrap();
        let (mut control, _) = listener.accept().await.unwrap();
        let request = read_request(&mut control).await;
        assert!(String::from_utf8(request).unwrap().contains("/stop "));
        control
            .write_all(&json_response(&serde_json::Value::Null))
            .await
            .unwrap();
        released.await.unwrap();
        drop(data);
    });
    let writer = transport.clone();
    let start = tokio::spawn(async move { writer.send_resource(&dispatch).await });
    waiting.await.unwrap();
    let reply: Option<StopReceiptV1> = tokio::time::timeout(
        Duration::from_secs(1),
        transport.exchange("stop", &serde_json::json!({"protocol":1})),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(reply.is_none());
    release.send(()).unwrap();
    assert!(matches!(
        start.await.unwrap(),
        ResourceSendOutcome::Uncertain(_)
    ));
    server.await.unwrap();
}
