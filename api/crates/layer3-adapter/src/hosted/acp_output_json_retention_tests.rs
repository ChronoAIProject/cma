use super::super::tests::{receive_json, settings, socket_pair, stop};
use super::super::LANE_RETAINED_OUTPUT;
use super::budget_tests::{open_with_json, open_with_permissions, retained};
use super::*;
use crate::hosted::retained_json::JsonPools;
use crate::ConversationInput;
use futures_util::StreamExt as _;
use std::time::Duration;

async fn pool(bridge: &Bridge, trees: usize, scratch: usize, startup: bool) -> JsonPools {
    let pools = JsonPools::new(trees, scratch);
    let mut state = bridge.output.as_ref().unwrap().lock().await;
    state.json_pools = Some(pools.clone());
    if startup {
        state.session = None;
    }
    pools
}

#[tokio::test]
async fn reply_outlives_raw_capture_and_receipt_then_releases_on_last_consumer() {
    let (opened, mut server, _received) = open_with_permissions(true, true).await;
    let pools = pool(&opened.bridge, 1024 * 1024, 1024 * 1024, false).await;
    let waiting = opened
        .bridge
        .begin("example", serde_json::json!({}))
        .await
        .unwrap();
    let request = receive_json(&mut server).await;
    let payload =
        serde_json::to_vec(&serde_json::json!({"id":request["id"],"result":{"answer":42}}))
            .unwrap();
    let (capture, mut effects) = UpdateRelay::capture();
    opened
        .bridge
        .receive_output(&retained(3, &payload), &capture)
        .await
        .unwrap();
    let receipt = effects.try_recv().unwrap();
    let frame = waiting.await.unwrap();
    let result = crate::hosted::acp::rpc_result("example", frame).unwrap();
    assert_eq!(result["answer"], 42);
    let held = pools.available();
    assert!(held.0 < 1024 * 1024);
    assert_eq!(held.1, 1024 * 1024);
    drop(receipt);
    assert_eq!(
        pools.available(),
        held,
        "receipt drop cannot free reply memory"
    );
    let clone = result.clone();
    drop(result);
    assert_eq!(pools.available(), held);
    drop(clone);
    assert_eq!(pools.available(), (1024 * 1024, 1024 * 1024));
    stop(opened).await;
}

#[tokio::test]
async fn startup_retains_parsed_owner_and_refusal_keeps_original_cursor() {
    let (opened, _server, _received) = open_with_permissions(true, true).await;
    let pools = pool(&opened.bridge, 1024 * 1024, 1024 * 1024, true).await;
    let (capture, mut effects) = UpdateRelay::capture();
    let payload = br#"{"method":"session/update","params":{"sessionId":"conversation-1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"one"}}}}"#;
    opened
        .bridge
        .receive_output(&retained(3, payload), &capture)
        .await
        .unwrap();
    assert!(effects.try_recv().is_err());
    assert!(pools.available().0 < 1024 * 1024);
    opened
        .bridge
        .activate_output("conversation-1", &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, update } = effects.try_recv().unwrap() else {
        panic!("receipt missing")
    };
    assert_eq!(receipt.source.cursor, 3);
    assert!(matches!(
        update.as_deref(),
        Some(ConversationUpdate::Said(_))
    ));
    assert_eq!(pools.available(), (1024 * 1024, 1024 * 1024));
    let refused = pool(&opened.bridge, 0, 1024 * 1024, false).await;
    assert!(matches!(
        opened
            .bridge
            .receive_output(&retained(4, payload), &capture)
            .await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(refused.available(), (0, 1024 * 1024));
    assert_eq!(
        opened
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(3)
    );
    assert!(effects.try_recv().is_err());
    pool(&opened.bridge, 1024 * 1024, 1024 * 1024, false).await;
    opened
        .bridge
        .receive_output(&retained(4, payload), &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, .. } = effects.try_recv().unwrap() else {
        panic!("replayed receipt missing")
    };
    assert_eq!(receipt.source.cursor, 4);
    stop(opened).await;
}

#[tokio::test]
async fn exhausted_json_reader_closes_without_ack_or_synthetic_prompt_terminal() {
    let (mut opened, mut server, mut received) = open_with_json(false, false, true).await;
    pool(&opened.bridge, 0, 1024 * 1024, false).await;
    let (capture, mut effects) = UpdateRelay::capture();
    opened
        .bridge
        .drive(ConversationInput::Prompt(vec![]), &capture)
        .await
        .unwrap();
    assert_eq!(receive_json(&mut server).await["method"], "session/prompt");
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend(retained(3, b"{}"));
    server.send(Message::Binary(bytes)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
        .await
        .unwrap()
        .unwrap();
    assert!(received.try_recv().is_err());
    drop(capture);
    assert!(tokio::time::timeout(Duration::from_secs(2), effects.recv())
        .await
        .unwrap()
        .is_none());
    while let Ok(Some(Ok(frame))) =
        tokio::time::timeout(Duration::from_millis(30), server.next()).await
    {
        if let Message::Binary(bytes) = frame {
            assert_ne!(bytes[0], LANE_OUTPUT_ACK);
        }
    }
    opened.bridge.retire().await;
    opened.relay.abort();
    let _ = opened.relay.await;
}

#[tokio::test]
async fn enabled_policy_requires_retained_ready_and_scopes_ready_admission() {
    let (client, mut server) = socket_pair().await;
    super::super::tests::send_ready(&mut server, "agent-1").await;
    let mut settings = settings(None, false);
    settings.retained_json = true;
    let (sender, _) = mpsc::channel(4);
    let result = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings,
        None,
        sender,
    )
    .await;
    assert!(
        matches!(result, Err(AdapterError::Rejected(message)) if message.contains("require retained"))
    );
    let text =
        serde_json::json!({"t":"ready","agentId":"agent-1","wireVersion":1,"lanes":["ctl","conv"]})
            .to_string();
    let pools = JsonPools::new(0, 1024 * 1024);
    assert!(matches!(
        super::super::validate_ready(Message::Text(text.clone()), "agent-1", false, Some(&pools)),
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(pools.available(), (0, 1024 * 1024));
    let pools = JsonPools::new(1024 * 1024, 1024 * 1024);
    assert!(
        super::super::validate_ready(Message::Text(text), "agent-1", false, Some(&pools)).is_ok()
    );
    assert_eq!(pools.available(), (1024 * 1024, 1024 * 1024));
}

#[tokio::test]
async fn two_attachments_share_capacity_and_teardown_allows_same_cursor_replay() {
    let (first, _first_server, _first_updates) = open_with_json(false, false, true).await;
    let (second, _second_server, _second_updates) = open_with_json(false, false, true).await;
    let sizing = JsonPools::new(1024 * 1024, 1024 * 1024);
    let sample = sizing.decode(b"{}").unwrap();
    let weight = 1024 * 1024 - sizing.available().0;
    drop(sample);
    let shared = JsonPools::new(weight, 1024 * 1024);
    {
        let mut state = first.bridge.output.as_ref().unwrap().lock().await;
        state.json_pools = Some(shared.clone());
        state.session = None;
    }
    second
        .bridge
        .output
        .as_ref()
        .unwrap()
        .lock()
        .await
        .json_pools = Some(shared.clone());
    let (capture, mut effects) = UpdateRelay::capture();
    first
        .bridge
        .receive_output(&retained(3, b"{}"), &capture)
        .await
        .unwrap();
    assert_eq!(shared.available(), (0, 1024 * 1024));
    assert!(matches!(
        second
            .bridge
            .receive_output(&retained(3, b"{}"), &capture)
            .await,
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(
        second
            .bridge
            .output
            .as_ref()
            .unwrap()
            .lock()
            .await
            .last_received,
        Some(2)
    );
    assert!(effects.try_recv().is_err());
    stop(first).await;
    assert_eq!(shared.available(), (weight, 1024 * 1024));
    second
        .bridge
        .receive_output(&retained(3, b"{}"), &capture)
        .await
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, .. } = effects.try_recv().unwrap() else {
        panic!("replay receipt missing")
    };
    assert_eq!(receipt.source.cursor, 3);
    assert_eq!(shared.available(), (weight, 1024 * 1024));
    stop(second).await;
}
