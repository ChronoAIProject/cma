use super::*;
use std::io::Write;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
struct EventCapture(Arc<Mutex<Vec<u8>>>);

impl<'a> MakeWriter<'a> for EventCapture {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for EventCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl EventCapture {
    fn events(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter(|line| line.contains("event=\"agent_create_completed\""))
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ordinary_completion_is_after_ready_has_open_phases_and_does_not_repeat() {
    let capture = EventCapture(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("create-completion-test", vec![]).await;
    for _ in 0..6 {
        let current = harness.stored(&admitted.id).await;
        if current.work.step == AgentWorkStep::PublishReady {
            break;
        }
        assert_eq!(current.status, AgentStatus::Starting);
        assert!(harness
            .worker
            .run_one(&admitted.id, harness.now)
            .await
            .unwrap());
    }
    let before = harness.stored(&admitted.id).await;
    assert_eq!(before.status, AgentStatus::Starting);
    assert_eq!(before.work.step, AgentWorkStep::PublishReady);
    tokio::task::yield_now().await;
    assert!(capture.events().is_empty());

    assert!(harness
        .worker
        .run_one(&admitted.id, harness.now)
        .await
        .unwrap());
    assert_eq!(
        harness.stored(&admitted.id).await.status,
        AgentStatus::Ready
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while capture.events().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("completion task must emit after the acknowledged Ready commit");
    let events = capture.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert!(event.contains("ready_path=\"ordinary\""));
    assert!(event.contains("boundary=\"ready_commit_ack\""));
    assert!(event.contains("open_conversation_ms="));
    assert!(event.contains("open_runtime_ms="));
    assert!(event.contains("timing_valid=false"));
    assert!(!event.contains("total_ms="));

    harness
        .worker
        .run_scheduled(&admitted.id, harness.now)
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        capture.events().len(),
        1,
        "connection continuation is not another create"
    );
}
