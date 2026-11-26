use super::*;
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

fn ready(registry: &StatusRegistry) -> StatusAttempt {
    let attempt = registry.begin("private-record", Some("private-revision"));
    attempt.lease_acquired(Duration::from_secs(30));
    attempt.identify_reserved(false);
    attempt.identify_sent();
    attempt.heartbeat_ack(Duration::from_secs(2));
    attempt.ready();
    attempt
}
#[tokio::test(start_paused = true)]
async fn health_requires_ready_and_both_fresh_lease_and_heartbeat() {
    let r = StatusRegistry::default();
    assert_eq!(r.view("private-record", None).connected, None);
    let a = r.begin("private-record", Some("private-revision"));
    a.lease_acquired(Duration::from_secs(30));
    a.identify_reserved(false);
    a.identify_sent();
    a.heartbeat_ack(Duration::from_secs(2));
    assert_eq!(
        r.view("private-record", Some("private-revision"))
            .authenticated,
        None
    );
    a.ready();
    assert_eq!(
        r.view("private-record", Some("private-revision"))
            .authenticated,
        Some(true)
    );
    tokio::time::advance(Duration::from_secs(3)).await;
    a.lease_renewed(Duration::from_secs(30));
    let stale = r.view("private-record", Some("private-revision"));
    assert!(stale.stale);
    assert_eq!(stale.connected, None);
    a.heartbeat_ack(Duration::from_secs(120));
    tokio::time::advance(Duration::from_secs(31)).await;
    a.poll();
    assert!(r.view("private-record", Some("private-revision")).stale);
}
#[tokio::test(start_paused = true)]
async fn stale_attempt_revision_and_removed_source_cannot_claim_health() {
    let r = StatusRegistry::default();
    let old = ready(&r);
    assert_eq!(
        r.view("private-record", Some("new-revision")).connected,
        None
    );
    r.remove("private-record");
    let new = r.begin("private-record", Some("new-revision"));
    old.ready();
    old.lease_renewed(Duration::from_secs(30));
    old.event_committed(true);
    assert_eq!(r.view("private-record", Some("new-revision")).ready, None);
    new.finish(
        FailureCode::DisallowedIntents,
        true,
        Duration::from_secs(60),
    );
    old.stopped();
    let view = r.view("private-record", Some("new-revision"));
    assert_eq!(view.outcome, Outcome::Blocked);
    assert_eq!(view.last_failure_code, Some(FailureCode::DisallowedIntents));
    assert!(view.retry_at.is_none());
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(r.view("private-record", Some("new-revision")).stale);
    new.poll();
    assert!(!r.view("private-record", Some("new-revision")).stale);
}
#[test]
fn cleanup_preserves_primary_reason_and_dispatch_does_not_change_connection_health() {
    let r = StatusRegistry::default();
    let a = ready(&r);
    a.event_received();
    let view = r.view("private-record", Some("private-revision"));
    assert!(view.last_event_at.is_some());
    assert!(view.last_event_committed_at.is_none());
    a.event_committed(true);
    r.dispatch("private-record", "private-revision", "pending");
    let view = r.view("private-record", Some("private-revision"));
    assert_eq!(view.connected, Some(true));
    assert_eq!(view.last_dispatch_outcome, "pending");
    a.step(Stage::Identify);
    a.failure(FailureCode::DisallowedIntents);
    a.cleanup_failed();
    a.lease_released(false);
    a.finish(FailureCode::Transport, true, Duration::ZERO);
    a.heartbeat_ack(Duration::from_secs(30));
    a.ready();
    let view = r.view("private-record", Some("private-revision"));
    assert_eq!(view.last_failure_stage, Some(Stage::Identify));
    assert_eq!(view.last_failure_code, Some(FailureCode::DisallowedIntents));
    assert_eq!(view.connected, Some(false));
    assert_eq!(view.lease, "lost");
}
#[test]
fn registry_is_bounded_and_new_selection_can_reuse_removed_capacity() {
    let r = StatusRegistry::default();
    for n in 0..64 {
        r.begin(&format!("record-{n}"), Some("revision")).finish(
            FailureCode::Unavailable,
            false,
            Duration::ZERO,
        );
    }
    r.begin("overflow", Some("revision")).ready();
    assert_eq!(
        r.view("overflow", Some("revision")).stage,
        Stage::Unobserved
    );
    r.remove("record-0");
    r.begin("overflow", Some("revision"))
        .finish(FailureCode::Unavailable, false, Duration::ZERO);
    assert_eq!(
        r.view("overflow", Some("revision")).last_failure_code,
        Some(FailureCode::Unavailable)
    );
}
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn logs_are_bounded_and_detached_from_sensitive_parent_spans() {
    static KEEP: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
    KEEP.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
    let capture = Capture::default();
    let sink = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let parent = tracing::info_span!(
            "sensitive",
            owner = "private-owner",
            token = "private-token",
            session = "private-session",
            body = "private-message"
        );
        let _entered = parent.enter();
        let r = StatusRegistry::default();
        for n in 0..1000 {
            let a = r.begin("private-record", Some("private-revision"));
            a.step(Stage::Identify);
            a.finish(
                if n % 2 == 0 {
                    FailureCode::DisallowedIntents
                } else {
                    FailureCode::Protocol
                },
                true,
                Duration::ZERO,
            );
            a.cleanup_failed();
        }
        let wire =
            serde_json::to_string(&r.view("private-record", Some("private-revision"))).unwrap();
        assert!(!wire.contains("private-"));
    });
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.is_empty());
    assert!(logs.lines().count() <= 64);
    assert!(logs.contains("native Discord connection state changed"));
    assert!(!logs.contains("private-"));
    assert!(!logs.contains("sensitive"));
}

#[test]
fn failure_code_is_fenced_to_the_current_failed_attempt() {
    let registry = StatusRegistry::default();
    let old = registry.begin("record", Some("revision"));
    old.step(Stage::Lease);
    old.lease_refused(FailureCode::LeaseContended);
    assert_eq!(old.failure_code(), Some(FailureCode::LeaseContended));
    let current = registry.begin("record", Some("revision"));
    assert_eq!(old.failure_code(), None);
    assert_eq!(current.failure_code(), None);
    assert_eq!(
        registry.view("record", Some("revision")).last_failure_code,
        Some(FailureCode::LeaseContended)
    );
    current.failure(FailureCode::AuthorityRefused);
    assert_eq!(current.failure_code(), Some(FailureCode::AuthorityRefused));
    registry.remove("record");
    assert_eq!(current.failure_code(), None);
    assert_eq!(StatusAttempt::default().failure_code(), None);
}

#[test]
fn ready_failure_and_cleanup_logs_carry_http_convention_record_hash() {
    let capture = Capture::default();
    let sink = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let parent = tracing::info_span!("sensitive", token = "private-token");
        let _entered = parent.enter();
        let r = StatusRegistry::default();
        ready(&r);
        let a = r.begin("other-private-record", Some("private-revision"));
        a.finish(
            FailureCode::AuthorityRefused,
            false,
            Duration::from_secs(30),
        );
        a.cleanup_failed();
    });
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let state: Vec<_> = logs
        .lines()
        .filter(|line| line.contains("native Discord connection state changed"))
        .collect();
    assert_eq!(state.len(), 2, "{logs}");
    for (line, record) in [
        (state[0], "private-record"),
        (state[1], "other-private-record"),
    ] {
        let expected = crate::telemetry::http::correlation_hash(record.as_bytes());
        assert_eq!(expected.len(), 64);
        assert!(line.contains(&format!("record_hash={expected}")), "{line}");
        assert!(!line.contains("channel_hash="), "{line}");
    }
    assert!(state[0].contains("stage=\"ready\""), "{logs}");
    assert!(state[0].contains("outcome=\"ok\""), "{logs}");
    assert!(state[1].contains("stage=\"resolve\""), "{logs}");
    assert!(state[1].contains("authority_refused"), "{logs}");
    let cleanup = logs
        .lines()
        .find(|line| line.contains("native Discord connection cleanup failed"))
        .unwrap();
    assert!(
        cleanup.contains(&format!(
            "record_hash={}",
            crate::telemetry::http::correlation_hash(b"other-private-record")
        )),
        "{cleanup}"
    );
    assert!(!logs.contains("private-"), "{logs}");
    assert!(!logs.contains("sensitive"), "{logs}");
}
// #3745 observed stage ready, outcome ok, lease acquired after a reconnect
// failure. A failed attempt must never keep reporting that it is serving.
#[tokio::test]
async fn failure_after_ready_never_reports_ok_before_cleanup_finishes() {
    let r = StatusRegistry::default();
    let a = ready(&r);
    a.failure(FailureCode::Reconnect);
    let view = r.view("private-record", Some("private-revision"));
    assert_eq!(view.stage, Stage::Ready);
    assert_eq!(view.outcome, Outcome::Pending);
    assert_eq!(view.connected, Some(false));
    assert_eq!(view.last_failure_code, Some(FailureCode::Reconnect));
}
