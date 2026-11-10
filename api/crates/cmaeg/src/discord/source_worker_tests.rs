use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tokio::sync::mpsc;

struct Port {
    revision: Mutex<Option<String>>,
    connects: AtomicUsize,
    attempts: mpsc::UnboundedSender<usize>,
    first: Mutex<Option<EndReason>>,
    stopped: AtomicUsize,
}
impl SourcePort for Port {
    fn revision<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async {
            self.revision
                .lock()
                .unwrap()
                .clone()
                .ok_or(GatewayError::Authority)
        })
    }
    fn connect<'a>(
        &'a self,
        _: &'a str,
        mut stop: watch::Receiver<bool>,
        _: super::super::status::StatusAttempt,
    ) -> BoxFuture<'a, Attempt> {
        Box::pin(async move {
            let n = self.connects.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self.attempts.send(n);
            let immediate = self.first.lock().unwrap().take();
            let reason = if let Some(reason) = immediate {
                reason
            } else {
                let _ = stop.changed().await;
                self.stopped.fetch_add(1, Ordering::SeqCst);
                EndReason::Stopped
            };
            Attempt {
                result: Ok(GatewayEnd {
                    reason,
                    checkpoint: None,
                    retry_after: Duration::from_millis(1),
                }),
                cooldown: Duration::ZERO,
            }
        })
    }
}
fn timing() -> Timing {
    Timing {
        poll: Duration::from_millis(5),
        retry: Duration::from_millis(1),
        stop: Duration::from_millis(100),
    }
}
fn fixture(first: Option<EndReason>) -> (Arc<Port>, mpsc::UnboundedReceiver<usize>) {
    let (attempts, rx) = mpsc::unbounded_channel();
    (
        Arc::new(Port {
            revision: Mutex::new(Some("initial".into())),
            connects: AtomicUsize::new(0),
            attempts,
            first: Mutex::new(first),
            stopped: AtomicUsize::new(0),
        }),
        rx,
    )
}
async fn next(rx: &mut mpsc::UnboundedReceiver<usize>) -> usize {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
#[tokio::test]
async fn reconnect_reacquires_and_shutdown_stops_the_owned_connection() {
    let (port, mut rx) = fixture(Some(EndReason::Reconnect));
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(
        port.clone(),
        "record".into(),
        stopped,
        timing(),
        super::super::status::StatusRegistry::default(),
    ));
    assert_eq!(next(&mut rx).await, 1);
    assert_eq!(next(&mut rx).await, 2);
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(port.stopped.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn source_revision_change_stops_old_socket_and_starts_current_revision() {
    let (port, mut rx) = fixture(None);
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(run(
        port.clone(),
        "record".into(),
        stopped,
        timing(),
        super::super::status::StatusRegistry::default(),
    ));
    assert_eq!(next(&mut rx).await, 1);
    *port.revision.lock().unwrap() = Some("rotated".into());
    assert_eq!(next(&mut rx).await, 2);
    assert_eq!(port.stopped.load(Ordering::SeqCst), 1);
    *port.revision.lock().unwrap() = None;
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(port.stopped.load(Ordering::SeqCst), 2);
    assert_eq!(port.connects.load(Ordering::SeqCst), 2);
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn terminal_authentication_error_requires_a_new_source_revision() {
    let (port, mut rx) = fixture(Some(EndReason::AuthenticationFailed));
    let (stop, stopped) = watch::channel(false);
    let registry = super::super::status::StatusRegistry::default();
    let task = tokio::spawn(run(
        port.clone(),
        "record".into(),
        stopped,
        timing(),
        registry.clone(),
    ));
    assert_eq!(next(&mut rx).await, 1);
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(port.connects.load(Ordering::SeqCst), 1);
    let view = registry.view("record", Some("initial"));
    assert_eq!(
        view.last_failure_code,
        Some(FailureCode::AuthenticationFailed)
    );
    assert_eq!(view.outcome, super::super::status::Outcome::Blocked);
    let failed_at = view.last_failure_at.clone();
    *port.revision.lock().unwrap() = None;
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(
        registry.view("record", Some("initial")).last_failure_code,
        Some(FailureCode::AuthorityRefused)
    );
    *port.revision.lock().unwrap() = Some("initial".into());
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(port.connects.load(Ordering::SeqCst), 1);
    assert_eq!(
        registry.view("record", Some("initial")).last_failure_code,
        Some(FailureCode::AuthenticationFailed)
    );
    assert_eq!(
        registry.view("record", Some("initial")).last_failure_at,
        failed_at
    );
    *port.revision.lock().unwrap() = Some("fixed-credential".into());
    assert_eq!(next(&mut rx).await, 2);
    assert_eq!(
        registry
            .view("record", Some("fixed-credential"))
            .last_failure_code,
        None
    );
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[test]
fn retry_delay_never_shortens_gateway_or_discovery_cooldown() {
    let attempt = Attempt {
        result: Ok(GatewayEnd {
            reason: EndReason::RateLimited,
            checkpoint: None,
            retry_after: Duration::from_secs(60),
        }),
        cooldown: Duration::from_secs(120),
    };
    assert_eq!(
        retry_delay(&attempt, 1, Duration::from_secs(5)),
        Duration::from_secs(120)
    );
    let busy = Attempt {
        result: Err(GatewayError::Authority),
        cooldown: Duration::ZERO,
    };
    assert_eq!(
        retry_delay(&busy, 1, Duration::from_secs(5)),
        Duration::from_secs(30)
    );
}

// Exercise resolution recovery through the worker, including invalidated handles.
struct HistoryPort {
    port: Arc<Port>,
    handles: Mutex<Vec<StatusAttempt>>,
}
impl SourcePort for HistoryPort {
    fn revision<'a>(&'a self, record: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        self.port.revision(record)
    }
    fn connect<'a>(
        &'a self,
        record: &'a str,
        stop: watch::Receiver<bool>,
        status: StatusAttempt,
    ) -> BoxFuture<'a, Attempt> {
        let mut handles = self.handles.lock().unwrap();
        if handles.is_empty() {
            status.lease_acquired(Duration::from_secs(30));
            status.heartbeat_ack(Duration::from_secs(30));
            status.ready();
            status.event_received();
            status.event_committed(true);
        }
        handles.push(status.clone());
        self.port.connect(record, stop, status)
    }
}
fn history(view: &NativeDiscordConnectionStatus) -> serde_json::Value {
    serde_json::json!([
        view.last_ready_at,
        view.last_event_at,
        view.last_event_committed_at,
        view.last_dispatch_at,
        view.last_dispatch_outcome,
    ])
}
async fn resolution_recovery(recovered_revision: &str) {
    let (port, mut rx) = fixture(None);
    let history_port = Arc::new(HistoryPort {
        port: port.clone(),
        handles: Mutex::new(Vec::new()),
    });
    let (stop, stopped) = watch::channel(false);
    let registry = StatusRegistry::default();
    let task = tokio::spawn(run(
        history_port.clone(),
        "record".into(),
        stopped,
        timing(),
        registry.clone(),
    ));
    assert_eq!(next(&mut rx).await, 1);
    registry.dispatch("record", "initial", "admitted");
    let before = registry.view("record", Some("initial"));
    assert_eq!(before.connected, Some(true));
    assert!(before.last_ready_at.is_some());
    assert!(before.last_event_at.is_some());
    assert!(before.last_event_committed_at.is_some());
    assert!(before.last_dispatch_at.is_some());
    *port.revision.lock().unwrap() = None;
    let failed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let view = registry.view("record", Some("initial"));
            if view.last_failure_code == Some(FailureCode::AuthorityRefused) {
                break view;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(history(&failed), history(&before));
    assert_eq!(failed.connected, Some(false));
    assert_eq!(failed.authenticated, Some(false));
    assert_eq!(failed.ready, Some(false));
    assert_eq!(
        failed.last_failure_stage,
        Some(super::super::status::Stage::Resolve)
    );
    assert!(failed.last_failure_at.is_some());
    *port.revision.lock().unwrap() = Some(recovered_revision.into());
    assert_eq!(next(&mut rx).await, 2);
    let after = registry.view("record", Some(recovered_revision));
    if recovered_revision == "initial" {
        assert_eq!(history(&after), history(&before));
        assert_eq!(after.last_failure_code, failed.last_failure_code);
        assert_eq!(after.last_failure_stage, failed.last_failure_stage);
        assert_eq!(after.last_failure_at, failed.last_failure_at);
    } else {
        assert_eq!(
            history(&after),
            serde_json::json!([null, null, null, null, "unknown"])
        );
        assert_eq!(after.last_failure_code, None);
        assert_eq!(after.last_failure_stage, None);
        assert_eq!(after.last_failure_at, None);
    }
    assert_eq!(after.connected, None);
    assert_eq!(after.authenticated, None);
    assert_eq!(after.ready, None);
    assert_eq!(after.lease, "unknown");
    let handles = history_port.handles.lock().unwrap().clone();
    handles[0].lease_acquired(Duration::from_secs(30));
    handles[0].heartbeat_ack(Duration::from_secs(30));
    handles[0].ready();
    handles[0].event_received();
    handles[0].event_committed(true);
    handles[0].failure(FailureCode::Protocol);
    handles[0].stopped();
    let mut unchanged = registry.view("record", Some(recovered_revision));
    // The public deadline is projected using wall time on each read.
    unchanged.valid_until = after.valid_until.clone();
    assert_eq!(
        serde_json::to_value(unchanged).unwrap(),
        serde_json::to_value(&after).unwrap(),
        "the old connection handle cannot alter the replacement attempt"
    );
    // READY alone cannot reuse the old attempt's lease or heartbeat deadline.
    handles[1].ready();
    assert_eq!(
        registry.view("record", Some(recovered_revision)).connected,
        None
    );
    handles[1].lease_acquired(Duration::from_secs(30));
    handles[1].heartbeat_ack(Duration::from_secs(30));
    handles[1].ready();
    assert_eq!(
        registry.view("record", Some(recovered_revision)).connected,
        Some(true)
    );
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn active_same_revision_resolution_recovery_retains_history() {
    resolution_recovery("initial").await;
}
#[tokio::test]
async fn different_revision_resolution_recovery_clears_history() {
    resolution_recovery("replacement").await;
}
