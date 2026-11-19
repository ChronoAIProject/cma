use super::*;
use layer2_core::sandbox::AgentUpdateOutcome;
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource, RuntimeOutputUpdate};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Recorder(AtomicUsize);

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
        assert_eq!(generation, 7);
        assert_eq!(update.delivery().receipt.unwrap().source.cursor, 19);
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
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
        let recorder = Recorder(AtomicUsize::new(0));
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
            assert_eq!(recorder.0.load(Ordering::SeqCst), 2);
        } else {
            assert!(result.is_err());
            assert_eq!(recorder.0.load(Ordering::SeqCst), 0);
        }
        assert!(acknowledged.try_recv().is_err());
    }
}
