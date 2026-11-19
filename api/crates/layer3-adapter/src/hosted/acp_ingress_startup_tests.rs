use super::{identity::PendingCalls, tests::fixture};
use crate::hosted::retained_json::JsonPools;
use crate::ingress_budget::{AllocationPlan, IngressClass};

#[test]
fn class_bounds_derive_progress_floors_from_attachment_and_layout_limits() {
    let empty = AllocationPlan::new("no downstream work in this fixture", &[]).unwrap();
    for attachments in [64, 200] {
        let bounds = crate::ingress_budget::IngressOwnershipPolicy::conversation_bounds(
            128 * 1024 * 1024,
            attachments,
            empty,
            empty,
        )
        .unwrap();
        let typed = super::typed::maximum_retained_bytes().unwrap();
        // Raw maximum plus JSON's existing wire/node formula and the generic
        // typed layout must fit together, before consumer work is added.
        let raw_node =
            size_of::<crate::hosted::acp::output::HeldOutput>() + 2 * size_of::<usize>() + 128;
        let delivery = 1024
            + 36
            + 3 * 64
            + crate::ingress_budget::sizing::shared_owner_bytes::<crate::ConversationUpdate>()
            + 2 * size_of::<crate::ConversationUpdate>()
            + 2 * size_of::<usize>();
        let raw_json =
            4 * 1024 * 1024 + raw_node + 2 * 4 * 1024 * 1024 + 8192 * 1024 + 1024 + delivery;
        let incoming = 4 * 1024 * 1024
            + 25
            + crate::ingress_budget::sizing::shared_owner_bytes::<super::Message>();
        assert!(
            typed + raw_json + incoming < bounds.data_bytes(),
            "{attachments} attachments: typed={typed}, raw_json={raw_json}, data={}",
            bounds.data_bytes()
        );
        assert!(bounds.transformation_bytes() >= 13 * 1024 * 1024);
        eprintln!("attachments={attachments} control_floor={} transformation_floor={} data_ceiling={} maximum_typed_retained={typed}", bounds.control_bytes(), bounds.transformation_bytes(), bounds.data_bytes());
    }
    for attachments in [0, usize::MAX, 4096] {
        assert!(
            crate::ingress_budget::IngressOwnershipPolicy::conversation_bounds(
                128 * 1024 * 1024,
                attachments,
                empty,
                empty,
            )
            .is_err()
        );
    }
    let too_large = AllocationPlan::new(
        "unsupported downstream transformation",
        &[128 * 1024 * 1024],
    )
    .unwrap();
    assert!(
        crate::ingress_budget::IngressOwnershipPolicy::conversation_bounds(
            128 * 1024 * 1024,
            64,
            empty,
            too_large,
        )
        .is_err()
    );
}

#[tokio::test]
async fn rpc_channel_custody_survives_removal_and_returned_json_survives_caller() {
    let fixture = fixture();
    let baseline = fixture.root.available_bytes();
    let mut calls = PendingCalls::new(Some(&fixture.policy)).unwrap();
    let reply = calls.register("retained-call", false).unwrap();
    let waiting = calls.remove("retained-call").unwrap();
    drop(calls);
    let channel_held = fixture.root.available_bytes();
    assert!(channel_held < baseline);
    let pools = JsonPools::new(1024 * 1024, 1024 * 1024).with_ownership(&fixture.policy);
    let payload =
        serde_json::to_vec(&serde_json::json!({"result":{"text":"x".repeat(65536)}})).unwrap();
    let frame = pools.decode(&payload).unwrap();
    waiting.answer.send(frame).unwrap();
    drop(waiting._storage);
    assert!(fixture.root.available_bytes() < channel_held);
    let result = reply.await.unwrap().into_result();
    assert_eq!(result["text"].as_str().unwrap().len(), 65536);
    let clone = result.clone();
    drop(result);
    assert!(fixture.root.available_bytes() < baseline - 65536);
    drop(clone);
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[tokio::test]
async fn bounded_pending_calls_refuse_without_evicting_original_waiters() {
    let fixture = fixture();
    let baseline = fixture.root.available_bytes();
    let mut calls = PendingCalls::new(Some(&fixture.policy)).unwrap();
    let mut replies = Vec::with_capacity(128);
    for n in 0..128 {
        replies.push(calls.register(&format!("rpc-{n}"), false).unwrap());
    }
    let full = fixture.root.available_bytes();
    assert!(calls.register("overflow", false).is_err());
    assert!(calls.register("rpc-0", true).is_err());
    assert_eq!(fixture.root.available_bytes(), full);
    assert!(!calls.get("rpc-0").unwrap().prompt);
    calls.clear();
    assert!(calls.is_empty());
    drop(calls);
    assert!(fixture.root.available_bytes() < baseline);
    drop(replies);
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[tokio::test]
async fn call_admission_refusal_does_not_create_a_channel_or_change_existing_calls() {
    let fixture = fixture();
    let mut calls = PendingCalls::new(Some(&fixture.policy)).unwrap();
    let waiting = calls.register("existing", false).unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let blocker = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("RPC saturation", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    assert!(calls.register("new", false).is_err());
    assert!(calls.get("new").is_none());
    assert!(calls.get("existing").is_some());
    drop((blocker, waiting));
}

#[test]
fn legacy_pending_calls_keep_their_existing_unbounded_replacement_behavior() {
    let mut calls = PendingCalls::new(None).unwrap();
    let mut replies = Vec::new();
    for n in 0..256 {
        replies.push(calls.register(&format!("rpc-{n}"), false).unwrap());
    }
    let replacement = calls.register("rpc-0", true).unwrap();
    assert!(calls.get("rpc-0").unwrap().prompt);
    drop((replacement, replies));
}

struct ScriptedSocket {
    incoming:
        tokio::sync::mpsc::UnboundedReceiver<crate::ingress_budget::SharedOwned<super::Message>>,
    outgoing:
        tokio::sync::mpsc::UnboundedSender<crate::ingress_budget::SharedOwned<super::Message>>,
}

impl futures_util::Stream for ScriptedSocket {
    type Item = crate::Result<crate::ingress_budget::SharedOwned<super::Message>>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.incoming.poll_recv(cx).map(|frame| frame.map(Ok))
    }
}

impl futures_util::Sink<crate::ingress_budget::SharedOwned<super::Message>> for ScriptedSocket {
    type Error = crate::AdapterError;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<crate::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn start_send(
        self: std::pin::Pin<&mut Self>,
        frame: crate::ingress_budget::SharedOwned<super::Message>,
    ) -> crate::Result<()> {
        self.outgoing
            .send(frame)
            .map_err(|_| crate::AdapterError::Unavailable("scripted socket closed".into()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<crate::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<crate::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

fn admitted_message(
    policy: &crate::ingress_budget::IngressOwnershipPolicy,
    value: &serde_json::Value,
    cursor: Option<u64>,
) -> crate::ingress_budget::SharedOwned<super::Message> {
    let mut length = super::EncodedLength(0);
    serde_json::to_writer(&mut length, value).unwrap();
    policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new(
                "synthetic admitted producer",
                &[
                    length.0 + 25,
                    crate::ingress_budget::sizing::shared_owner_bytes::<super::Message>(),
                ],
            )
            .unwrap(),
        )
        .unwrap()
        .initialize(|| {
            let mut bytes = Vec::with_capacity(length.0 + 25);
            if let Some(cursor) = cursor {
                bytes.push(4);
                bytes.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
                bytes.extend_from_slice(&cursor.to_be_bytes());
            }
            serde_json::to_writer(&mut bytes, value).unwrap();
            match cursor {
                Some(_) => super::Message::Binary(bytes),
                None => super::Message::Text(String::from_utf8(bytes).unwrap()),
            }
        })
}

#[tokio::test]
async fn admitted_socket_refuses_a_frame_charged_to_another_process_root() {
    use futures_util::StreamExt;
    let expected = fixture();
    let foreign = fixture();
    let before = foreign.root.available_bytes();
    let (incoming_tx, incoming) = tokio::sync::mpsc::unbounded_channel();
    let (outgoing, _) = tokio::sync::mpsc::unbounded_channel();
    incoming_tx
        .send(admitted_message(
            &foreign.policy,
            &serde_json::json!({"t":"ready"}),
            None,
        ))
        .unwrap();
    let mut socket = super::admitted(
        Box::new(ScriptedSocket { incoming, outgoing }),
        expected.policy,
    );
    assert!(socket.next().await.unwrap().is_err());
    assert_eq!(foreign.root.available_bytes(), before);
}

#[tokio::test]
async fn admitted_open_preserves_ready_startup_order_and_commit_before_ack() {
    use crate::ConversationUpdate;
    use serde_json::json;
    let fixture = fixture();
    let policy = fixture.policy.clone();
    let baseline = fixture.root.available_bytes();
    let (to_client, incoming) = tokio::sync::mpsc::unbounded_channel();
    let (outgoing, mut from_client) = tokio::sync::mpsc::unbounded_channel::<
        crate::ingress_budget::SharedOwned<super::Message>,
    >();
    to_client
        .send(admitted_message(
            &policy,
            &json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent",
                "outputAcknowledgement":{"version":1, "streamId":uuid::Uuid::from_u128(1)}
            }),
            None,
        ))
        .unwrap();
    let (ack_seen, mut acknowledgements) = tokio::sync::mpsc::unbounded_channel();
    let server_policy = policy.clone();
    let server = tokio::spawn(async move {
        while let Some(frame) = from_client.recv().await {
            let super::Message::Binary(bytes) = &*frame else {
                panic!("expected conversation frame")
            };
            if bytes[0] == 5 {
                ack_seen
                    .send(u64::from_be_bytes(bytes[17..25].try_into().unwrap()))
                    .unwrap();
                continue;
            }
            assert_eq!(bytes[0], 2);
            let request: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
            let (cursor, result) = match request["method"].as_str().unwrap() {
                "initialize" => (1, json!({"protocolVersion":1, "agentCapabilities":{}})),
                "session/new" => {
                    to_client.send(admitted_message(&server_policy, &json!({"method":"session/update", "params":{
                        "sessionId":"conversation", "update":{"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"startup"}}
                    }}), Some(2))).unwrap();
                    (3, json!({"sessionId":"conversation"}))
                }
                method => panic!("unexpected method {method}"),
            };
            to_client
                .send(admitted_message(
                    &server_policy,
                    &json!({"id":request["id"], "result":result}),
                    Some(cursor),
                ))
                .unwrap();
        }
    });
    let mut settings = super::super::tests::settings(None, false);
    settings.retained_json = true;
    settings.bounded_raw_output = true;
    settings.bounded_permission_state = true;
    settings.output_json_budget = true;
    settings.shared_runtime_output = true;
    let (updates, mut received) = tokio::sync::mpsc::channel(8);
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        super::super::Bridge::open_admitted(
            Box::new(ScriptedSocket { incoming, outgoing }),
            "agent",
            "/synthetic/work".into(),
            (settings, None, policy),
            None,
            None,
            updates,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation")
    );
    assert!(acknowledgements.try_recv().is_err());
    for cursor in 1..=3 {
        let update = tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
            .await
            .unwrap()
            .unwrap();
        let delivery = update.delivery();
        let receipt = delivery.receipt.unwrap();
        assert_eq!(receipt.source.cursor, cursor);
        assert_eq!(receipt.source.native_conversation_id, "conversation");
        if cursor == 2 {
            assert!(
                matches!(delivery.effect, Some(ConversationUpdate::WithoutPromptEvidence(activity))
                if matches!(activity.effect(), ConversationUpdate::Said(_)))
            );
        } else {
            assert!(delivery.effect.is_none());
        }
        assert!(acknowledgements.try_recv().is_err());
        receipt.acknowledge().await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), acknowledgements.recv())
                .await
                .unwrap(),
            Some(cursor)
        );
    }
    super::super::tests::stop(opened).await;
    server.abort();
    let _ = server.await;
    drop(received);
    assert_eq!(fixture.root.available_bytes(), baseline);
}

#[tokio::test]
async fn independent_legacy_flags_preserve_rpc_errors_receipts_and_ack_order() {
    use super::super::{tests as legacy, Bridge};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    let mut combinations = 0;
    for flags in 0..128 {
        if flags & 1 != 0 && flags & 14 != 14 {
            continue;
        }
        let (client, mut server) = legacy::socket_pair().await;
        let mut settings = legacy::settings(None, false);
        settings.coordinated_ingress = flags & 1 != 0;
        settings.retained_json = flags & 2 != 0;
        settings.bounded_raw_output = flags & 4 != 0;
        settings.bounded_permission_state = flags & 8 != 0;
        settings.output_json_budget = flags & 16 != 0;
        settings.shared_runtime_output = flags & 32 != 0;
        let (updates, mut received) = tokio::sync::mpsc::channel(8);
        let peer =
            async move {
                server.send(super::Message::Text(json!({
                "t":"ready", "wireVersion":1, "lanes":["ctl","conv"], "agentId":"agent",
                "outputAcknowledgement":{"version":1, "streamId":uuid::Uuid::from_u128(1)}
            }).to_string())).await.unwrap();
                for (cursor, result) in [
                    (1, json!({"protocolVersion":1, "agentCapabilities":{}})),
                    (2, json!({"sessionId":"conversation"})),
                ] {
                    let request = legacy::receive_json(&mut server).await;
                    send_legacy_retained(
                        &mut server,
                        cursor,
                        json!({"id":request["id"], "result":result}),
                    )
                    .await;
                }
                server
            };
        let (opened, mut server) = tokio::join!(
            Bridge::open_with_relay_policy(
                client,
                "agent",
                "/synthetic/work".into(),
                (settings, None, flags & 64 != 0),
                None,
                None,
                updates,
            ),
            peer
        );
        let opened = opened.unwrap();
        assert!(opened.bridge.ingress.is_none(), "flags={flags}");
        assert_eq!(
            opened.channel.conversation_id.as_deref(),
            Some("conversation")
        );
        let bridge = opened.bridge.clone();
        let call = tokio::spawn(async move { bridge.call("fixture/error", json!({})).await });
        let request = legacy::receive_json(&mut server).await;
        send_legacy_retained(
            &mut server,
            3,
            json!({"id":request["id"], "error":{
                "code":-32000, "message":"synthetic refusal"
            }}),
        )
        .await;
        assert!(
            matches!(call.await.unwrap(), Err(crate::AdapterError::Rejected(message))
            if message == "fixture/error: {\"code\":-32000,\"message\":\"synthetic refusal\"}")
        );
        for cursor in 1..=3 {
            let update = received.recv().await.unwrap();
            let delivery = update.delivery();
            let receipt = delivery.receipt.unwrap();
            assert_eq!(receipt.source.cursor, cursor);
            assert_eq!(receipt.source.native_conversation_id, "conversation");
            assert!(delivery.effect.is_none());
            receipt.acknowledge().await.unwrap();
            let super::Message::Binary(ack) = server.next().await.unwrap().unwrap() else {
                panic!("missing ACK")
            };
            assert_eq!(ack[0], 5);
            assert_eq!(u64::from_be_bytes(ack[17..25].try_into().unwrap()), cursor);
        }
        legacy::stop(opened).await;
        combinations += 1;
    }
    assert_eq!(combinations, 72);
}

async fn send_legacy_retained(
    server: &mut super::super::tests::ServerSocket,
    cursor: u64,
    value: serde_json::Value,
) {
    use futures_util::SinkExt;
    let mut wire = vec![4];
    wire.extend_from_slice(uuid::Uuid::from_u128(1).as_bytes());
    wire.extend_from_slice(&cursor.to_be_bytes());
    wire.extend_from_slice(&serde_json::to_vec(&value).unwrap());
    server.send(super::Message::Binary(wire)).await.unwrap();
}
