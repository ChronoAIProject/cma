use super::tests::fixture;
use crate::hosted::update_relay::UpdateRelay;
use crate::ingress_budget::{AllocationPlan, IngressClass};
use crate::{ConversationUpdate, PermissionDecision};
use serde_json::json;

#[tokio::test]
async fn permission_payload_and_locator_remain_owned_after_raw_and_json_drop() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let before = fixture.root.available_bytes();
    let value = json!({"id":"permission", "method":"session/request_permission", "params":{
        "sessionId":"conversation", "toolCall":{"toolCallId":"tool", "title":"t".repeat(65536)},
        "options":[{"optionId":"yes", "name":"Allow", "kind":"allow_once"}]
    }});
    fixture
        .bridge
        .receive_output(&fixture.retained(&value, 1), &relay)
        .await
        .unwrap();
    let delivery = captured.take().unwrap().unwrap();
    let ConversationUpdate::WithoutPromptEvidence(activity) = delivery.delivery().effect.unwrap()
    else {
        panic!("passive permission wrapper missing")
    };
    let ConversationUpdate::Asked(permission) = activity.effect() else {
        panic!("permission missing")
    };
    assert_eq!(
        permission
            .request
            .tool_call
            .fields
            .title
            .as_ref()
            .unwrap()
            .len(),
        65536
    );
    assert!(permission.locator.is_some());
    let entry = fixture
        .bridge
        .permission_state
        .as_ref()
        .unwrap()
        .lock()
        .await
        .get(&permission.id)
        .unwrap();
    let clone = delivery.clone();
    drop(delivery);
    assert!(fixture.root.available_bytes() < before - 65536);
    drop(clone);
    // The compact replay entry outlives the delivered tool body.
    assert!(fixture.root.available_bytes() > before - 65536);
    entry.select(PermissionDecision::Allow);
    let answer = entry.admitted_answer(&fixture.policy).unwrap().unwrap();
    assert_eq!(
        answer
            .pointer("/result/outcome/optionId")
            .and_then(serde_json::Value::as_str),
        Some("yes")
    );
}

#[tokio::test]
async fn selected_permission_survives_data_pressure_and_replay_table_overflow() {
    let fixture = fixture();
    let options = vec![crate::PermissionOption::new(
        "yes",
        "Allow",
        crate::PermissionOptionKind::AllowOnce,
    )];
    let state = fixture.bridge.permission_state.as_ref().unwrap();
    let entry = state
        .lock()
        .await
        .remember(&format!("{:064x}", 1), &json!("request"), &options, false)
        .unwrap();
    entry.select(PermissionDecision::Allow);
    for index in 2..=crate::hosted::permission_state::MAX_ENTRIES {
        state
            .lock()
            .await
            .remember(&format!("{index:064x}"), &json!(index), &options, false)
            .unwrap();
    }
    assert!(state
        .lock()
        .await
        .remember(&format!("{:064x}", 1000), &json!("new"), &options, false)
        .is_err());
    let data = fixture.policy.budget(IngressClass::Data);
    let available = data.available_bytes();
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("permission pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    let first = entry.admitted_answer(&fixture.policy).unwrap().unwrap();
    entry.select(PermissionDecision::Deny);
    entry.select(PermissionDecision::Abort);
    let replay = state.lock().await.get(&format!("{:064x}", 1)).unwrap();
    let answer = replay.admitted_answer(&fixture.policy).unwrap().unwrap();
    assert_eq!(*first, *answer);
    assert_eq!(
        answer
            .pointer("/result/outcome/optionId")
            .and_then(serde_json::Value::as_str),
        Some("yes")
    );
    assert_eq!(data.available_bytes(), 0);
    fixture
        .bridge
        .bounded_permission_answer(
            &format!("{:064x}", 1),
            None,
            "conversation",
            PermissionDecision::Deny,
        )
        .await
        .unwrap();
    let writes = fixture.writes.lock().unwrap();
    let super::Message::Binary(bytes) = &*writes[0] else {
        panic!("expected permission reply")
    };
    let sent: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
    assert_eq!(sent, *first);
    drop(writes);
    drop((answer, first, held));
}

struct StalledSocket(Option<crate::ingress_budget::SharedOwned<super::Message>>);

impl futures_util::Stream for StalledSocket {
    type Item = crate::Result<crate::ingress_budget::SharedOwned<super::Message>>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Pending
    }
}

impl futures_util::Sink<crate::ingress_budget::SharedOwned<super::Message>> for StalledSocket {
    type Error = crate::AdapterError;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<crate::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn start_send(
        mut self: std::pin::Pin<&mut Self>,
        frame: crate::ingress_budget::SharedOwned<super::Message>,
    ) -> crate::Result<()> {
        assert!(self.0.is_none());
        self.0 = Some(frame);
        Ok(())
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<crate::Result<()>> {
        std::task::Poll::Pending
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<crate::Result<()>> {
        std::task::Poll::Pending
    }
}

#[tokio::test]
async fn cancelled_permission_write_keeps_first_answer_and_socket_storage_until_drop() {
    use futures_util::StreamExt;
    let mut fixture = fixture();
    let id = format!("{:064x}", 1);
    let options = vec![crate::PermissionOption::new(
        "yes",
        "Allow",
        crate::PermissionOptionKind::AllowOnce,
    )];
    fixture
        .bridge
        .permission_state
        .as_ref()
        .unwrap()
        .lock()
        .await
        .remember(&id, &json!("request"), &options, false)
        .unwrap();
    let (sink, _) = super::admitted(Box::new(StalledSocket(None)), fixture.policy.clone()).split();
    let original = std::mem::replace(
        &mut fixture.bridge.sink,
        std::sync::Arc::new(super::super::Mutex::new(sink, Some(&fixture.policy)).unwrap()),
    );
    let control = fixture.policy.budget(IngressClass::Control);
    let before = control.available_bytes();
    {
        let answer = fixture.bridge.bounded_permission_answer(
            &id,
            None,
            "conversation",
            PermissionDecision::Allow,
        );
        tokio::pin!(answer);
        assert!(futures_util::poll!(&mut answer).is_pending());
    }
    assert!(
        control.available_bytes() < before,
        "the socket still owns the queued reply"
    );
    fixture.bridge.sink = original;
    assert_eq!(control.available_bytes(), before);
    fixture
        .bridge
        .bounded_permission_answer(&id, None, "conversation", PermissionDecision::Deny)
        .await
        .unwrap();
    let writes = fixture.writes.lock().unwrap();
    let super::Message::Binary(bytes) = &*writes[0] else {
        panic!("expected permission reply")
    };
    let sent: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
    assert_eq!(
        sent.pointer("/result/outcome/optionId")
            .and_then(serde_json::Value::as_str),
        Some("yes")
    );
}

#[tokio::test]
async fn malformed_permission_decodes_under_admission_without_creating_an_approval() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let value = json!({"id":"permission", "method":"session/request_permission", "params":{
        "sessionId":"conversation", "toolCall":{"toolCallId":false, "title":"title"}, "options":[]
    }});
    fixture
        .bridge
        .receive_output(&fixture.retained(&value, 1), &relay)
        .await
        .unwrap();
    let delivery = captured.take().unwrap().unwrap();
    assert!(delivery.delivery().effect.is_none());
    assert!(fixture.bridge.blocked.lock().await.is_empty());
    assert!(fixture.bridge.answered.lock().await.is_empty());
    assert!(fixture.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn permission_capacity_refusal_progresses_after_data_fills_without_accepting_cursor() {
    let fixture = fixture();
    fixture.activate().await;
    let options = vec![crate::PermissionOption::new(
        "yes",
        "Allow",
        crate::PermissionOptionKind::AllowOnce,
    )];
    let state = fixture.bridge.permission_state.as_ref().unwrap();
    for index in 0..crate::hosted::permission_state::MAX_ENTRIES {
        state
            .lock()
            .await
            .remember(&format!("{index:064x}"), &json!(index), &options, false)
            .unwrap();
    }
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let wire = fixture.retained(
        &json!({"id":"overflow", "method":"session/request_permission", "params":{
            "sessionId":"conversation", "toolCall":{"toolCallId":"tool", "title":"new"},
            "options":[{"optionId":"yes", "name":"Allow", "kind":"allow_once"}]
        }}),
        1,
    );
    let gate = state.lock().await;
    let attempt = fixture.bridge.receive_output(&wire, &relay);
    tokio::pin!(attempt);
    assert!(futures_util::poll!(&mut attempt).is_pending());
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("refusal pressure", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    drop(gate);
    let error = tokio::time::timeout(std::time::Duration::from_secs(1), &mut attempt)
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error, crate::AdapterError::Unavailable(message) if message.contains("permission retention capacity"))
    );
    assert!(captured.take().unwrap().is_none());
    {
        let writes = fixture.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        let super::Message::Binary(bytes) = &*writes[0] else {
            panic!("expected refusal")
        };
        assert_eq!(bytes[0], 2);
        let refusal: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
        assert_eq!(refusal["id"], "overflow");
        assert_eq!(refusal["error"]["code"], -32002);
    }
    drop(held);
    // An exact replay reaches the same capacity refusal, rather than a cursor
    // rejection or an eviction of any selected answer.
    let error = fixture
        .bridge
        .receive_output(&wire, &relay)
        .await
        .unwrap_err();
    assert!(
        matches!(error, crate::AdapterError::Unavailable(message) if message.contains("permission retention capacity"))
    );
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.bridge.blocked.lock().await.is_empty());
    assert!(fixture.bridge.answered.lock().await.is_empty());
}
#[tokio::test]
async fn completion_receipt_keeps_typed_storage_after_rpc_json_drops() {
    use crate::hosted::acp_open_completion::HostedOpenCompletionProtocol;
    for valid in [true, false] {
        let fixture = fixture();
        let before = fixture.root.available_bytes();
        let server = async {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if let Some(frame) = fixture.writes.lock().unwrap().pop() {
                        break frame;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let super::Message::Binary(bytes) = &*frame else {
                panic!("expected RPC")
            };
            let request: serde_json::Value = serde_json::from_slice(&bytes[1..]).unwrap();
            let reply = serde_json::json!({"id":request["id"], "result":{
                "complete":true, "version":1, "sessionId":"conversation", "challenge":request["params"]["challenge"],
                "bindingDigest":"synthetic-digest", "processNonce":"00000000-0000-4000-8000-000000000001",
                "openGeneration":"00000000-0000-4000-8000-000000000002", "sessionGeneration":if valid { 1 } else { u64::MAX }
            }});
            let reply = fixture
                .bridge
                .json_pools
                .as_ref()
                .unwrap()
                .decode(&serde_json::to_vec(&reply).unwrap())
                .unwrap();
            fixture
                .bridge
                .pending
                .lock()
                .await
                .remove(request["id"].as_str().unwrap())
                .unwrap()
                .answer
                .send(reply)
                .unwrap();
        };
        let (receipt, ()) = tokio::join!(
            fixture.bridge.read_open_completion(
                HostedOpenCompletionProtocol::V1,
                "conversation",
                "synthetic-digest",
            ),
            server
        );
        assert_eq!(receipt.is_ok(), valid);
        if valid {
            assert!(fixture.root.available_bytes() < before);
        }
        drop(receipt);
        assert_eq!(fixture.root.available_bytes(), before);
    }
}
