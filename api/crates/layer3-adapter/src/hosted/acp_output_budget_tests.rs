use super::super::tests::{receive_json, settings, socket_pair, stop, ServerSocket};
use super::super::{OpenedBridge, LANE_OUTPUT_ACK, LANE_RETAINED_OUTPUT};
use super::*;
use crate::{ConversationInput, ConversationSettingsSupport};
use futures_util::StreamExt as _;
use std::time::Duration;

#[path = "acp_output_combined_tests.rs"]
mod combined;

fn stream() -> uuid::Uuid {
    uuid::Uuid::from_u128(1710)
}

pub(super) fn retained(cursor: u64, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES + payload.len());
    bytes.extend_from_slice(stream().as_bytes());
    bytes.extend_from_slice(&cursor.to_be_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

async fn answer(
    server: &mut ServerSocket,
    cursor: u64,
    request: serde_json::Value,
    result: serde_json::Value,
) {
    let payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0", "id":request["id"], "result":result,
    }))
    .unwrap();
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend(retained(cursor, &payload));
    server.send(Message::Binary(bytes)).await.unwrap();
}

async fn open(
    bounded: bool,
) -> (
    OpenedBridge,
    ServerSocket,
    mpsc::Receiver<ConversationUpdate>,
) {
    open_with_permissions(bounded, false).await
}

pub(super) async fn open_with_permissions(
    bounded: bool,
    permissions: bool,
) -> (
    OpenedBridge,
    ServerSocket,
    mpsc::Receiver<ConversationUpdate>,
) {
    open_with_json(bounded, permissions, permissions).await
}

pub(super) async fn open_with_json(
    bounded: bool,
    permissions: bool,
    json: bool,
) -> (
    OpenedBridge,
    ServerSocket,
    mpsc::Receiver<ConversationUpdate>,
) {
    open_with_coordinator(bounded, permissions, json, false).await
}

pub(super) async fn open_with_coordinator(
    bounded: bool,
    permissions: bool,
    json: bool,
    coordinated: bool,
) -> (
    OpenedBridge,
    ServerSocket,
    mpsc::Receiver<ConversationUpdate>,
) {
    let (client, mut server) = socket_pair().await;
    server
        .send(Message::Text(
            serde_json::json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent-1",
                "outputAcknowledgement":{"version":1,"streamId":stream()},
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let mut configured = settings(None, false);
    configured.coordinated_ingress = coordinated;
    configured.retained_json = json;
    configured.bounded_raw_output = bounded;
    configured.bounded_permission_state = permissions;
    configured.output_json_budget = permissions;
    configured.shared_runtime_output = permissions;
    let (sender, mut received) = mpsc::channel(4);
    let opening = tokio::spawn(Bridge::open_with_relay_policy(
        client,
        "agent-1",
        "/workspace".into(),
        (configured, None, (permissions, None)),
        None,
        None,
        sender,
    ));
    let initialize = receive_json(&mut server).await;
    answer(&mut server, 1, initialize, serde_json::json!({})).await;
    let session = receive_json(&mut server).await;
    answer(
        &mut server,
        2,
        session,
        serde_json::json!({"sessionId":"conversation-1"}),
    )
    .await;
    let opened = opening.await.unwrap().unwrap();
    for cursor in 1..=2 {
        let update = tokio::time::timeout(Duration::from_secs(2), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(update, ConversationUpdate::RuntimeOutput { receipt, .. } if receipt.source.cursor == cursor)
        );
    }
    (opened, server, received)
}

async fn budget(opened: &OpenedBridge, bytes: usize, startup: bool) -> Arc<Semaphore> {
    let budget = Arc::new(Semaphore::new(bytes));
    let mut state = opened.bridge.output.as_ref().unwrap().lock().await;
    assert_eq!(state.queued_bytes, 0);
    assert!(state.queued.pop_front().is_none());
    state.raw_budget = opened.bridge.bounded_raw_output.then(|| budget.clone());
    if startup {
        state.session = None;
    }
    budget
}

#[tokio::test]
async fn pressure_refuses_before_cursor_effect_and_receipt_then_same_cursor_recovers() {
    let (opened, mut server, _received) = open(true).await;
    let pool = budget(&opened, 4096, false).await;
    let held = pool.clone().try_acquire_many_owned(4096).unwrap();
    let (capture, mut updates) = UpdateRelay::capture();
    let payload = br#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"conversation-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"accepted once"}}}}"#;
    let bytes = retained(3, payload);
    assert!(matches!(
        opened.bridge.receive_output(&bytes, &capture).await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(2)
    );
    assert!(updates.try_recv().is_err());
    assert_eq!(
        *opened
            .bridge
            .managed_context_progress()
            .await
            .unwrap()
            .borrow(),
        0
    );
    drop(held);
    opened
        .bridge
        .receive_output(&bytes, &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, update } = updates.try_recv().unwrap() else {
        panic!("source receipt")
    };
    assert!(update.is_some());
    assert_eq!(receipt.source.cursor, 3);
    assert_eq!(
        receipt.source.payload_sha256,
        format!("{:x}", Sha256::digest(payload))
    );
    assert_eq!(
        pool.available_permits(),
        4096,
        "raw storage ends after capture; decoded effect is separate"
    );
    receipt.acknowledge().await.unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Binary(ack) = ack else {
        panic!("ACK frame")
    };
    assert_eq!(ack[0], LANE_OUTPUT_ACK);
    assert_eq!(&ack[1..17], stream().as_bytes());
    assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), 3);
    stop(opened).await;
}

#[tokio::test]
async fn startup_raw_storage_stays_charged_until_ordered_activation() {
    let (opened, _server, _received) = open(true).await;
    let pool = budget(&opened, 8192, true).await;
    let (capture, mut updates) = UpdateRelay::capture();
    for cursor in 3..=5 {
        opened
            .bridge
            .receive_output(&retained(cursor, b"{}"), &capture)
            .await
            .unwrap();
    }
    assert!(pool.available_permits() < 8192);
    assert!(updates.try_recv().is_err());
    opened
        .bridge
        .activate_output("conversation-1", &capture)
        .await
        .unwrap();
    for cursor in 3..=5 {
        assert!(
            matches!(updates.try_recv().unwrap(), ConversationUpdate::RuntimeOutput { receipt, .. } if receipt.source.cursor == cursor)
        );
    }
    assert_eq!(pool.available_permits(), 8192);
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .queued_bytes,
        0
    );
    stop(opened).await;
}

#[tokio::test]
async fn cancelled_bootstrap_releases_raw_copy_without_ack_or_domain_publication() {
    let (opened, _server, _received) = open(true).await;
    let pool = budget(&opened, 8192, true).await;
    let (capture, mut updates) = UpdateRelay::capture();
    let pending = opened.bridge.pending.lock().await;
    let bytes = retained(3, br#"{"jsonrpc":"2.0","id":"bootstrap","result":{}}"#);
    let mut receiving = Box::pin(opened.bridge.receive_output(&bytes, &capture));
    assert!(futures_util::poll!(&mut receiving).is_pending());
    assert!(
        pool.available_permits() < 8192,
        "raw copy remains charged at the bootstrap barrier"
    );
    drop(receiving);
    drop(pending);
    assert_eq!(pool.available_permits(), 8192);
    assert!(updates.try_recv().is_err());
    stop(opened).await;
}

#[tokio::test]
async fn startup_wire_boundary_and_oversized_frame_refuse_without_more_storage() {
    let (opened, _server, _received) = open(true).await;
    let pool = budget(&opened, 16 * 1024 * 1024, true).await;
    let (capture, mut updates) = UpdateRelay::capture();
    for (cursor, len) in [
        (3, MAX_FRAME_BYTES),
        (4, MAX_FRAME_BYTES - 2 * HEADER_BYTES),
    ] {
        let mut payload = vec![b' '; len];
        payload[..2].copy_from_slice(b"{}");
        opened
            .bridge
            .receive_output(&retained(cursor, &payload), &capture)
            .await
            .unwrap();
    }
    let remaining = pool.available_permits();
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .queued_bytes,
        STARTUP_BYTES
    );
    assert!(matches!(
        opened
            .bridge
            .receive_output(&retained(5, b"{}"), &capture)
            .await,
        Err(AdapterError::Unavailable(_))
    ));
    assert_eq!(pool.available_permits(), remaining);
    assert!(matches!(
        opened
            .bridge
            .receive_output(&retained(5, &vec![0; MAX_FRAME_BYTES + 1]), &capture)
            .await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(pool.available_permits(), remaining);
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(4)
    );
    assert!(updates.try_recv().is_err());
    stop(opened).await;
    // The input writer is detached; wait for its final bridge owner to drop.
    // Acquiring the entire pool proves actual storage release, not elapsed time.
    let released = tokio::time::timeout(
        Duration::from_secs(2),
        pool.clone().acquire_many_owned(16 * 1024 * 1024),
    )
    .await
    .expect("final bridge owner releases queued storage")
    .unwrap();
    drop(released);
    assert_eq!(pool.available_permits(), 16 * 1024 * 1024);
}

#[tokio::test]
async fn enabled_policy_requires_retained_output_and_disabled_policy_keeps_legacy_storage() {
    let (client, mut server) = socket_pair().await;
    server
        .send(Message::Text(
            serde_json::json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent-1",
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let mut configured = settings(None, false);
    configured.bounded_raw_output = true;
    let (sender, _received) = mpsc::channel(4);
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
    let (opened, _server, _received) = open(false).await;
    let pool = budget(&opened, 0, true).await;
    let (capture, mut updates) = UpdateRelay::capture();
    opened
        .bridge
        .receive_output(&retained(3, b"{}"), &capture)
        .await
        .unwrap();
    assert!(matches!(
        opened.bridge.output.as_ref().unwrap().lock().await.queued,
        OutputQueue::Legacy(_)
    ));
    opened
        .bridge
        .activate_output("conversation-1", &capture)
        .await
        .unwrap();
    assert!(updates.try_recv().is_ok());
    assert_eq!(pool.available_permits(), 0);
    stop(opened).await;
}

#[tokio::test]
async fn socket_refusal_does_not_synthesize_ack_receipt_or_legacy_terminal() {
    let (mut opened, mut server, mut received) = open(true).await;
    budget(&opened, 0, false).await;
    let (capture, mut prompt_effects) = UpdateRelay::capture();
    opened
        .bridge
        .drive(ConversationInput::Prompt(vec![]), &capture)
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await["method"], "session/prompt");
    let mut frame = vec![LANE_RETAINED_OUTPUT];
    frame.extend(retained(3, b"{}"));
    server.send(Message::Binary(frame)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
        .await
        .unwrap()
        .unwrap();
    assert!(received.try_recv().is_err());
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
            .is_none()
    );
    opened.bridge.retire().await;
    opened.relay.abort();
    let _ = opened.relay.await;
}
