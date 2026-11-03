use super::*;
use layer2_core::sandbox::AgentUpdateOutcome;
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Default)]
struct Recorder {
    committed: Mutex<Vec<u64>>,
    entered: Notify,
    release: Notify,
    blocked: bool,
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
        assert_eq!(generation, 7);
        self.entered.notify_one();
        if self.blocked {
            self.release.notified().await;
        }
        if self.refuse {
            return Err(AgentConversationError::StaleGeneration);
        }
        let ConversationUpdate::RuntimeOutput { receipt, .. } = update else {
            panic!("expected reliable output")
        };
        self.committed.lock().unwrap().push(receipt.source.cursor);
        Ok(AgentUpdateOutcome {
            appended: Vec::new(),
            turn_closed: false,
        })
    }
}

fn output(
    cursor: u64,
    conversation: &str,
    ack: mpsc::Sender<RuntimeOutputSource>,
) -> ConversationUpdate {
    ConversationUpdate::RuntimeOutput {
        receipt: RuntimeOutputReceipt::new(
            RuntimeOutputSource {
                stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                cursor,
                native_conversation_id: conversation.into(),
                payload_sha256: format!("{cursor:064x}"),
            },
            ack,
        ),
        update: None,
    }
}

#[tokio::test]
async fn quiet_reconnect_drains_more_than_channel_capacity_before_open_completes() {
    let recorder = Arc::new(Recorder::default());
    let (send, mut receiver) = mpsc::channel(256);
    let (ack, mut acknowledged) = mpsc::channel(1);
    let observed = recorder.clone();
    let opening = async move {
        for cursor in 1..=1024 {
            send.send(output(cursor, "original", ack.clone()))
                .await
                .unwrap();
            let receipt = acknowledged
                .recv()
                .await
                .expect("retained output must be acknowledged");
            assert_eq!(receipt.cursor, cursor);
            assert_eq!(observed.committed.lock().unwrap().last(), Some(&cursor));
        }
        Ok("original")
    };
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        drain(
            opening,
            &mut receiver,
            recorder.as_ref(),
            &AgentId("agt_replay".into()),
            7,
            "original",
        ),
    )
    .await
    .expect("opening cannot wait for publication to release its output buffer")
    .unwrap();
    assert_eq!(result, "original");
    assert_eq!(
        *recorder.committed.lock().unwrap(),
        (1..=1024).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn quiet_replay_ack_waits_for_commit_and_refusal_retains_output() {
    for refuse in [false, true] {
        let recorder = Arc::new(Recorder {
            blocked: true,
            refuse,
            ..Recorder::default()
        });
        let (send, mut receiver) = mpsc::channel(1);
        let (ack, mut acknowledged) = mpsc::channel(1);
        send.send(output(1, "original", ack)).await.unwrap();
        let task_recorder = recorder.clone();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            drain(
                async {
                    finished.await.unwrap();
                    Ok(())
                },
                &mut receiver,
                task_recorder.as_ref(),
                &AgentId("agt_replay".into()),
                7,
                "original",
            )
            .await
        });
        recorder.entered.notified().await;
        assert!(acknowledged.try_recv().is_err());
        assert!(recorder.committed.lock().unwrap().is_empty());
        recorder.release.notify_one();
        if refuse {
            assert!(task.await.unwrap().is_err());
            assert!(acknowledged.try_recv().is_err());
            assert!(recorder.committed.lock().unwrap().is_empty());
        } else {
            assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
            finish.send(()).unwrap();
            task.await.unwrap().unwrap();
            assert_eq!(*recorder.committed.lock().unwrap(), [1]);
        }
    }
}

#[tokio::test]
async fn quiet_replay_rejects_another_conversation_and_output_gaps_without_ack() {
    let recorder = Recorder::default();
    let (ack, mut acknowledged) = mpsc::channel(1);
    for update in [
        output(1, "other", ack),
        ConversationUpdate::Lost { frames: 1 },
    ] {
        let (send, mut receiver) = mpsc::channel(1);
        send.send(update).await.unwrap();
        assert!(drain(
            std::future::pending::<Result<(), WorkerFailure>>(),
            &mut receiver,
            &recorder,
            &AgentId("agt_replay".into()),
            7,
            "original"
        )
        .await
        .is_err());
    }
    assert!(acknowledged.try_recv().is_err());
    assert!(recorder.committed.lock().unwrap().is_empty());
}
