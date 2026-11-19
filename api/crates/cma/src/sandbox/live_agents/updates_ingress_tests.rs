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
