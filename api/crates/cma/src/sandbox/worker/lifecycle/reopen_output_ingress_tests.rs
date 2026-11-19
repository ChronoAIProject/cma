use super::*;
use layer2_core::sandbox::store::SandboxMutationGuard;
use layer2_core::sandbox::{AgentUpdateOutcome, SandboxStatus, SandboxWorkStep};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource, RuntimeOutputUpdate};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Recorder {
    attempts: AtomicUsize,
    refuse: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for Recorder {
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
            refuse,
        };
        let source = RuntimeOutputSource {
            stream_id: "00000000-0000-4000-8000-000000000001".into(),
            cursor: 19,
            native_conversation_id: "synthetic-conversation".into(),
            payload_sha256: "b".repeat(64),
        };
        let guard = AgentReopenGuard {
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
            native_conversation_id: source.native_conversation_id.clone(),
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
                &AgentId("agt_shared_reopen".into()),
                &guard,
            ),
        )
        .await
        .expect("reopen must acknowledge or refuse without waiting for timeout");
        assert_eq!(recorder.attempts.load(Ordering::SeqCst), 2);
        if refuse {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), Some(source));
        }
        assert!(acknowledged.try_recv().is_err());
    }
}
