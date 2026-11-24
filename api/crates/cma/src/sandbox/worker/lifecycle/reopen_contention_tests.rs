use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct ContendedRecorder {
    writer: tokio::sync::Mutex<()>,
    entered: Notify,
    attempts: AtomicUsize,
    committed: AtomicUsize,
    unavailable_once: bool,
    refuse: bool,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for ContendedRecorder {
    async fn record_update(
        &self,
        _: &AgentId,
        _: u64,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        panic!("reopen must retain lifecycle-fenced recording")
    }

    async fn record_reopen_update(
        &self,
        _: &AgentId,
        _: &AgentReopenGuard,
        _: &mut AgentMessageAssembly,
        _: ConversationUpdate,
        _: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.unavailable_once && attempt == 0 {
            return Err(AgentConversationError::Unavailable("retry storage".into()));
        }
        let _held = self.writer.lock().await;
        if self.refuse {
            return Err(AgentConversationError::StaleGeneration);
        }
        self.committed.fetch_add(1, Ordering::SeqCst);
        Ok(AgentUpdateOutcome {
            appended: Vec::new(),
            turn_closed: false,
        })
    }
}

#[tokio::test]
async fn reopen_confirmation_releases_the_writer_for_output_and_accepted_stop() {
    for outcome in [
        Ok(()),
        Err(AdapterError::Rejected("confirmation refused".into())),
    ] {
        let recorder = ContendedRecorder::default();
        let (send, mut receiver) = mpsc::channel(1);
        let (ack, mut acknowledged) = mpsc::channel(1);
        let stop_waiting = Notify::new();
        let opening = async {
            let held = recorder.writer.lock().await;
            send.send(output(ack)).await.unwrap();
            recorder.entered.notified().await;
            stop_waiting.notify_one();
            // The Mongo completion wakes this future. It must be polled to
            // release the same Agent writer used by replay and stop publication.
            tokio::task::yield_now().await;
            drop(held);
            outcome.clone()
        };
        let stop = async {
            stop_waiting.notified().await;
            let _held = recorder.writer.lock().await;
            "stop publication can proceed"
        };
        let (result, stopped) = tokio::time::timeout(Duration::from_secs(1), async {
            let agent = AgentId("agt_reopen".into());
            let guard = guard();
            tokio::join!(
                drain(opening, &mut receiver, &recorder, &agent, &guard, false),
                stop,
            )
        })
        .await
        .expect("reopen and stop must not wait for the lifecycle lease to expire");
        assert_eq!(result, outcome);
        assert_eq!(stopped, "stop publication can proceed");
        assert_eq!(recorder.committed.load(Ordering::SeqCst), 1);
        assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
        assert!(acknowledged.try_recv().is_err(), "do not ACK twice");
    }
}

#[tokio::test(start_paused = true)]
async fn reopen_retries_the_receipt_without_repolling_completed_confirmation() {
    let recorder = ContendedRecorder {
        unavailable_once: true,
        ..Default::default()
    };
    let (send, mut receiver) = mpsc::channel(1);
    let (ack, mut acknowledged) = mpsc::channel(1);
    let opening = async {
        let held = recorder.writer.lock().await;
        send.send(output(ack)).await.unwrap();
        recorder.entered.notified().await;
        drop(held);
        Ok("original")
    };
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        drain(
            opening,
            &mut receiver,
            &recorder,
            &AgentId("agt_reopen".into()),
            &guard(),
            false,
        ),
    )
    .await
    .expect("transient recording failure must retry without starving confirmation")
    .unwrap();
    assert_eq!(result, "original");
    assert_eq!(recorder.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(recorder.committed.load(Ordering::SeqCst), 1);
    assert_eq!(acknowledged.recv().await.unwrap().cursor, 1);
    assert!(acknowledged.try_recv().is_err());
}

#[tokio::test]
async fn concurrent_open_completion_cannot_hide_a_refused_receipt() {
    for outcome in [
        Ok(()),
        Err(AdapterError::Rejected("original refusal".into())),
    ] {
        let recorder = ContendedRecorder {
            refuse: true,
            ..Default::default()
        };
        let (send, mut receiver) = mpsc::channel(1);
        let (ack, mut acknowledged) = mpsc::channel(1);
        let opening = async {
            let held = recorder.writer.lock().await;
            send.send(output(ack)).await.unwrap();
            recorder.entered.notified().await;
            drop(held);
            outcome.clone()
        };
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            drain(
                opening,
                &mut receiver,
                &recorder,
                &AgentId("agt_reopen".into()),
                &guard(),
                false,
            ),
        )
        .await
        .expect("a refused receipt must return without retaining the writer");
        match outcome {
            Err(original) => assert_eq!(result, Err(original)),
            Ok(()) => assert!(matches!(result, Err(AdapterError::Unavailable(_)))),
        }
        assert_eq!(recorder.committed.load(Ordering::SeqCst), 0);
        assert!(acknowledged.try_recv().is_err());
        assert!(recorder.writer.try_lock().is_ok());
    }
}

#[tokio::test(start_paused = true)]
async fn expired_reopen_releases_the_writer_without_acknowledging_uncommitted_output() {
    let recorder = ContendedRecorder::default();
    let mut guard = guard();
    guard.sandbox.lease = Some(layer2_core::sandbox::SandboxWorkLease {
        holder: "reopen-worker".into(),
        fence: 1,
        expires_at: layer2_core::rfc3339_at(std::time::SystemTime::now() + Duration::from_secs(2)),
    });
    let (send, mut receiver) = mpsc::channel(1);
    let (ack, mut acknowledged) = mpsc::channel(1);
    let opening = async {
        let _held = recorder.writer.lock().await;
        send.send(output(ack)).await.unwrap();
        std::future::pending::<layer3_adapter::Result<()>>().await
    };
    let started = tokio::time::Instant::now();
    let result = drain_while(
        opening,
        &mut receiver,
        &recorder,
        &AgentId("agt_reopen".into()),
        &guard,
        false,
    )
    .await;
    assert_eq!(
        result,
        Err(AdapterError::Unavailable(
            "reopen output lifecycle lease expired".into()
        ))
    );
    assert!(started.elapsed() <= Duration::from_secs(2));
    assert_eq!(recorder.committed.load(Ordering::SeqCst), 0);
    assert!(acknowledged.try_recv().is_err());
    assert!(
        recorder.writer.try_lock().is_ok(),
        "stop can acquire the writer"
    );
}
