use super::*;
use layer2_core::sandbox::{AgentOutputCommit, AgentUpdateOutcome};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct BatchRecorder {
    last: Mutex<Option<RuntimeOutputSource>>,
    attempts: AtomicUsize,
    failures: usize,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for BatchRecorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("live consumption must use the explicit commit result")
    }
    async fn record_output(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<AgentOutputCommit, AgentConversationError> {
        let ConversationUpdate::RuntimeOutput { receipt, .. } = update else {
            panic!("expected receipt")
        };
        *self.last.lock().unwrap() = Some(receipt.source);
        Ok(AgentOutputCommit {
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
                "simulated unknown commit".into(),
            ));
        }
        Ok(self.last.lock().unwrap().clone())
    }
}

fn source(cursor: u64) -> RuntimeOutputSource {
    RuntimeOutputSource {
        stream_id: uuid::Uuid::nil().to_string(),
        cursor,
        native_conversation_id: "test".into(),
        payload_sha256: format!("{cursor:064x}"),
    }
}

#[tokio::test(start_paused = true)]
async fn quiet_batch_acks_only_last_receipt_after_unavailable_flush_retries() {
    let recorder = BatchRecorder {
        last: Mutex::new(None),
        attempts: AtomicUsize::new(0),
        failures: 2,
    };
    let mut assembly = AgentMessageAssembly::default();
    let mut pending = None;
    let (send, mut receive) = mpsc::channel(8);
    let id = AgentId("agt_batch".into());
    for cursor in 1..=3 {
        process_update(
            &recorder,
            &id,
            1,
            &mut assembly,
            ConversationUpdate::RuntimeOutput {
                receipt: RuntimeOutputReceipt::new(source(cursor), send.clone()),
                update: None,
            },
            &mut pending,
        )
        .await
        .unwrap();
        assert!(receive.try_recv().is_err());
    }
    assert_eq!(pending.as_ref().unwrap().source.cursor, 3);
    let started = tokio::time::Instant::now();
    flush_output(&recorder, &id, 1, &mut assembly, &mut pending)
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert_eq!(recorder.attempts.load(Ordering::SeqCst), 3);
    assert_eq!(receive.recv().await, Some(source(3)));
    assert!(receive.try_recv().is_err());
    assert!(pending.is_none());
}

#[tokio::test]
async fn cumulative_ack_requires_exact_receipt_and_keeps_newer_pending_output() {
    let (send, mut receive) = mpsc::channel(8);
    let mut pending = Some(RuntimeOutputReceipt::new(source(3), send.clone()));
    let older = RuntimeOutputReceipt::new(source(1), send);
    acknowledge(&source(1), Some(&older), &mut pending)
        .await
        .unwrap();
    assert_eq!(receive.recv().await, Some(source(1)));
    assert_eq!(pending.as_ref().unwrap().source.cursor, 3);
    let mut invalid = source(3);
    invalid.payload_sha256 = "f".repeat(64);
    assert!(acknowledge(&invalid, None, &mut pending).await.is_err());
    assert!(receive.try_recv().is_err());
    drop(pending);
    assert!(
        receive.try_recv().is_err(),
        "cancellation must not acknowledge uncommitted output"
    );
}
