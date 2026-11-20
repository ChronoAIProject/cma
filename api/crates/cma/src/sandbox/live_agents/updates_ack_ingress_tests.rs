use super::*;
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, ClassBounds, IngressBudget, IngressClass,
    IngressOwnershipPolicy,
};
use layer2_core::sandbox::{AgentOutputCommit, AgentUpdateOutcome};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
use std::sync::atomic::{AtomicUsize, Ordering};

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(256 * 1024);
    let plan = |bytes| AllocationPlan::checked("live ACK fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024)).unwrap(),
    )
    .unwrap();
    (root, policy)
}

fn source(cursor: u64) -> RuntimeOutputSource {
    RuntimeOutputSource {
        stream_id: "00000000-0000-4000-8000-000000000001".into(),
        cursor,
        native_conversation_id: "synthetic-conversation".into(),
        payload_sha256: "b".repeat(64),
    }
}

#[tokio::test]
async fn admitted_ack_refusal_retains_pending_identity_until_exact_retry() {
    let (root, policy) = policy();
    let (sender, mut receiver) = RuntimeOutputReceipt::admitted_channel_checked(policy, 1).unwrap();
    let earlier = RuntimeOutputReceipt::new_admitted(source(1), sender.clone());
    earlier.acknowledge_admitted().unwrap();
    let committed = source(2);
    let receipt = RuntimeOutputReceipt::new_admitted(committed.clone(), sender);
    let mut pending = Some(PendingReceipt::Legacy(receipt));
    let before = root.available_bytes();
    let error = acknowledge(&committed, None, &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::Unavailable(detail) if detail.is_empty()));
    assert_eq!(pending.as_ref().unwrap().source, committed);
    assert_eq!(root.available_bytes(), before);
    assert_eq!(*receiver.recv().await.unwrap(), earlier.source);
    acknowledge(&committed, None, &mut pending, true)
        .await
        .unwrap();
    assert!(pending.is_none());
    assert_eq!(*receiver.recv().await.unwrap(), committed);
    drop(earlier);
    assert!(receiver.recv().await.is_none());
    drop(receiver);
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn enabled_ack_refuses_legacy_queue_without_enqueuing() {
    let (sender, mut receiver) = mpsc::channel(1);
    let source = source(1);
    let receipt = RuntimeOutputReceipt::new(source.clone(), sender);
    let mut pending = None;
    let error = acknowledge(&source, Some(&receipt), &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::Unavailable(detail) if detail.is_empty()));
    assert!(receiver.try_recv().is_err());
    assert_eq!(receipt.source, source);
}

#[tokio::test]
async fn enabled_ack_refuses_closed_queue_and_preserves_the_pending_receipt() {
    let (root, policy) = policy();
    let (sender, receiver) = RuntimeOutputReceipt::admitted_channel_checked(policy, 1).unwrap();
    drop(receiver);
    let source = source(1);
    let mut pending = Some(PendingReceipt::Legacy(RuntimeOutputReceipt::new_admitted(
        source.clone(),
        sender,
    )));
    let before = root.available_bytes();
    let error = acknowledge(&source, None, &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::Unavailable(detail) if detail.is_empty()));
    assert_eq!(pending.as_ref().unwrap().source, source);
    assert_eq!(root.available_bytes(), before);
    drop(pending);
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[tokio::test]
async fn admitted_ack_cannot_substitute_a_different_committed_source() {
    let (root, policy) = policy();
    let (sender, mut receiver) = RuntimeOutputReceipt::admitted_channel_checked(policy, 1).unwrap();
    let receipt = RuntimeOutputReceipt::new_admitted(source(1), sender);
    let mut pending = None;
    let error = acknowledge(&source(2), Some(&receipt), &mut pending, true)
        .await
        .unwrap_err();
    assert!(matches!(&*error, AgentConversationError::InvalidState(_)));
    drop(receipt);
    assert!(receiver.recv().await.is_none());
    drop(receiver);
    drop(error);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

struct AdmittedRecorder {
    policy: IngressOwnershipPolicy,
    source: RuntimeOutputSource,
    calls: AtomicUsize,
}

impl AdmittedRecorder {
    fn operation<'a, T: Send + 'a>(
        &'a self,
        call: usize,
        initialize: impl FnOnce() -> T + Send + 'a,
    ) -> Result<AdmittedOperation<'a, Result<T, AgentConversationError>>, AdmittedError> {
        AdmittedOperation::from_owned(
            &self.policy,
            IngressClass::Transformation,
            AllocationPlan::checked("live recording operation", &[1024])?,
            move |_| async move {
                let retained = IngressWorkScope::new(self.policy.admit(
                    IngressClass::Data,
                    AllocationPlan::checked("live recording result", &[4096])?,
                )?);
                Ok(retained.initialize_owned(|| {
                    self.calls.fetch_or(call, Ordering::SeqCst);
                    Ok(initialize())
                }))
            },
        )
    }
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for AdmittedRecorder {
    fn restore_session_operation<'a>(
        &'a self,
        _: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
    ) -> Result<AdmittedOperation<'a, Result<(), AgentConversationError>>, AdmittedError> {
        self.operation(1, || ())
    }

    fn record_session_output_operation<'a>(
        &'a self,
        _: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
        update: &'a ConversationUpdate,
        _: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentOutputCommit, AgentConversationError>>,
        AdmittedError,
    > {
        self.operation(2, move || {
            assert_eq!(update.delivery().receipt.unwrap().source, self.source);
            AgentOutputCommit {
                outcome: AgentUpdateOutcome {
                    appended: vec![],
                    turn_closed: false,
                },
                committed: None,
            }
        })
    }

    fn flush_session_output_operation<'a>(
        &'a self,
        _: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
        _: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<Option<RuntimeOutputSource>, AgentConversationError>>,
        AdmittedError,
    > {
        self.operation(4, || Some(self.source.clone()))
    }

    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("enabled consumer must enter synchronously")
    }
}

#[tokio::test]
async fn enabled_live_restore_record_and_flush_enter_admitted_ports_before_ack() {
    let (root, policy) = policy();
    let recorder = AdmittedRecorder {
        policy: policy.clone(),
        source: source(1),
        calls: AtomicUsize::new(0),
    };
    let (sender, mut receiver) =
        RuntimeOutputReceipt::admitted_channel_checked(policy.clone(), 1).unwrap();
    let value = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("live admitted input", &[4096]).unwrap(),
        )
        .unwrap()
        .initialize(|| ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new_admitted(recorder.source.clone(), sender),
            update: None,
        });
    let update = ConversationUpdate::RetainedEffect(
        layer3_adapter::RuntimeOutputUpdate::from_admitted(value),
    );
    let mut session = RecordingSession::new(Some(policy));
    let id = AgentId("agt_admitted_live_ports".into());
    let mut pending = None;
    restore_assembly(&recorder, &id, 7, &mut session)
        .await
        .unwrap();
    process_update(&recorder, &id, 7, &mut session, update, &mut pending)
        .await
        .unwrap();
    assert!(futures_util::FutureExt::now_or_never(receiver.recv()).is_none());
    assert_eq!(pending.as_ref().unwrap().source, recorder.source);
    flush_output(&recorder, &id, 7, &mut session, &mut pending)
        .await
        .unwrap();
    assert!(pending.is_none());
    assert_eq!(*receiver.recv().await.unwrap(), recorder.source);
    assert_eq!(recorder.calls.load(Ordering::SeqCst), 7);
    drop(receiver);
    drop(session);
    drop(recorder);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
