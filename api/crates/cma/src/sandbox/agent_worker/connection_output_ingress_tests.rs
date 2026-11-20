use super::*;
use layer2_core::sandbox::ingress_ownership::{
    AdmittedError, AdmittedOperation, AllocationPlan, IngressClass, OwnedWork, RecordingSession,
};
use layer2_core::sandbox::AgentUpdateOutcome;
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource, RuntimeOutputUpdate};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Recorder {
    attempts: AtomicUsize,
    sessions: AtomicUsize,
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
        generation: u64,
        _: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        assert_eq!(generation, 7);
        assert_eq!(update.delivery().receipt.unwrap().source.cursor, 19);
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(AgentConversationError::Unavailable(
                "synthetic lost reply".into(),
            ));
        }
        Ok(AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        })
    }
}

#[tokio::test(start_paused = true)]
async fn outer_shared_reconnect_delivery_keeps_the_receipt_and_conversation_fence() {
    for conversation in ["synthetic-conversation", "another-conversation"] {
        let recorder = Recorder {
            attempts: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
        };
        let source = RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000001".into(),
            cursor: 19,
            native_conversation_id: conversation.into(),
            payload_sha256: "b".repeat(64),
        };
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
                &AgentId("agt_shared_reconnect".into()),
                7,
                "synthetic-conversation",
            ),
        )
        .await
        .expect("reconnect must acknowledge or refuse without waiting for timeout");
        if conversation == "synthetic-conversation" {
            assert_eq!(result.unwrap(), Some(source));
            assert_eq!(recorder.attempts.load(Ordering::SeqCst), 2);
        } else {
            assert!(result.is_err());
            assert_eq!(recorder.attempts.load(Ordering::SeqCst), 0);
        }
        assert_eq!(recorder.sessions.load(Ordering::SeqCst), 1);
        assert!(acknowledged.try_recv().is_err());
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

    async fn restore_session(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut layer2_core::sandbox::ingress_ownership::RecordingSession,
    ) -> Result<
        layer2_core::sandbox::ingress_ownership::OwnedWork<()>,
        layer2_core::sandbox::ingress_ownership::OwnedWork<AgentConversationError>,
    > {
        assert!(
            !self.enabled,
            "enabled restoration must enter synchronously"
        );
        Ok(
            layer2_core::sandbox::ingress_ownership::IngressWorkScope::inherit_owned(async {})
                .await,
        )
    }

    fn restore_session_operation<'a>(
        &'a self,
        _: &'a AgentId,
        _: u64,
        _: &'a mut RecordingSession,
    ) -> Result<AdmittedOperation<'a, Result<(), AgentConversationError>>, AdmittedError> {
        AdmittedOperation::from_owned(
            &self.policy,
            IngressClass::Transformation,
            AllocationPlan::checked("restore operation fixture", &[1024])?,
            |scope| async move { Ok(scope.initialize_owned(|| Ok(()))) },
        )
    }

    fn record_session_update_operation<'a>(
        &'a self,
        id: &'a AgentId,
        _: u64,
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
            AllocationPlan::checked("reconnect operation fixture", &[1024])?,
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
        panic!("reconnect must use the session entry point")
    }

    async fn record_session_update(
        &self,
        id: &AgentId,
        _: u64,
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
                    AllocationPlan::checked("reconnect returned result", &[8192]).unwrap(),
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
async fn reconnect_result_stays_charged_through_blocked_ack_and_cancellation() {
    use layer2_core::sandbox::ingress_ownership::{
        AllocationPlan, ClassBounds, IngressBudget, IngressOwnershipPolicy,
    };
    for cancel in [false, true] {
        let root = IngressBudget::new(32768);
        let plan = |n| AllocationPlan::checked("reconnect result fixture", &[n]).unwrap();
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
        let id = AgentId("agt_owned_reconnect".into());
        let mut draining = Box::pin(drain(
            async {
                finished.await.unwrap();
                Ok(())
            },
            &mut receiver,
            &recorder,
            &id,
            7,
            "synthetic-conversation",
        ));
        assert!(futures_util::poll!(&mut draining).is_pending());
        assert_eq!(
            root.available_bytes(),
            root.capacity_bytes() - 8192,
            "reconnect released its returned outcome before acknowledgement completed"
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
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[tokio::test]
async fn enabled_reconnect_uses_admitted_ack_and_refuses_legacy_or_full_queues() {
    use layer2_core::sandbox::ingress_ownership::{
        AllocationPlan, ClassBounds, IngressBudget, IngressOwnershipPolicy,
    };
    for mode in ["success", "full", "legacy"] {
        let root = IngressBudget::new(256 * 1024);
        let plan = |n| AllocationPlan::checked("reconnect ACK fixture", &[n]).unwrap();
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
        let id = AgentId("agt_admitted_reconnect".into());
        let mut draining = Box::pin(drain(
            std::future::pending::<Result<(), WorkerFailure>>(),
            &mut receiver,
            &recorder,
            &id,
            7,
            "synthetic-conversation",
        ));
        let polled = futures_util::poll!(&mut draining);
        if mode == "success" {
            assert!(polled.is_pending());
        } else {
            let std::task::Poll::Ready(Err(error)) = polled else {
                panic!("unadmitted or full ACK queue must refuse without waiting");
            };
            assert_eq!(error.to_string(), "agent_connection_output_unavailable: ");
        }
        drop(draining);
        if mode != "legacy" {
            assert_eq!(*acknowledged.recv().await.unwrap(), source);
        }
        assert!(legacy_acknowledged.try_recv().is_err());
        drop(acknowledged);
        drop(recorder);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}
