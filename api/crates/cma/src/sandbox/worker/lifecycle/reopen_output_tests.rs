use super::*;
use layer2_core::sandbox::store::SandboxMutationGuard;
use layer2_core::sandbox::{AgentUpdateOutcome, SandboxStatus, SandboxWorkStep};
use layer3_adapter::{RuntimeOutputReceipt, RuntimeOutputSource};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[path = "reopen_contention_tests.rs"]
mod contention;

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
async fn expired_reopen_authority_never_polls_a_ready_effect() {
    let recorder = Recorder {
        entered: Notify::new(),
        release: Notify::new(),
        committed: Mutex::new(Vec::new()),
        refuse: false,
    };
    let (_send, mut receiver) = mpsc::channel(1);
    let mut observed = guard();
    observed.sandbox.lease = Some(layer2_core::sandbox::SandboxWorkLease {
        holder: "expired-worker".into(),
        fence: 1,
        expires_at: "2000-01-01T00:00:00Z".into(),
    });
    let mut polls = 0;
    let effect = async {
        polls += 1;
        Ok(())
    };
    let result = drain_while(
        effect,
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &observed,
    )
    .await;
    assert_eq!(
        result,
        Err(AdapterError::Unavailable(
            "reopen output lifecycle lease expired".into()
        ))
    );
    assert_eq!(polls, 0);
    assert!(recorder.committed.lock().unwrap().is_empty());
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

#[tokio::test]
async fn failed_ack_preserves_available_open_failure_after_the_commit_finishes() {
    for outcome in [
        Err(AdapterError::Unsupported(
            "original tool receipt refusal".into(),
        )),
        Err(AdapterError::Rejected("original initialize refusal".into())),
        Ok(()),
    ] {
        let expected = outcome.clone().err().unwrap_or_else(|| {
            AdapterError::Unavailable("runtime acknowledgement link ended".into())
        });
        let recorder = Arc::new(Recorder {
            entered: Notify::new(),
            release: Notify::new(),
            committed: Mutex::new(Vec::new()),
            refuse: false,
        });
        let (send, mut receiver) = mpsc::channel(1);
        let (ack, acknowledged) = mpsc::channel(1);
        send.send(output(ack)).await.unwrap();
        let (complete, completed) = tokio::sync::oneshot::channel();
        let task_recorder = recorder.clone();
        let task = tokio::spawn(async move {
            drain(
                async { completed.await.unwrap() },
                &mut receiver,
                task_recorder.as_ref(),
                &AgentId("agt_reopen".into()),
                &guard(),
            )
            .await
        });
        recorder.entered.notified().await;
        drop(acknowledged);
        complete.send(outcome).unwrap();
        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "an admitted durable write must finish");
        assert!(recorder.committed.lock().unwrap().is_empty());
        recorder.release.notify_one();
        assert_eq!(task.await.unwrap(), Err(expected));
        assert_eq!(*recorder.committed.lock().unwrap(), [1]);
    }
}

#[tokio::test]
async fn failed_ack_does_not_wait_for_an_unresolved_open_or_publish_success() {
    let recorder = Recorder {
        entered: Notify::new(),
        release: Notify::new(),
        committed: Mutex::new(Vec::new()),
        refuse: false,
    };
    recorder.release.notify_one();
    let (send, mut receiver) = mpsc::channel(1);
    let (ack, acknowledged) = mpsc::channel(1);
    send.send(output(ack)).await.unwrap();
    drop(acknowledged);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        drain(
            std::future::pending::<layer3_adapter::Result<()>>(),
            &mut receiver,
            &recorder,
            &AgentId("agt_reopen".into()),
            &guard(),
        ),
    )
    .await
    .expect("custody failure cannot wait on an unresolved open");
    assert_eq!(
        result,
        Err(AdapterError::Unavailable(
            "runtime acknowledgement link ended".into()
        ))
    );
    assert_eq!(*recorder.committed.lock().unwrap(), [1]);
}

#[tokio::test]
async fn closed_update_receiver_preserves_an_open_failure_finishing_cleanup() {
    let recorder = Recorder {
        entered: Notify::new(),
        release: Notify::new(),
        committed: Mutex::new(Vec::new()),
        refuse: false,
    };
    let (send, mut receiver) = mpsc::channel(1);
    drop(send);
    let error = AdapterError::Unsupported("original open refusal".into());
    let result = drain(
        async {
            tokio::task::yield_now().await;
            Err::<(), _>(error.clone())
        },
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard(),
    )
    .await;
    assert_eq!(result, Err(error));
    assert!(recorder.committed.lock().unwrap().is_empty());
}

fn idle_recorder() -> Recorder {
    Recorder {
        entered: Notify::new(),
        release: Notify::new(),
        committed: Mutex::new(Vec::new()),
        refuse: false,
    }
}

#[tokio::test(start_paused = true)]
async fn a_relayed_reopen_outlives_its_call_snapshot_while_the_lease_stays_held() {
    let recorder = idle_recorder();
    let (_send, mut receiver) = mpsc::channel(1);
    let started = tokio::time::Instant::now();
    // Longer than any 15 s work-lease snapshot could allow.
    let opening = async {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        Ok("reopened")
    };
    let result = drain_while_held(
        opening,
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard(),
        || async { true },
    )
    .await;
    assert_eq!(result, Ok("reopened"));
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(20));
}

#[tokio::test(start_paused = true)]
async fn a_relayed_reopen_stops_promptly_once_the_caller_lease_is_lost() {
    let recorder = idle_recorder();
    let (_send, mut receiver) = mpsc::channel(1);
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let started = tokio::time::Instant::now();
    let result = drain_while_held(
        std::future::pending::<layer3_adapter::Result<()>>(),
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard(),
        || {
            let held = checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 3;
            async move { held }
        },
    )
    .await;
    assert_eq!(
        result,
        Err(AdapterError::Unavailable(
            "reopen output lifecycle lease expired".into()
        ))
    );
    // The initial check plus two one-second watches held; the third refused.
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn a_relayed_reopen_never_polls_once_its_lease_is_already_gone() {
    let recorder = idle_recorder();
    let (_send, mut receiver) = mpsc::channel(1);
    let mut polls = 0;
    let effect = async {
        polls += 1;
        Ok(())
    };
    let result = drain_while_held(
        effect,
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard(),
        || async { false },
    )
    .await;
    assert_eq!(
        result,
        Err(AdapterError::Unavailable(
            "reopen output lifecycle lease expired".into()
        ))
    );
    assert_eq!(polls, 0);
}

#[tokio::test(start_paused = true)]
async fn a_relayed_reopen_is_capped_below_the_relay_timeout() {
    let recorder = idle_recorder();
    let (_send, mut receiver) = mpsc::channel(1);
    let started = tokio::time::Instant::now();
    let result = drain_while_held(
        std::future::pending::<layer3_adapter::Result<()>>(),
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard(),
        || async { true },
    )
    .await;
    assert_eq!(
        result,
        Err(AdapterError::Unavailable(
            "reopen output operation timed out".into()
        ))
    );
    assert_eq!(started.elapsed(), RELAYED_REOPEN_LIMIT);
    assert!(RELAYED_REOPEN_LIMIT < std::time::Duration::from_secs(60));
}
