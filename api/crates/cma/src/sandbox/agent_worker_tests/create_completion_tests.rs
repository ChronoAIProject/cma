use super::*;
use layer2_core::sandbox::store::{AgentCreateAdmission, SandboxStoreError};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;
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

struct PendingAdmissions {
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    released: AtomicBool,
    return_error: AtomicBool,
    wake: Notify,
}

struct ActiveLookup(Arc<PendingAdmissions>);

impl Drop for ActiveLookup {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PendingAdmissions {
    fn record_active(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let mut maximum = self.max_active.load(Ordering::SeqCst);
        while active > maximum {
            match self.max_active.compare_exchange(
                maximum,
                active,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(observed) => maximum = observed,
            }
        }
    }

    async fn lookup(
        self: Arc<Self>,
        _agent: Agent,
    ) -> Result<Option<AgentCreateAdmission>, SandboxStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.record_active();
        let _active = ActiveLookup(self.clone());
        while !self.released.load(Ordering::SeqCst) {
            let notified = self.wake.notified();
            if self.released.load(Ordering::SeqCst) {
                break;
            }
            notified.await;
        }
        if self.return_error.load(Ordering::SeqCst) {
            Err(SandboxStoreError::Unavailable(
                "test admission failure".into(),
            ))
        } else {
            Ok(None)
        }
    }

    fn release_with_error(&self) {
        self.return_error.store(true, Ordering::SeqCst);
        self.released.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }

    fn hold(&self) {
        self.return_error.store(false, Ordering::SeqCst);
        self.released.store(false, Ordering::SeqCst);
    }

    fn release_success(&self) {
        self.return_error.store(false, Ordering::SeqCst);
        self.released.store(true, Ordering::SeqCst);
        self.wake.notify_waiters();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn admission_lookup_saturation_keeps_ready_path_nonwaiting_and_falls_back_once() {
    let capture = EventCapture(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admissions = Arc::new(PendingAdmissions {
        calls: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
        released: AtomicBool::new(false),
        return_error: AtomicBool::new(false),
        wake: Notify::new(),
    });
    let pending = admissions.clone();
    harness
        .worker
        .set_completion_admission_lookup(move |agent| pending.clone().lookup(agent));
    let agent = harness.admit("held-admission", vec![]).await;

    for _ in 0..33 {
        harness
            .worker
            .schedule_completion_event_for_test(agent.clone());
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while admissions.calls.load(Ordering::SeqCst) < 32 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first 32 admission reads should be pending");
    assert_eq!(admissions.calls.load(Ordering::SeqCst), 32);
    assert_eq!(admissions.max_active.load(Ordering::SeqCst), 32);

    // The 33rd completion is emitted synchronously on saturation, proving
    // that a held admission read cannot delay the Ready caller.
    assert_eq!(capture.events().len(), 1);
    assert!(capture.events()[0].contains("timing_valid=false"));
    assert!(!capture.events()[0].contains("total_ms="));

    admissions.release_with_error();
    tokio::time::timeout(Duration::from_secs(1), async {
        while capture.events().len() < 33 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending admission failures should emit one unavailable event each");
    tokio::task::yield_now().await;
    let events = capture.events();
    assert_eq!(events.len(), 33);
    assert!(events
        .iter()
        .all(|event| { event.contains("timing_valid=false") && !event.contains("total_ms=") }));
    assert_eq!(admissions.active.load(Ordering::SeqCst), 0);
    assert_eq!(admissions.calls.load(Ordering::SeqCst), 32);

    // A completed error releases its permit for the next completion.
    harness
        .worker
        .schedule_completion_event_for_test(agent.clone());
    tokio::time::timeout(Duration::from_secs(1), async {
        while admissions.calls.load(Ordering::SeqCst) < 33 || capture.events().len() < 34 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a post-error lookup should acquire a released permit");
    assert_eq!(admissions.calls.load(Ordering::SeqCst), 33);

    // Hold one lookup past the production deadline. Its cancellation guard
    // releases the permit, and timeout emits exactly one unavailable event.
    admissions.hold();
    harness
        .worker
        .schedule_completion_event_for_test(agent.clone());
    tokio::time::timeout(Duration::from_secs(1), async {
        while admissions.calls.load(Ordering::SeqCst) < 34 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the timeout case should start a fresh admission lookup");
    tokio::time::timeout(Duration::from_secs(1), async {
        while capture.events().len() < 35 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the held lookup should emit after the whole-read timeout");
    assert_eq!(admissions.active.load(Ordering::SeqCst), 0);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        capture.events().len(),
        35,
        "timeout must not emit a late duplicate"
    );

    // The permit released by timeout is reusable too.
    admissions.release_success();
    harness.worker.schedule_completion_event_for_test(agent);
    tokio::time::timeout(Duration::from_secs(1), async {
        while admissions.calls.load(Ordering::SeqCst) < 35 || capture.events().len() < 36 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a post-timeout lookup should acquire a released permit");
    assert_eq!(admissions.calls.load(Ordering::SeqCst), 35);
    assert_eq!(admissions.active.load(Ordering::SeqCst), 0);
    let events = capture.events();
    assert_eq!(events.len(), 36);
    assert!(events
        .iter()
        .all(|event| { event.contains("timing_valid=false") && !event.contains("total_ms=") }));
}
