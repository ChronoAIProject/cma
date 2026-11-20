use super::*;
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, OwnedWork, RecordingSession,
};
use layer2_core::sandbox::store::SandboxMutationGuard;
use layer2_core::sandbox::{AgentUpdateOutcome, SandboxStatus, SandboxWorkStep};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource, RuntimeOutputUpdate};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Recorder {
    attempts: AtomicUsize,
    sessions: AtomicUsize,
    refuse: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
    fn recording_session(&self) -> layer2_core::sandbox::ingress_ownership::RecordingSession {
        self.sessions.fetch_add(1, Ordering::SeqCst);
        layer2_core::sandbox::ingress_ownership::RecordingSession::new(None)
    }

    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("reopen must retain its original write fence")
    }
    async fn record_reopen_update(
        &self,
        _: &AgentId,
        guard: &AgentReopenGuard,
        _: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        assert_eq!(guard.agent_fence, 2);
        assert_eq!(guard.runtime_generation, 7);
        let source = &update.delivery().receipt.unwrap().source;
        assert_eq!(source.native_conversation_id, guard.native_conversation_id);
        assert_eq!(source.cursor, 19);
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
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
async fn outer_shared_reopen_delivery_acknowledges_only_after_fenced_retry_succeeds() {
    for refuse in [false, true] {
        let recorder = Recorder {
            attempts: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
            refuse,
        };
        let source = RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000001".into(),
            cursor: 19,
            native_conversation_id: "synthetic-conversation".into(),
            payload_sha256: "b".repeat(64),
        };
        let guard = guard();
        let (ack, mut acknowledged) = mpsc::channel(1);
        let (send, mut receiver) = mpsc::channel(1);
        send.send(ConversationUpdate::RetainedEffect(
            RuntimeOutputUpdate::new(
                ConversationUpdate::RuntimeOutput {
                    receipt: RuntimeOutputReceipt::new(source.clone(), ack),
                    update: None,
                },
                true,
            ),
        ))
        .await
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            drain(
                async { Ok(acknowledged.recv().await) },
                &mut receiver,
                &recorder,
                &AgentId("agt_shared_reopen".into()),
                &guard,
            ),
        )
        .await
        .expect("reopen must acknowledge or refuse without waiting for timeout");
        assert_eq!(recorder.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(recorder.sessions.load(Ordering::SeqCst), 1);
        if refuse {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), Some(source));
        }
        assert!(acknowledged.try_recv().is_err());
    }
}

fn guard() -> AgentReopenGuard {
    AgentReopenGuard {
        sandbox: SandboxMutationGuard {
            sandbox_id: layer2_core::SandboxId("sbx_shared_reopen".into()),
            owner: layer2_core::SubjectId::new("synthetic-owner"),
            expected_revision: 1,
            status: SandboxStatus::Reviving,
            work_step: SandboxWorkStep::RunLifecycleOperation,
            lease: None,
            operation: None,
            runtime: None,
            checkpoint_capture: None,
            background_checkpoint: None,
            checkpoint_epoch_high_water: 0,
        },
        agent_fence: 2,
        runtime_generation: 7,
        native_conversation_id: "synthetic-conversation".into(),
    }
}

struct OwnedRecorder {
    policy: layer2_core::sandbox::ingress_ownership::IngressOwnershipPolicy,
    enabled: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for OwnedRecorder {
    fn recording_session(&self) -> layer2_core::sandbox::ingress_ownership::RecordingSession {
        layer2_core::sandbox::ingress_ownership::RecordingSession::new(
            self.enabled.then(|| self.policy.clone()),
        )
    }

    fn record_session_reopen_operation<'a>(
        &'a self,
        id: &'a AgentId,
        _: &'a AgentReopenGuard,
        _: &'a mut RecordingSession,
        _: &'a ConversationUpdate,
        _: std::time::SystemTime,
    ) -> Result<
        AdmittedOperation<'a, Result<AgentUpdateOutcome, AgentConversationError>>,
        AdmittedError,
    > {
        AdmittedOperation::from_owned(
            &self.policy,
            IngressClass::Transformation,
            AllocationPlan::checked("reopen operation fixture", &[1024])?,
            move |_| async move { Ok(self.outcome(id).unwrap().map(Ok)) },
        )
    }

    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("reopen must use the session entry point")
    }

    async fn record_session_reopen(
        &self,
        id: &AgentId,
        _: &AgentReopenGuard,
        _: &mut layer2_core::sandbox::ingress_ownership::RecordingSession,
        _: &ConversationUpdate,
        _: String,
    ) -> Result<
        layer2_core::sandbox::ingress_ownership::OwnedWork<AgentUpdateOutcome>,
        layer2_core::sandbox::ingress_ownership::OwnedWork<AgentConversationError>,
    > {
        assert!(!self.enabled, "enabled recorder must enter synchronously");
        self.outcome(id)
    }
}

impl OwnedRecorder {
    fn outcome(
        &self,
        id: &AgentId,
    ) -> Result<OwnedWork<AgentUpdateOutcome>, OwnedWork<AgentConversationError>> {
        use layer2_core::sandbox::ingress_ownership::IngressWorkScope;
        let scope = IngressWorkScope::new(
            self.policy
                .admit(
                    IngressClass::Data,
                    AllocationPlan::checked("reopen returned result", &[8192]).unwrap(),
                )
                .unwrap(),
        );
        Ok(scope.initialize_owned(|| AgentUpdateOutcome {
            appended: vec![layer2_core::sandbox::AgentLogEntry {
                agent_id: id.clone(),
                seq: 1,
                appended_at: "2026-01-01T00:00:00Z".into(),
                kind: layer2_core::sandbox::AgentLogKind::AgentMessage {
                    content: "x".repeat(1024),
                },
            }],
            turn_closed: false,
        }))
    }
}

#[tokio::test]
async fn reopen_result_stays_charged_through_blocked_ack_and_cancellation() {
    use layer2_core::sandbox::ingress_ownership::{
        AllocationPlan, ClassBounds, IngressBudget, IngressOwnershipPolicy,
    };
    for cancel in [false, true] {
        let root = IngressBudget::new(32768);
        let plan = |n| AllocationPlan::checked("reopen result fixture", &[n]).unwrap();
        let policy = IngressOwnershipPolicy::new(
            root.clone(),
            ClassBounds::new(32768, plan(1024), plan(8192)).unwrap(),
        )
        .unwrap();
        let recorder = OwnedRecorder {
            policy,
            enabled: false,
        };
        let source = RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000002".into(),
            cursor: 23,
            native_conversation_id: "synthetic-conversation".into(),
            payload_sha256: "c".repeat(64),
        };
        let (ack, mut acknowledged) = mpsc::channel(1);
        ack.send(source.clone()).await.unwrap();
        let (send, mut receiver) = mpsc::channel(1);
        send.send(ConversationUpdate::RuntimeOutput {
            receipt: RuntimeOutputReceipt::new(source.clone(), ack),
            update: None,
        })
        .await
        .unwrap();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let id = AgentId("agt_owned_reopen".into());
        let guard = guard();
        let mut draining = Box::pin(drain(
            async {
                finished.await.unwrap();
                Ok(())
            },
            &mut receiver,
            &recorder,
            &id,
            &guard,
        ));
        assert!(futures_util::poll!(&mut draining).is_pending());
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes() - 8192,
            "reopen released its returned outcome before acknowledgement completed"
        );
        if cancel {
            drop(draining);
            assert_eq!(acknowledged.recv().await, Some(source));
            assert_eq!(acknowledged.recv().await, None);
        } else {
            finish.send(()).unwrap();
            assert_eq!(acknowledged.recv().await, Some(source.clone()));
            draining.await.unwrap();
            assert_eq!(acknowledged.recv().await, Some(source));
            assert_eq!(acknowledged.recv().await, None);
        }
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
    }
}

#[tokio::test]
async fn enabled_reopen_uses_admitted_ack_and_refuses_legacy_or_full_queues() {
    use layer2_core::sandbox::ingress_ownership::{
        AllocationPlan, ClassBounds, IngressBudget, IngressOwnershipPolicy,
    };
    for mode in ["success", "full", "legacy"] {
        let root = IngressBudget::new(256 * 1024);
        let plan = |n| AllocationPlan::checked("reopen ACK fixture", &[n]).unwrap();
        let policy = IngressOwnershipPolicy::new(
            root.clone(),
            ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024)).unwrap(),
        )
        .unwrap();
        let (sender, mut acknowledged) =
            RuntimeOutputReceipt::admitted_channel_checked(policy.clone(), 1).unwrap();
        let source = RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000002".into(),
            cursor: 23,
            native_conversation_id: "synthetic-conversation".into(),
            payload_sha256: "c".repeat(64),
        };
        let (legacy, mut legacy_acknowledged) = mpsc::channel(1);
        let receipt = if mode == "legacy" {
            drop(sender);
            RuntimeOutputReceipt::new(source.clone(), legacy)
        } else {
            RuntimeOutputReceipt::new_admitted(source.clone(), sender)
        };
        if mode == "full" {
            receipt.acknowledge_admitted().unwrap();
        }
        let recorder = OwnedRecorder {
            policy,
            enabled: true,
        };
        let (send, mut receiver) = mpsc::channel(1);
        send.send(ConversationUpdate::RuntimeOutput {
            receipt,
            update: None,
        })
        .await
        .unwrap();
        let id = AgentId("agt_admitted_reopen".into());
        let guard = guard();
        let mut draining = Box::pin(drain(
            std::future::pending::<layer3_adapter::Result<()>>(),
            &mut receiver,
            &recorder,
            &id,
            &guard,
        ));
        let polled = futures_util::poll!(&mut draining);
        if mode == "success" {
            assert!(polled.is_pending());
        } else {
            let std::task::Poll::Ready(Err(error)) = polled else {
                panic!("unadmitted or full ACK queue must refuse without waiting");
            };
            assert!(matches!(error, AdapterError::Unavailable(detail) if detail.is_empty()));
        }
        drop(draining);
        if mode != "legacy" {
            assert_eq!(*acknowledged.recv().await.unwrap(), source);
        }
        assert!(legacy_acknowledged.try_recv().is_err());
        drop(acknowledged);
        drop(recorder);
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - root.metadata_bytes()
        );
    }
}
