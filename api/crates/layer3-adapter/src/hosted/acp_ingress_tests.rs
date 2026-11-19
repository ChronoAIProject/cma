use super::*;
use crate::hosted::{retained_json::JsonPools, update_relay::UpdateRelay};
use crate::ingress_budget::{AllocationPlan, IngressBudget, IngressClass, IngressOwnershipPolicy};
use crate::ConversationUpdate;
use futures_util::StreamExt;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize},
    Arc, Mutex as StdMutex,
};
use tokio::sync::Mutex;

type Writes = Arc<StdMutex<Vec<SharedOwned<Message>>>>;

struct SyntheticSocket(Writes);

impl Stream for SyntheticSocket {
    type Item = crate::Result<SharedOwned<Message>>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

impl Sink<SharedOwned<Message>> for SyntheticSocket {
    type Error = crate::AdapterError;
    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn start_send(self: Pin<&mut Self>, frame: SharedOwned<Message>) -> Result<(), Self::Error> {
        self.0.lock().unwrap().push(frame);
        Ok(())
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

pub(super) struct Fixture {
    pub(super) bridge: super::super::Bridge,
    pub(super) policy: IngressOwnershipPolicy,
    pub(super) root: IngressBudget,
    pub(super) stream: uuid::Uuid,
    pub(super) acknowledgements: crate::runtime_output::AdmittedAckReceiver,
    pub(super) writes: Writes,
}

pub(super) fn fixture() -> Fixture {
    use super::super::{
        output::ReliableOutput, recovery::PromptRecovery, Bridge, ConversationState,
    };
    let root = IngressBudget::new(128 * 1024 * 1024);
    let plan = |bytes| AllocationPlan::new("bridge fixture", &[bytes]).unwrap();
    let bounds =
        IngressOwnershipPolicy::conversation_bounds(root.capacity_bytes(), 200, plan(0), plan(0))
            .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    let data = policy.budget(IngressClass::Data);
    let pools = JsonPools::new(32 * 1024 * 1024, 13 * 1024 * 1024).with_ownership(&policy);
    let stream = uuid::Uuid::from_u128(1);
    let writes = Arc::new(StdMutex::new(Vec::with_capacity(16)));
    let (sink, _) = admitted(Box::new(SyntheticSocket(writes.clone())), policy.clone()).split();
    let (sender, acknowledgements) =
        crate::RuntimeOutputReceipt::admitted_channel(policy.clone(), 128).unwrap();
    let bridge = Bridge {
        ingress: Some(policy.clone()),
        authority: None,
        json_pools: Some(pools.clone()),
        bounded_raw_output: true,
        output_json_budget: true,
        shared_runtime_output: true,
        permission_state: Some(Mutex::new(
            crate::hosted::permission_state::PermissionState::new(Some(data.clone())).unwrap(),
        )),
        agent_correlation_digest: "fixture".into(),
        subscription: None,
        sink: Arc::new(Mutex::new(sink)),
        pending: Arc::new(Mutex::new(
            super::identity::PendingCalls::new(Some(&policy)).unwrap(),
        )),
        request_namespace: "fixture".into(),
        next_id: AtomicI64::new(1),
        recovery: Mutex::new(PromptRecovery::new(None, Some(&policy)).unwrap()),
        conversation: Mutex::new(ConversationState::default()),
        output: Some(Mutex::new(ReliableOutput::new(
            stream,
            AckSender::Admitted(sender),
            true,
            Some(pools),
            Some(data),
        ))),
        blocked: Mutex::new(HashMap::new()),
        answered: Arc::new(Mutex::new(HashMap::new())),
        owned_prompt_waiters: Arc::new(AtomicUsize::new(0)),
        orphaned_prompt_response: AtomicBool::new(false),
        retired: AtomicBool::new(false),
        instructions_support: AtomicU8::new(0),
        context_refresh: None,
        context_goal: Mutex::new(None),
        recovering_context: AtomicU64::new(0),
        retained_history_deadline: tokio::sync::watch::channel(None).0,
    };
    Fixture {
        bridge,
        policy,
        root,
        stream,
        acknowledgements,
        writes,
    }
}

impl Fixture {
    pub(super) async fn activate(&self) {
        let (relay, captured) = UpdateRelay::capture_admitted(&self.policy).unwrap();
        self.bridge
            .activate_session("conversation".into(), &relay)
            .await
            .unwrap();
        self.bridge
            .activate_output("conversation", &relay)
            .await
            .unwrap();
        assert!(captured.take().unwrap().is_none());
    }

    pub(super) fn retained(&self, value: &serde_json::Value, cursor: u64) -> Vec<u8> {
        let mut wire = self.stream.as_bytes().to_vec();
        wire.extend_from_slice(&cursor.to_be_bytes());
        wire.extend_from_slice(&serde_json::to_vec(value).unwrap());
        wire
    }
}

#[tokio::test]
async fn typed_pressure_refuses_before_cursor_effect_or_ack() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let value = json!({"method":"session/update", "params":{"sessionId":"conversation", "update":{
        "sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"x".repeat(256 * 1024)}
    }}});
    let wire = fixture.retained(&value, 1);
    let pools = fixture.bridge.json_pools.as_ref().unwrap();
    let before = fixture.root.available_bytes();
    let parsed = pools.decode(&wire[24..]).unwrap();
    let json_bytes = before - fixture.root.available_bytes();
    drop(parsed);
    let data = fixture.policy.budget(IngressClass::Data);
    // Leave space for the actual raw copy/node and JSON, but less than the
    // independent typed text. Retrying cursor 1 below proves no acceptance.
    let held_bytes = data.available_bytes() - json_bytes - wire.len() - 16 * 1024;
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("pressure payload", &[held_bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; held_bytes - 512]);
    let failure = fixture
        .bridge
        .receive_output(&wire, &relay)
        .await
        .unwrap_err();
    assert!(matches!(failure, crate::AdapterError::AtCapacity(_)));
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.writes.lock().unwrap().is_empty());
    drop(held);
    fixture.bridge.receive_output(&wire, &relay).await.unwrap();
    let delivery = captured.take().unwrap().unwrap();
    assert_eq!(delivery.delivery().receipt.unwrap().source.cursor, 1);
    let Some(ConversationUpdate::WithoutPromptEvidence(activity)) = delivery.delivery().effect
    else {
        panic!("passive delivery lost its activity wrapper");
    };
    assert!(matches!(activity.effect(), ConversationUpdate::Said(_)));
    assert!(delivery.turn_evidence().is_none());
}

#[tokio::test]
async fn complete_delivery_clones_share_receipt_and_hold_storage_until_last_drop() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let before = fixture.root.available_bytes();
    let wire = fixture.retained(&json!({"method":"session/update", "params":{
        "sessionId":"conversation", "update":{"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"retained"}}
    }}), 1);
    fixture.bridge.receive_output(&wire, &relay).await.unwrap();
    let delivery = captured.take().unwrap().unwrap();
    let used = fixture.root.available_bytes();
    assert!(used < before);
    let clone = delivery.clone();
    assert!(std::ptr::eq(
        delivery.delivery().receipt.unwrap(),
        clone.delivery().receipt.unwrap()
    ));
    assert_eq!(fixture.root.available_bytes(), used);
    drop(delivery);
    assert_eq!(fixture.root.available_bytes(), used);
    drop(clone);
    assert_eq!(fixture.root.available_bytes(), before);
}

#[tokio::test]
async fn committed_ack_progresses_while_data_is_exhausted() {
    let mut fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let wire = fixture.retained(&json!({"method":"session/update", "params":{
        "sessionId":"conversation", "update":{"sessionUpdate":"agent_message_chunk", "content":{"type":"text", "text":"committed"}}
    }}), 1);
    fixture.bridge.receive_output(&wire, &relay).await.unwrap();
    let delivery = captured.take().unwrap().unwrap();
    let available = fixture.policy.budget(IngressClass::Data).available_bytes();
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("saturated data", &[available]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; available - 512]);
    delivery
        .delivery()
        .receipt
        .unwrap()
        .acknowledge()
        .await
        .unwrap();
    let ack = fixture.acknowledgements.recv().await.unwrap();
    let state = fixture.bridge.output_ack_state().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.bridge.acknowledge_output(&*ack, state.as_ref()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        matches!(&*fixture.writes.lock().unwrap()[0], Message::Binary(bytes) if bytes.len() == 25 && bytes[0] == 5)
    );
    drop(held);
}

#[tokio::test]
async fn recovered_terminal_keeps_its_original_owner_after_json_drops() {
    let fixture = fixture();
    let prompt = crate::PromptIdentity::new("conversation", 1, 2);
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    *fixture.bridge.recovery.lock().await =
        super::super::recovery::PromptRecovery::new(Some(prompt.clone()), Some(&fixture.policy))
            .unwrap();
    let before = fixture.root.available_bytes();
    let value = json!({"id":prompt.as_str(), "result":{"stopReason":"end_turn", "_meta":{"text":"x".repeat(65536)}}});
    let terminal = fixture.bridge.prepare_terminal(&value).unwrap();
    let frame = fixture
        .bridge
        .json_pools
        .as_ref()
        .unwrap()
        .decode(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .with_prepared_terminal(terminal);
    assert!(fixture
        .bridge
        .recover_prompt_result(prompt.as_str(), &frame, &relay)
        .await
        .unwrap());
    drop(frame);
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.root.available_bytes() < before);
    fixture.bridge.release_recovered_prompt(&relay).await;
    let terminal = captured.take().unwrap().unwrap();
    assert!(
        matches!(terminal.effect(), ConversationUpdate::TurnEndedForPrompt {prompt_id, ..} if prompt_id == &prompt)
    );
    drop(terminal);
    assert_eq!(fixture.root.available_bytes(), before);
}

#[test]
fn prompt_id_borrowed_check_matches_the_native_parser() {
    let valid = crate::PromptIdentity::new("conversation", 1, 2);
    for id in [
        valid.as_str(),
        "",
        "cma-prompt-v1-xyz",
        "1",
        "cma-rpc-fixture-1",
    ] {
        assert_eq!(
            valid_prompt_id(id),
            crate::PromptIdentity::parse(id).is_some()
        );
    }
}

#[tokio::test]
async fn prompt_admission_failure_does_not_publish_a_native_terminal() {
    let fixture = fixture();
    fixture.activate().await;
    let (relay, captured) = UpdateRelay::capture_admitted(&fixture.policy).unwrap();
    let content = vec![serde_json::from_value::<crate::ContentBlock>(
        json!({"type":"text", "text":"retry this exact prompt"}),
    )
    .unwrap()];
    let data = fixture.policy.budget(IngressClass::Data);
    let before = data.available_bytes();
    let preparation = super::identity::json_storage(Some(&fixture.policy), &content).unwrap();
    let preparation_bytes = before - data.available_bytes();
    drop(preparation);
    let held_bytes = before - 1024 - preparation_bytes;
    let held = fixture
        .policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("pressure before RPC preparation", &[held_bytes]).unwrap(),
        )
        .unwrap()
        .initialize(|| vec![0u8; held_bytes - 512]);
    let result = fixture
        .bridge
        .drive(
            crate::ConversationInput::RecoverablePrompt {
                content,
                prompt_id: crate::PromptIdentity::new("conversation", 1, 2),
            },
            &relay,
        )
        .await;
    assert!(matches!(result, Err(crate::AdapterError::AtCapacity(_))));
    assert!(captured.take().unwrap().is_none());
    assert!(fixture.writes.lock().unwrap().is_empty());
    assert!(fixture.bridge.pending.lock().await.is_empty());
    assert_eq!(
        fixture
            .bridge
            .owned_prompt_waiters
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    drop(held);
    assert_eq!(data.available_bytes(), before);
}

#[tokio::test]
async fn admitted_open_refuses_each_missing_prerequisite_before_polling_socket() {
    for missing in 0..5 {
        let fixture = fixture();
        let before = fixture.root.available_bytes();
        let mut settings = super::super::tests::settings(None, false);
        settings.retained_json = missing != 0;
        settings.bounded_raw_output = missing != 1;
        settings.bounded_permission_state = missing != 2;
        settings.shared_runtime_output = missing != 3;
        settings.output_json_budget = missing != 4;
        let (updates, _) = mpsc::channel(1);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            super::super::Bridge::open_admitted(
                Box::new(SyntheticSocket(fixture.writes.clone())),
                "agent",
                "/synthetic/work".into(),
                (settings, None, fixture.policy.clone()),
                None,
                None,
                updates,
            ),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(crate::AdapterError::Rejected(_))));
        assert!(fixture.writes.lock().unwrap().is_empty());
        assert_eq!(fixture.root.available_bytes(), before);
    }
}
