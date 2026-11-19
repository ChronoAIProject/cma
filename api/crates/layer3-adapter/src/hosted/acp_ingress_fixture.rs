use super::super::Mutex;
use super::*;
use crate::hosted::{retained_json::JsonPools, update_relay::UpdateRelay};
use crate::ingress_budget::{AllocationPlan, IngressBudget, IngressClass, IngressOwnershipPolicy};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize},
    Arc, Mutex as StdMutex,
};

type Writes = Arc<StdMutex<Vec<SharedOwned<Message>>>>;

pub(in crate::hosted) struct SyntheticSocket(pub(in crate::hosted) Writes);

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

pub(in crate::hosted) struct Fixture {
    pub(in crate::hosted) bridge: super::super::Bridge,
    pub(in crate::hosted) policy: IngressOwnershipPolicy,
    pub(in crate::hosted) root: IngressBudget,
    pub(in crate::hosted) stream: uuid::Uuid,
    pub(in crate::hosted) acknowledgements: crate::runtime_output::AdmittedAckReceiver,
    pub(in crate::hosted) writes: Writes,
}

pub(in crate::hosted) fn fixture() -> Fixture {
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
    let identity_storage = super::identity::bridge_identity_storage(Some(&policy)).unwrap();
    let bridge = Bridge {
        ingress: Some(policy.clone()),
        authority: None,
        json_pools: Some(pools.clone()),
        bounded_raw_output: true,
        output_json_budget: true,
        shared_runtime_output: true,
        permission_state: Some(
            Mutex::new(
                crate::hosted::permission_state::PermissionState::new(Some(data.clone())).unwrap(),
                Some(&policy),
            )
            .unwrap(),
        ),
        agent_correlation_digest: "fixture".into(),
        subscription: None,
        sink: Arc::new(Mutex::new(sink, Some(&policy)).unwrap()),
        pending: Arc::new(
            Mutex::new(
                super::identity::PendingCalls::new(Some(&policy)).unwrap(),
                Some(&policy),
            )
            .unwrap(),
        ),
        request_namespace: "fixture".into(),
        next_id: AtomicI64::new(1),
        recovery: Mutex::new(
            PromptRecovery::new(None, Some(&policy)).unwrap(),
            Some(&policy),
        )
        .unwrap(),
        conversation: Mutex::new(ConversationState::default(), Some(&policy)).unwrap(),
        output: Some(
            Mutex::new(
                ReliableOutput::new(
                    stream,
                    AckSender::Admitted(sender),
                    true,
                    Some(pools),
                    Some(data),
                    Some(&policy),
                )
                .unwrap(),
                Some(&policy),
            )
            .unwrap(),
        ),
        blocked: Mutex::new(HashMap::new(), Some(&policy)).unwrap(),
        answered: Arc::new(Mutex::new(HashMap::new(), Some(&policy)).unwrap()),
        owned_prompt_waiters: Arc::new(AtomicUsize::new(0)),
        orphaned_prompt_response: AtomicBool::new(false),
        retired: AtomicBool::new(false),
        instructions_support: AtomicU8::new(0),
        context_refresh: None,
        context_goal: Mutex::new(None, Some(&policy)).unwrap(),
        recovering_context: AtomicU64::new(0),
        retained_history_deadline: super::super::progress::ProgressSender::new(None, Some(&policy))
            .unwrap(),
        _identity_storage: identity_storage,
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
    pub(in crate::hosted) async fn activate(&self) {
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

    pub(in crate::hosted) fn retained(&self, value: &serde_json::Value, cursor: u64) -> Vec<u8> {
        let mut wire = self.stream.as_bytes().to_vec();
        wire.extend_from_slice(&cursor.to_be_bytes());
        wire.extend_from_slice(&serde_json::to_vec(value).unwrap());
        wire
    }
}
