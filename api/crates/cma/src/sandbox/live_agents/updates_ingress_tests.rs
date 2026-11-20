use super::*;
use layer2_core::sandbox::AgentUpdateOutcome;
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource, RuntimeOutputUpdate};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Recorder {
    source: RuntimeOutputSource,
    effect_address: usize,
    attempts: AtomicUsize,
    refuse: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
    async fn record_update(
        &self,
        _: &AgentId,
        generation: u64,
        _: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        let delivery = update.delivery();
        assert_eq!(generation, 7);
        assert_eq!(delivery.receipt.unwrap().source, self.source);
        assert_eq!(
            delivery.effect.unwrap() as *const _ as usize,
            self.effect_address
        );
        if self.attempts.fetch_add(1, Ordering::SeqCst) < 2 {
            return Err(AgentConversationError::Unavailable(
                "synthetic lost reply".into(),
            ));
        }
        if self.refuse {
            return Err(AgentConversationError::StaleGeneration);
        }
        Ok(AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        })
    }
}

#[tokio::test(start_paused = true)]
async fn outer_shared_delivery_preserves_identity_and_exact_ack_across_live_retries() {
    for refuse in [false, true] {
        let source = RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000001".into(),
            cursor: 19,
            native_conversation_id: "synthetic-conversation".into(),
            payload_sha256: "b".repeat(64),
        };
        let (ack, mut acknowledged) = mpsc::channel(1);
        let update = ConversationUpdate::RetainedEffect(RuntimeOutputUpdate::new(
            ConversationUpdate::RuntimeOutput {
                receipt: RuntimeOutputReceipt::new(source.clone(), ack),
                update: Some(RuntimeOutputUpdate::new(
                    ConversationUpdate::Failed {
                        message: "synthetic payload".repeat(4096),
                        capability: None,
                    },
                    true,
                )),
            },
            true,
        ));
        let recorder = Recorder {
            source: source.clone(),
            effect_address: update.delivery().effect.unwrap() as *const _ as usize,
            attempts: AtomicUsize::new(0),
            refuse,
        };
        let result = record_update(
            &recorder,
            &AgentId("agt_shared_retry".into()),
            7,
            &mut AgentMessageAssembly::default(),
            update,
        )
        .await;
        assert_eq!(recorder.attempts.load(Ordering::SeqCst), 3);
        assert_eq!(result.is_ok(), !refuse);
        if !refuse {
            assert_eq!(acknowledged.try_recv().unwrap(), source);
        }
        assert!(acknowledged.try_recv().is_err());
    }
}

struct QuietRecorder {
    source: RuntimeOutputSource,
    attempts: AtomicUsize,
    failures: usize,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for QuietRecorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("quiet consumers use record_output")
    }

    async fn record_output(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<layer2_core::sandbox::AgentOutputCommit, AgentConversationError> {
        assert_eq!(update.delivery().receipt.unwrap().source, self.source);
        Ok(layer2_core::sandbox::AgentOutputCommit {
            outcome: AgentUpdateOutcome {
                appended: vec![],
                turn_closed: false,
            },
            committed: None,
        })
    }

    async fn flush_output(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: String,
    ) -> Result<Option<RuntimeOutputSource>, AgentConversationError> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) < self.failures {
            return Err(AgentConversationError::Unavailable(
                "synthetic unavailable quiet checkpoint".into(),
            ));
        }
        Ok(Some(self.source.clone()))
    }
}

fn quiet_recorder(cursor: u64) -> QuietRecorder {
    let source = RuntimeOutputSource {
        stream_id: "00000000-0000-4000-8000-000000000001".into(),
        cursor,
        native_conversation_id: "synthetic-quiet-conversation".into(),
        payload_sha256: format!("{cursor:064x}"),
    };
    QuietRecorder {
        source,
        attempts: AtomicUsize::new(0),
        failures: 2,
    }
}

fn admitted_quiet_delivery(
    policy: &layer2_core::sandbox::ingress_ownership::IngressOwnershipPolicy,
    cursor: u64,
) -> (
    ConversationUpdate,
    QuietRecorder,
    mpsc::Receiver<RuntimeOutputSource>,
) {
    use layer2_core::sandbox::ingress_ownership::{AllocationPlan, IngressClass};
    let recorder = quiet_recorder(cursor);
    let (ack, acknowledged) = mpsc::channel(1);
    let storage = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("synthetic quiet delivery", &[QUIET_DELIVERY_BYTES]).unwrap(),
        )
        .unwrap();
    let value = storage.initialize(|| ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(recorder.source.clone(), ack),
        update: Some(RuntimeOutputUpdate::new(
            ConversationUpdate::Failed {
                message: "x".repeat(1024 * 1024 - 1),
                capability: None,
            },
            true,
        )),
    });
    (
        ConversationUpdate::RetainedEffect(RuntimeOutputUpdate::from_admitted(value)),
        recorder,
        acknowledged,
    )
}

const QUIET_DELIVERY_BYTES: usize = 1024 * 1024 + 8192;

fn quiet_policy() -> (
    layer2_core::sandbox::ingress_ownership::IngressBudget,
    layer2_core::sandbox::ingress_ownership::IngressOwnershipPolicy,
) {
    use layer2_core::sandbox::ingress_ownership::{
        AllocationPlan, ClassBounds, IngressBudget, IngressOwnershipPolicy,
    };
    let root = IngressBudget::new(4 * QUIET_DELIVERY_BYTES);
    let bounds = ClassBounds::new(
        root.capacity_bytes(),
        AllocationPlan::new("control", &[64 * 1024]).unwrap(),
        AllocationPlan::new("work", &[1024]).unwrap(),
    )
    .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    (root, policy)
}

#[tokio::test(start_paused = true)]
async fn quiet_flush_keeps_original_receipt_charged_through_retries_and_exact_ack() {
    let (root, policy) = quiet_policy();
    let (update, recorder, mut acknowledged) = admitted_quiet_delivery(&policy, 31);
    let receipt_address = &update.delivery().receipt.unwrap().source as *const _ as usize;
    let mut pending = None;
    let mut assembly = RecordingSession::new(None);
    let id = AgentId("agt_quiet_owner".into());
    process_update(&recorder, &id, 7, &mut assembly, update, &mut pending)
        .await
        .unwrap();
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes() - QUIET_DELIVERY_BYTES,
        "quiet buffering released the original receipt owner"
    );
    assert_eq!(
        &pending.as_ref().unwrap().source as *const _ as usize,
        receipt_address,
        "quiet buffering copied the receipt out of its owner"
    );
    let mut flush = Box::pin(flush_output(&recorder, &id, 7, &mut assembly, &mut pending));
    assert!(futures_util::poll!(&mut flush).is_pending());
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes() - QUIET_DELIVERY_BYTES
    );
    assert!(acknowledged.try_recv().is_err());
    flush.await.unwrap();
    assert_eq!(recorder.attempts.load(Ordering::SeqCst), 3);
    assert_eq!(acknowledged.try_recv().unwrap(), recorder.source);
    assert!(acknowledged.try_recv().is_err());
    assert!(pending.is_none());
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test(start_paused = true)]
async fn cancelling_quiet_flush_retains_original_receipt_until_pending_state_drops() {
    let (root, policy) = quiet_policy();
    let (update, mut recorder, mut acknowledged) = admitted_quiet_delivery(&policy, 41);
    recorder.failures = usize::MAX;
    let mut pending = None;
    let mut assembly = RecordingSession::new(None);
    let id = AgentId("agt_quiet_cancel".into());
    process_update(&recorder, &id, 7, &mut assembly, update, &mut pending)
        .await
        .unwrap();
    let mut flush = Box::pin(flush_output(&recorder, &id, 7, &mut assembly, &mut pending));
    assert!(futures_util::poll!(&mut flush).is_pending());
    drop(flush);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes() - QUIET_DELIVERY_BYTES
    );
    assert!(acknowledged.try_recv().is_err());
    drop(pending);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
    assert!(acknowledged.try_recv().is_err());
}

#[tokio::test]
async fn replacing_quiet_receipt_releases_only_the_superseded_delivery() {
    let (root, policy) = quiet_policy();
    let mut assembly = RecordingSession::new(None);
    let mut pending = None;
    let id = AgentId("agt_quiet_replace".into());
    for (cursor, retained_cursor) in [(1, 1), (2, 2), (1, 2)] {
        let (update, recorder, mut acknowledged) = admitted_quiet_delivery(&policy, cursor);
        process_update(&recorder, &id, 7, &mut assembly, update, &mut pending)
            .await
            .unwrap();
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes() - QUIET_DELIVERY_BYTES,
            "exactly the newest quiet delivery must remain charged"
        );
        assert_eq!(pending.as_ref().unwrap().source.cursor, retained_cursor);
        assert!(acknowledged.try_recv().is_err());
    }
    assert_eq!(pending.as_ref().unwrap().source.cursor, 2);
    drop(pending);
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

#[tokio::test(start_paused = true)]
async fn quiet_ack_uses_control_capacity_while_data_is_saturated() {
    use layer2_core::sandbox::ingress_ownership::{AllocationPlan, IngressClass};
    let (root, policy) = quiet_policy();
    let recorder = quiet_recorder(51);
    let (ack, mut acknowledged) =
        RuntimeOutputReceipt::admitted_channel(policy.clone(), 1).unwrap();
    let baseline = root.available_bytes();
    let value = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("quiet delivery", &[QUIET_DELIVERY_BYTES]).unwrap(),
        )
        .unwrap()
        .initialize(|| ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new_admitted(recorder.source.clone(), ack),
            update: Some(RuntimeOutputUpdate::new(
                ConversationUpdate::Failed {
                    message: "x".repeat(1024 * 1024 - 1),
                    capability: None,
                },
                true,
            )),
        });
    let update = ConversationUpdate::RetainedEffect(RuntimeOutputUpdate::from_admitted(value));
    let mut pending = None;
    let mut assembly = RecordingSession::new(None);
    let id = AgentId("agt_quiet_control".into());
    process_update(&recorder, &id, 7, &mut assembly, update, &mut pending)
        .await
        .unwrap();
    assert_eq!(root.available_bytes(), baseline - QUIET_DELIVERY_BYTES);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .reserve(
            IngressClass::Data,
            AllocationPlan::new("data pressure", &[available]).unwrap(),
        )
        .unwrap();
    assert_eq!(policy.budget(IngressClass::Data).available_bytes(), 0);
    flush_output(&recorder, &id, 7, &mut assembly, &mut pending)
        .await
        .unwrap();
    let committed = acknowledged.recv().await.unwrap();
    assert_eq!(*committed, recorder.source);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), acknowledged.recv())
            .await
            .unwrap()
            .is_none()
    );
    assert!(pending.is_none());
    assert_eq!(recorder.attempts.load(Ordering::SeqCst), 3);
    drop((committed, acknowledged, pressure));
    assert_eq!(
        root.available_bytes(),
        root.capacity_bytes() - root.metadata_bytes()
    );
}

struct OwnedResultRecorder {
    policy: layer2_core::sandbox::ingress_ownership::IngressOwnershipPolicy,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for OwnedResultRecorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("the consumer must use its session entry point")
    }

    async fn record_session_output(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut RecordingSession,
        update: &ConversationUpdate,
        _: String,
    ) -> Result<OwnedWork<layer2_core::sandbox::AgentOutputCommit>, OwnedWork<AgentConversationError>>
    {
        use layer2_core::sandbox::ingress_ownership::{AllocationPlan, IngressClass};
        let scope = IngressWorkScope::new(
            self.policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("synthetic owned recording result", &[4096]).unwrap(),
                )
                .unwrap(),
        );
        scope
            .run(async {
                Ok(layer2_core::sandbox::AgentOutputCommit {
                    outcome: AgentUpdateOutcome {
                        appended: vec![],
                        turn_closed: false,
                    },
                    committed: Some(update.delivery().receipt.unwrap().source.clone()),
                })
            })
            .await
            .transpose()
    }
}

#[tokio::test]
async fn returned_commit_stays_charged_through_blocked_ack_and_cancellation() {
    for cancel in [false, true] {
        let (root, policy) = quiet_policy();
        let recorder = OwnedResultRecorder { policy };
        let source = quiet_recorder(61).source;
        let (send, mut receive) = mpsc::channel(1);
        // Keep the legacy sink full so the final ACK actually suspends.
        send.send(source.clone()).await.unwrap();
        let update = ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(source.clone(), send),
            update: None,
        };
        let mut session = recorder.recording_session();
        let mut pending = None;
        let id = AgentId("agt_owned_result".into());
        let mut operation = Box::pin(process_update(
            &recorder,
            &id,
            7,
            &mut session,
            update,
            &mut pending,
        ));
        assert!(futures_util::poll!(&mut operation).is_pending());
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes() - 4096
        );
        if cancel {
            drop(operation);
            assert_eq!(receive.recv().await, Some(source));
            assert_eq!(receive.recv().await, None);
        } else {
            assert_eq!(receive.recv().await, Some(source.clone()));
            operation.await.unwrap();
            assert_eq!(receive.recv().await, Some(source));
            assert_eq!(receive.recv().await, None);
        }
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
    }
}
