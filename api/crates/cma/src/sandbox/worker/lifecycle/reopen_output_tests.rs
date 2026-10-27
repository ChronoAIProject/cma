use super::*;
use layer2_core::sandbox::store::SandboxMutationGuard;
use layer2_core::sandbox::{AgentUpdateOutcome, SandboxStatus, SandboxWorkStep};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

struct Recorder {
    entered: Notify,
    release: Notify,
    committed: Mutex<Vec<u64>>,
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
        panic!("startup must use lifecycle-fenced recording")
    }
    async fn record_reopen_update(
        &self,
        _: &AgentId,
        _: &AgentReopenGuard,
        _: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.entered.notify_one();
        self.release.notified().await;
        if self.refuse {
            return Err(AgentConversationError::StaleGeneration);
        }
        let ConversationUpdate::RuntimeOutput { receipt, .. } = update else {
            panic!("receipt required")
        };
        self.committed.lock().unwrap().push(receipt.source.cursor);
        Ok(AgentUpdateOutcome {
            appended: Vec::new(),
            turn_closed: false,
        })
    }
}

fn guard() -> AgentReopenGuard {
    AgentReopenGuard {
        sandbox: SandboxMutationGuard {
            sandbox_id: layer2_core::SandboxId("sbx_reopen".into()),
            owner: layer2_core::SubjectId::new("owner"),
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
        native_conversation_id: "original".into(),
        runtime_generation: 5,
    }
}

fn output(ack: mpsc::Sender<RuntimeOutputSource>) -> ConversationUpdate {
    ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor: 1,
                native_conversation_id: "original".into(),
                payload_sha256: "a".repeat(64),
            },
            ack,
        ),
        update: None,
    }
}

#[tokio::test]
async fn reopen_ack_follows_commit_even_when_handshake_then_fails() {
    let recorder = Arc::new(Recorder {
        entered: Notify::new(),
        release: Notify::new(),
        committed: Mutex::new(Vec::new()),
        refuse: false,
    });
    let (send, mut receiver) = mpsc::channel(1);
    let (ack, mut acknowledged) = mpsc::channel(1);
    send.send(output(ack)).await.unwrap();
    let (fail, failed) = tokio::sync::oneshot::channel();
    let task_recorder = recorder.clone();
    let task = tokio::spawn(async move {
        drain(
            async {
                failed.await.unwrap();
                Err::<(), _>(AdapterError::Unavailable("load failed after replay".into()))
            },
            &mut receiver,
            task_recorder.as_ref(),
            &AgentId("agt_reopen".into()),
            &guard(),
        )
        .await
    });
    recorder.entered.notified().await;
    assert!(
        acknowledged.try_recv().is_err(),
        "parsing must not release retained output"
    );
    assert!(recorder.committed.lock().unwrap().is_empty());
    recorder.release.notify_one();
    let ack = tokio::time::timeout(Duration::from_secs(2), acknowledged.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ack.cursor, 1);
    assert_eq!(*recorder.committed.lock().unwrap(), [1]);
    fail.send(()).unwrap();
    assert!(task.await.unwrap().is_err());
    assert_eq!(*recorder.committed.lock().unwrap(), [1]);
}

#[tokio::test]
async fn rejected_startup_commit_never_acknowledges_and_disconnect_ends_drain() {
    let recorder = Recorder {
        entered: Notify::new(),
        release: Notify::new(),
        committed: Mutex::new(Vec::new()),
        refuse: true,
    };
    recorder.release.notify_one();
    let (send, mut receiver) = mpsc::channel(1);
    let (ack, mut acknowledged) = mpsc::channel(1);
    send.send(output(ack)).await.unwrap();
    assert!(drain(
        std::future::pending::<layer3_adapter::Result<()>>(),
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard()
    )
    .await
    .is_err());
    assert!(acknowledged.try_recv().is_err());
    assert!(recorder.committed.lock().unwrap().is_empty());
    drop(send);
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        drain(
            std::future::pending::<layer3_adapter::Result<()>>(),
            &mut receiver,
            &recorder,
            &AgentId("agt_reopen".into()),
            &guard()
        )
    )
    .await
    .unwrap()
    .is_err());
}
