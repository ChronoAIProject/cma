use super::*;
use crate::hosted::permission_state::{MAX_ENTRIES, MAX_OPTION_BYTES};

#[path = "acp_permission_uncertainty_tests.rs"]
mod uncertainty;

#[path = "acp_integrated_policy_tests.rs"]
mod integrated;

fn stream_id() -> uuid::Uuid {
    uuid::Uuid::from_u128(19)
}

fn permission(call: u64, option: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0", "id":call, "method":"session/request_permission",
        "params":{"sessionId":"conversation-1", "toolCall":{"toolCallId":"tool", "title":"Permission"},
            "options":[{"optionId":option,"name":"Allow", "kind":"allow_once"}]}})
}

fn retained_bytes(cursor: u64, value: &serde_json::Value) -> Vec<u8> {
    let mut bytes = stream_id().as_bytes().to_vec();
    bytes.extend_from_slice(&cursor.to_be_bytes());
    bytes.extend(serde_json::to_vec(value).unwrap());
    bytes
}

async fn open(
    enabled: bool,
) -> (
    OpenedBridge,
    ServerSocket,
    Arc<AtomicBool>,
    mpsc::Receiver<ConversationUpdate>,
) {
    open_policies(enabled, false).await
}

async fn open_policies(
    enabled: bool,
    combined: bool,
) -> (
    OpenedBridge,
    ServerSocket,
    Arc<AtomicBool>,
    mpsc::Receiver<ConversationUpdate>,
) {
    let (client, mut server) = socket_pair().await;
    let fail_flush = Arc::new(AtomicBool::new(false));
    let socket: Socket = Box::new(uncertainty::UncertainSocket {
        inner: client,
        fail_flush: fail_flush.clone(),
    });
    server
        .send(Message::Text(
            serde_json::json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent-1",
                "outputAcknowledgement":{"version":1,"streamId":stream_id()}
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let (sender, mut received) = mpsc::channel(8);
    let mut configured = settings(None, false);
    configured.bounded_permission_state = enabled;
    configured.output_json_budget = combined;
    configured.shared_runtime_output = combined;
    let opening = Bridge::open_with_relay_policy(
        socket,
        "agent-1",
        "/workspace".into(),
        (configured, None, (combined, None)),
        None,
        None,
        sender,
    );
    let handshake = async {
        for (cursor, result) in [
            (1, serde_json::json!({})),
            (2, serde_json::json!({"sessionId":"conversation-1"})),
        ] {
            let request = receive_json(&mut server).await;
            let mut frame = vec![LANE_RETAINED_OUTPUT];
            frame.extend(retained_bytes(
                cursor,
                &serde_json::json!({"jsonrpc":"2.0", "id":request["id"], "result":result}),
            ));
            server.send(Message::Binary(frame)).await.unwrap();
        }
    };
    let (opened, ()) = tokio::join!(opening, handshake);
    let opened = opened.unwrap();
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
    }
    (opened, server, fail_flush, received)
}

fn request(update: &ConversationUpdate) -> &crate::PermissionRequest {
    match update {
        ConversationUpdate::RuntimeOutput {
            update: Some(update),
            ..
        } => request(update),
        ConversationUpdate::WithoutPromptEvidence(update) => request(update),
        ConversationUpdate::Asked(request) => request,
        other => panic!("expected permission, got {other:?}"),
    }
}

async fn accept(bridge: &Bridge, cursor: u64, call: u64) -> crate::PermissionRequest {
    let (capture, mut updates) = UpdateRelay::capture();
    bridge
        .receive_output(
            &retained_bytes(cursor, &permission(call, "allow")),
            &capture,
        )
        .await
        .unwrap();
    request(&updates.try_recv().unwrap()).clone()
}

#[tokio::test]
async fn full_retained_table_refuses_before_receipt_and_keeps_answer_replay() {
    let (opened, mut server, _, _received) = open(true).await;
    let first = accept(&opened.bridge, 3, 1).await;
    for n in 2..=MAX_ENTRIES {
        accept(&opened.bridge, n as u64 + 2, n as u64).await;
    }
    assert!(opened.bridge.blocked.lock().await.is_empty());
    assert!(opened.bridge.answered.lock().await.is_empty());
    let (capture, mut updates) = UpdateRelay::capture();
    let rejected = retained_bytes(MAX_ENTRIES as u64 + 3, &permission(999, "allow"));
    assert!(matches!(
        opened.bridge.receive_output(&rejected, &capture).await,
        Err(AdapterError::Unavailable(_))
    ));
    assert!(
        updates.try_recv().is_err(),
        "no domain effect or source receipt for refused state"
    );
    let refusal = receive_json(&mut server).await;
    assert_eq!(refusal["error"]["code"], -32002);
    opened
        .bridge
        .bounded_permission_answer(
            &first.id,
            first.locator.as_ref(),
            "conversation-1",
            PermissionDecision::Allow,
        )
        .await
        .unwrap();
    let answer = receive_json(&mut server).await;
    opened
        .bridge
        .bounded_permission_answer(
            &first.id,
            first.locator.as_ref(),
            "conversation-1",
            PermissionDecision::Deny,
        )
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await, answer);
    assert!(updates.try_recv().is_err());
    use crate::ConversationSettingsSupport;
    assert_eq!(
        *opened
            .bridge
            .managed_context_progress()
            .await
            .unwrap()
            .borrow(),
        0
    );
    stop(opened).await;
}

#[tokio::test]
async fn retained_permission_shape_refusal_has_no_receipt_or_ack() {
    let (opened, mut server, _, _received) = open(true).await;
    let (capture, mut updates) = UpdateRelay::capture();
    let bytes = retained_bytes(3, &permission(1, &"x".repeat(MAX_OPTION_BYTES + 1)));
    assert!(matches!(
        opened.bridge.receive_output(&bytes, &capture).await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(receive_json(&mut server).await["error"]["code"], -32002);
    assert!(updates.try_recv().is_err());
    stop(opened).await;
}

#[tokio::test]
async fn disabled_policy_preserves_legacy_capacity_and_option_shape() {
    let (opened, _server, _, _received) = open(false).await;
    let (capture, mut updates) = UpdateRelay::capture();
    for n in 1..=MAX_ENTRIES + 1 {
        let bytes = retained_bytes(
            n as u64 + 2,
            &permission(n as u64, &"x".repeat(MAX_OPTION_BYTES + 1)),
        );
        opened
            .bridge
            .receive_output(&bytes, &capture)
            .await
            .unwrap();
        assert_eq!(
            request(&updates.try_recv().unwrap()).request.options[0]
                .option_id
                .0
                .len(),
            MAX_OPTION_BYTES + 1
        );
    }
    assert!(opened.bridge.permission_state.is_none());
    assert_eq!(opened.bridge.blocked.lock().await.len(), MAX_ENTRIES + 1);
    stop(opened).await;
}

#[tokio::test]
async fn enabled_policy_requires_retained_protocol_before_handshake_work() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (sender, mut received) = mpsc::channel(4);
    let mut configured = settings(None, false);
    configured.bounded_permission_state = true;
    assert!(matches!(
        Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            configured,
            None,
            sender
        )
        .await,
        Err(AdapterError::Rejected(_))
    ));
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn socket_reader_stops_on_pressure_without_receipt_ack_or_terminal() {
    let (mut opened, mut server, _, mut received) = open(true).await;
    let (capture, mut prompt_effects) = UpdateRelay::capture();
    opened
        .bridge
        .drive(ConversationInput::Prompt(vec![]), &capture)
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await["method"], "session/prompt");
    let mut frame = vec![LANE_RETAINED_OUTPUT];
    frame.extend(retained_bytes(
        3,
        &permission(1, &"x".repeat(MAX_OPTION_BYTES + 1)),
    ));
    server.send(Message::Binary(frame)).await.unwrap();
    let refusal = receive_json(&mut server).await;
    assert_eq!(refusal["error"]["code"], -32002);
    tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
        .await
        .unwrap()
        .unwrap();
    assert!(
        received.try_recv().is_err(),
        "refused input produced no source receipt or terminal"
    );
    use crate::ConversationSettingsSupport;
    assert_eq!(
        *opened
            .bridge
            .managed_context_progress()
            .await
            .unwrap()
            .borrow(),
        0
    );
    drop(capture);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), prompt_effects.recv())
            .await
            .unwrap()
            .is_none(),
        "link refusal cannot synthesize a legacy terminal"
    );
    assert_eq!(opened.bridge.owned_prompt_waiters.load(Ordering::SeqCst), 0);
    opened.bridge.retire().await;
    opened.relay.abort();
    let _ = opened.relay.await;
}

#[tokio::test]
async fn prompt_transition_does_not_reuse_an_earlier_turns_approval() {
    let (opened, mut server, _, _received) = open(true).await;
    let first_prompt = crate::PromptIdentity::new("conversation-1", 1, 1);
    let second_prompt = crate::PromptIdentity::new("conversation-1", 1, 2);
    let waiting = opened
        .bridge
        .begin_prompt(serde_json::json!({}), Some(first_prompt.clone()))
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await["id"], first_prompt.as_str());
    let first = accept(&opened.bridge, 3, 1).await;
    let (capture, mut received) = UpdateRelay::capture();
    opened
        .bridge
        .drive(
            ConversationInput::PermissionAnswer {
                id: first.id.clone(),
                locator: first.locator.clone(),
                decision: PermissionDecision::Allow,
            },
            &capture,
        )
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut server).await["result"]["outcome"]["optionId"],
        "allow"
    );
    opened
        .bridge
        .receive_output(
            &retained_bytes(
                4,
                &serde_json::json!({
                    "jsonrpc":"2.0", "id":first_prompt.as_str(), "result":{"stopReason":"end_turn"}
                }),
            ),
            &capture,
        )
        .await
        .unwrap();
    waiting.await.unwrap();
    assert!(matches!(
        received.try_recv().unwrap(),
        ConversationUpdate::RuntimeOutput { .. }
    ));
    let _second_waiting = opened
        .bridge
        .begin_prompt(serde_json::json!({}), Some(second_prompt.clone()))
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut server).await["id"],
        second_prompt.as_str()
    );
    let second = accept(&opened.bridge, 5, 1).await;
    assert_eq!(second.id, first.id, "durable locator format is unchanged");
    opened
        .bridge
        .drive(
            ConversationInput::PermissionAnswer {
                id: second.id,
                locator: second.locator,
                decision: PermissionDecision::Deny,
            },
            &capture,
        )
        .await
        .unwrap();
    assert_eq!(
        receive_json(&mut server).await["result"]["outcome"]["outcome"],
        "cancelled"
    );
    stop(opened).await;
}
