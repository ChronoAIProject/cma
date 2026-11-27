use super::*;
use crate::RuntimeOutputReceipt;

#[path = "acp_backpressure_failure_tests.rs"]
mod failure_tests;

fn stream_id() -> uuid::Uuid {
    uuid::Uuid::from_u128(17)
}

async fn retained(server: &mut ServerSocket, cursor: u64, body: serde_json::Value) {
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend_from_slice(stream_id().as_bytes());
    bytes.extend_from_slice(&cursor.to_be_bytes());
    bytes.extend(serde_json::to_vec(&body).unwrap());
    server.send(Message::Binary(bytes)).await.unwrap();
}

async fn next_update(
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
) -> (RuntimeOutputReceipt, Option<crate::RuntimeOutputUpdate>) {
    let update = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    let ConversationUpdate::RuntimeOutput { receipt, update } = update else {
        panic!("missing retained receipt")
    };
    (receipt, update)
}

async fn ack(server: &mut ServerSocket, expected: u64) {
    let message = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Binary(bytes) = message else {
        panic!("missing binary ACK")
    };
    assert_eq!(bytes[0], LANE_OUTPUT_ACK);
    assert_eq!(&bytes[1..17], stream_id().as_bytes());
    assert_eq!(
        u64::from_be_bytes(bytes[17..25].try_into().unwrap()),
        expected
    );
}

async fn opened_pair() -> (
    OpenedBridge,
    mpsc::Receiver<ConversationUpdate>,
    ServerSocket,
) {
    opened_pair_with_gate(None).await
}

async fn opened_pair_with_gate(
    gate: Option<Arc<FlushGate>>,
) -> (
    OpenedBridge,
    mpsc::Receiver<ConversationUpdate>,
    ServerSocket,
) {
    let (client, mut server) = socket_pair().await;
    let client: Socket = match gate {
        Some(gate) => Box::new(GatedSocket {
            inner: client,
            gate,
        }),
        None => client,
    };
    server
        .send(Message::Text(
            serde_json::json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent-1",
                "outputAcknowledgement":{"version":1, "streamId":stream_id()}
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let (updates, mut received) = mpsc::channel(1);
    let opening = Bridge::open_with_relay_policy(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None, (true, None)),
        None,
        None,
        updates,
    );
    let handshake = async move {
        let initialize = receive_json(&mut server).await;
        retained(
            &mut server,
            1,
            serde_json::json!({"jsonrpc":"2.0", "id":initialize["id"], "result":{}}),
        )
        .await;
        let new = receive_json(&mut server).await;
        assert_eq!(new["method"], "session/new");
        retained(
            &mut server,
            2,
            serde_json::json!({"jsonrpc":"2.0", "id":new["id"], "result":{"sessionId":"retained"}}),
        )
        .await;
        server
    };
    let (opened, mut server) = tokio::join!(opening, handshake);
    let opened = opened.unwrap();
    let progress = opened.bridge.output_progress().await.unwrap();
    assert_eq!(*progress.borrow(), 0);
    for cursor in 1..=2 {
        let (receipt, update) = next_update(&mut received).await;
        assert_eq!(receipt.source.cursor, cursor);
        assert!(update.is_none());
        receipt.acknowledge().await.unwrap();
        ack(&mut server, cursor).await;
    }
    let mut progress = opened.bridge.output_progress().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        progress.wait_for(|cursor| *cursor == 2),
    )
    .await
    .unwrap()
    .unwrap();
    (opened, received, server)
}

fn chunk() -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0", "method":"session/update", "params":{
        "sessionId":"retained", "update":{"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"visible"}}
    }})
}

#[tokio::test]
async fn full_delivery_queue_keeps_ack_pump_live_and_preserves_terminal_order() {
    let (opened, mut received, mut server) = opened_pair().await;
    for cursor in 3..=202 {
        retained(&mut server, cursor, chunk()).await;
    }
    let prompt = crate::PromptIdentity::new("retained", 19, 157);
    retained(&mut server, 203, serde_json::json!({"jsonrpc":"2.0", "id":prompt.as_str(), "result":{"stopReason":"end_turn"}})).await;
    let (receipt, _) = next_update(&mut received).await;
    assert_eq!(receipt.source.cursor, 3);
    tokio::time::timeout(Duration::from_secs(2), async {
        // More ACKs than the 128-slot acknowledgement channel can hold. Duplicate
        // durable ACKs do not release a later frame or justify output progress.
        for _ in 0..260 {
            receipt.acknowledge().await.unwrap();
        }
    })
    .await
    .expect("ACKs must drain while no further output is consumed");
    for _ in 0..260 {
        ack(&mut server, 3).await;
    }
    for cursor in 4..=203 {
        let (receipt, update) = next_update(&mut received).await;
        assert_eq!(receipt.source.cursor, cursor);
        if cursor == 203 {
            assert!(
                matches!(update.as_deref(), Some(ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }) if prompt_id == &prompt)
            );
        } else {
            assert!(matches!(
                update.as_deref(),
                Some(ConversationUpdate::WithoutPromptEvidence(_))
            ));
        }
        receipt.acknowledge().await.unwrap();
        ack(&mut server, cursor).await;
    }
    let mut progress = opened.bridge.output_progress().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        progress.wait_for(|cursor| *cursor == 203),
    )
    .await
    .unwrap()
    .unwrap();
    stop(opened).await;
}

#[tokio::test]
async fn closed_consumer_ends_attachment_without_acknowledging_queued_output() {
    let (mut opened, received, mut server) = opened_pair().await;
    let progress = opened.bridge.output_progress().await.unwrap();
    for cursor in 3..=100 {
        retained(&mut server, cursor, chunk()).await;
    }
    drop(received);
    tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*progress.borrow(), 2);
    assert!(opened.bridge.retired.load(Ordering::SeqCst));
    opened.relay.abort();
    let _ = opened.relay.await;
    opened.bridge.retire().await;
}

#[tokio::test]
async fn bounded_policy_refuses_legacy_bridge_before_initialization_or_prompt() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (updates, _received) = mpsc::channel(1);
    let result = Bridge::open_with_relay_policy(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None, (true, None)),
        None,
        None,
        updates,
    )
    .await;
    assert!(
        matches!(result, Err(AdapterError::Unavailable(message)) if message.contains("retained runtime acknowledgements"))
    );
    let next = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .unwrap();
    assert!(!matches!(next, Some(Ok(Message::Binary(_)))));
}

#[tokio::test]
async fn corrupt_retained_identity_ends_attachment_without_progress() {
    let (mut opened, _received, mut server) = opened_pair().await;
    let progress = opened.bridge.output_progress().await.unwrap();
    let mut bytes = vec![LANE_RETAINED_OUTPUT];
    bytes.extend_from_slice(uuid::Uuid::from_u128(18).as_bytes());
    bytes.extend_from_slice(&3_u64.to_be_bytes());
    bytes.extend(serde_json::to_vec(&chunk()).unwrap());
    server.send(Message::Binary(bytes)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut opened.reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*progress.borrow(), 2);
    opened.relay.abort();
    let _ = opened.relay.await;
    opened.bridge.retire().await;
}

#[tokio::test]
async fn startup_backlog_drains_while_opening_and_preserves_recovered_terminal() {
    let (client, mut server) = socket_pair().await;
    server
        .send(Message::Text(
            serde_json::json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent-1",
                "outputAcknowledgement":{"version":1, "streamId":stream_id()}
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let prompt = crate::PromptIdentity::new("retained", 19, 157);
    let expected = prompt.clone();
    let (release, held) = tokio::sync::oneshot::channel();
    let server_task = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        for cursor in 1..=300 {
            retained(&mut server, cursor, chunk()).await;
        }
        retained(&mut server, 301, serde_json::json!({"jsonrpc":"2.0", "id":expected.as_str(), "result":{"stopReason":"end_turn"}})).await;
        retained(
            &mut server,
            302,
            serde_json::json!({"jsonrpc":"2.0", "id":initialize["id"], "error":{
                "code":-32603,"message":"Internal error","data":{"details":"Already initialized"}
            }}),
        )
        .await;
        for cursor in 1..=302 {
            ack(&mut server, cursor).await;
        }
        let _ = held.await;
    });
    let (updates, mut receiver) = mpsc::channel(1);
    let opening = Bridge::open_with_relay_policy(
        client,
        "agent-1",
        "/workspace".into(),
        (settings(None, false), None, (true, None)),
        Some("retained".into()),
        Some(prompt.clone()),
        updates,
    );
    tokio::pin!(opening);
    let mut opened = None;
    let mut expected = 1;
    tokio::time::timeout(Duration::from_secs(3), async {
        while opened.is_none() || expected <= 302 {
            tokio::select! {
                result = &mut opening, if opened.is_none() => { opened = Some(result.unwrap()); },
                (receipt, update) = next_update(&mut receiver), if expected <= 302 => {
                    assert_eq!(receipt.source.cursor, expected);
                    if expected == 301 {
                        assert!(matches!(update.as_deref(), Some(ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }) if prompt_id == &prompt));
                    }
                    receipt.acknowledge().await.unwrap();
                    expected += 1;
                }
            }
        }
    }).await.expect("bounded startup must progress with its durable consumer");
    release.send(()).unwrap();
    server_task.await.unwrap();
    stop(opened.unwrap()).await;
}

#[derive(Default)]
struct FlushGate {
    blocked: AtomicBool,
    entered: tokio::sync::Notify,
    waker: futures_util::task::AtomicWaker,
}

struct GatedSocket {
    inner: Socket,
    gate: Arc<FlushGate>,
}

impl futures_util::Stream for GatedSocket {
    type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.inner).poll_next(cx)
    }
}
impl futures_util::Sink<Message> for GatedSocket {
    type Error = tokio_tungstenite::tungstenite::Error;
    fn poll_ready(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_ready(cx)
    }
    fn start_send(mut self: std::pin::Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        std::pin::Pin::new(&mut self.inner).start_send(item)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.gate.waker.register(cx.waker());
        if self.gate.blocked.load(Ordering::SeqCst) {
            self.gate.entered.notify_one();
            return std::task::Poll::Pending;
        }
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_close(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[tokio::test]
async fn ack_backlog_cannot_suspend_dispatch_that_owns_the_socket_writer() {
    let gate = Arc::new(FlushGate::default());
    let (opened, mut received, mut server) = opened_pair_with_gate(Some(gate.clone())).await;
    retained(&mut server, 3, chunk()).await;
    let (receipt, _) = next_update(&mut received).await;
    gate.blocked.store(true, Ordering::SeqCst);
    retained(
        &mut server,
        4,
        serde_json::json!({"jsonrpc":"2.0", "id":987, "method":"unsupported/fixture", "params":{}}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(2), gate.entered.notified())
        .await
        .unwrap();
    let full = Arc::new(tokio::sync::Notify::new());
    let signal = full.clone();
    let sender = tokio::spawn(async move {
        for index in 0..130 {
            receipt.acknowledge().await.unwrap();
            if index == 128 {
                signal.notify_one();
            }
        }
    });
    // One ACK is waiting on the writer and 128 occupy its channel. The dispatch
    // future owns that writer, so it must still be polled after flush readiness.
    tokio::time::timeout(Duration::from_secs(2), full.notified())
        .await
        .unwrap();
    gate.blocked.store(false, Ordering::SeqCst);
    gate.waker.wake();
    tokio::time::timeout(Duration::from_secs(2), sender)
        .await
        .unwrap()
        .unwrap();
    let reply = receive_json(&mut server).await;
    assert_eq!(reply["id"], 987);
    for _ in 0..130 {
        ack(&mut server, 3).await;
    }
    let (receipt, update) = next_update(&mut received).await;
    assert_eq!(receipt.source.cursor, 4);
    assert!(update.is_none());
    receipt.acknowledge().await.unwrap();
    ack(&mut server, 4).await;
    stop(opened).await;
}
