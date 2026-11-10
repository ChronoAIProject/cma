use super::*;
use crate::discord::status::{FailureCode, Outcome, Stage, StatusRegistry};
use std::time::Duration;

fn observed() -> (StatusRegistry, crate::discord::status::StatusAttempt) {
    let registry = StatusRegistry::default();
    let status = registry.begin("record", Some("revision"));
    status.lease_acquired(Duration::from_secs(30));
    status.identify_reserved(false);
    (registry, status)
}
async fn snapshot_after_commit(
    registry: &StatusRegistry,
) -> crate::discord::status::NativeDiscordConnectionStatus {
    // Journal notification precedes the gateway's handling of its commit result.
    for _ in 0..100 {
        let view = registry.view("record", Some("revision"));
        if view.outcome == Outcome::Ok && view.ready == Some(true) {
            return view;
        }
        tokio::task::yield_now().await;
    }
    panic!("committed READY was not observed")
}
#[tokio::test]
async fn sent_identify_ack_and_checkpoint_do_not_authenticate_before_committed_ready() {
    let journal = Arc::new(Journal::default());
    let (registry, status) = observed();
    let (mut socket, stop, task) = start_observed(journal.clone(), None, 999, status).await;
    assert_eq!(hello(&mut socket).await["op"], 2);
    send(&mut socket, json!({"op":11,"d":null})).await;
    let view = registry.view("record", Some("revision"));
    assert_eq!(view.identify, "sent");
    assert_eq!(view.authenticated, None);
    ready(&mut socket).await;
    journal.wait(1).await;
    let view = snapshot_after_commit(&registry).await;
    assert_eq!(view.connected, Some(true));
    assert!(view.last_ready_at.is_some());
    message(&mut socket, 1, "100").await;
    journal.wait(2).await;
    let view = snapshot_after_commit(&registry).await;
    assert!(view.last_event_at.is_some());
    assert!(view.last_event_committed_at.is_some());
    stop.send(true).unwrap();
    let ended = task.await.unwrap();
    let checkpoint = ended.checkpoint.unwrap();
    let resumed = registry.begin("record", Some("revision"));
    resumed.lease_acquired(Duration::from_secs(30));
    resumed.identify_reserved(true);
    let (mut socket, stop, task) =
        start_observed(journal.clone(), Some(checkpoint), 999, resumed).await;
    assert_eq!(hello(&mut socket).await["op"], 6);
    let view = registry.view("record", Some("revision"));
    assert_eq!(view.identify, "resume_sent");
    assert_eq!(view.connected, None);
    send(&mut socket, json!({"op":0,"s":2,"t":"RESUMED","d":{}})).await;
    journal.wait(3).await;
    assert_eq!(
        snapshot_after_commit(&registry).await.authenticated,
        Some(true)
    );
    stop.send(true).unwrap();
    task.await.unwrap();
}
#[tokio::test]
async fn failed_message_commit_keeps_received_and_committed_status_distinct() {
    let journal = Arc::new(Journal::default());
    let (registry, status) = observed();
    let (mut socket, _stop, task) =
        start_observed(journal.clone(), None, 999, status.clone()).await;
    hello(&mut socket).await;
    ready(&mut socket).await;
    journal.wait(1).await;
    snapshot_after_commit(&registry).await;
    journal.fail_messages.store(true, Ordering::SeqCst);
    message(&mut socket, 1, "100").await;
    let end = task.await.unwrap();
    // The real connection supervisor consumes this end reason the same way.
    status.finish(end.reason.into(), !end.retryable(), end.retry_after);
    let view = registry.view("record", Some("revision"));
    assert_eq!(view.connected, Some(false));
    assert_eq!(view.last_failure_stage, Some(Stage::EventCommit));
    assert_eq!(view.last_failure_code, Some(FailureCode::CommitUnavailable));
    assert!(view.last_event_at.is_some());
    assert!(view.last_event_committed_at.is_none());
}
#[tokio::test(start_paused = true)]
async fn hello_timeout_retains_specific_reason_instead_of_generic_protocol_error() {
    let (registry, status) = observed();
    let (_socket, _stop, task) = start_observed(Arc::default(), None, 999, status.clone()).await;
    let end = task.await.unwrap();
    assert_eq!(end.reason, EndReason::Protocol);
    status.finish(end.reason.into(), true, end.retry_after);
    let view = registry.view("record", Some("revision"));
    assert_eq!(view.last_failure_stage, Some(Stage::Hello));
    assert_eq!(view.last_failure_code, Some(FailureCode::HelloTimeout));
    assert_eq!(view.outcome, Outcome::Blocked);
}
#[tokio::test]
async fn gateway_close_reports_disallowed_intents_without_close_text() {
    let (registry, status) = observed();
    let (mut socket, _stop, task) = start_observed(Arc::default(), None, 999, status.clone()).await;
    hello(&mut socket).await;
    socket
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(4014),
            reason: "private-close-text".into(),
        })))
        .await
        .unwrap();
    let end = task.await.unwrap();
    status.finish(end.reason.into(), !end.retryable(), end.retry_after);
    let view = registry.view("record", Some("revision"));
    assert_eq!(view.last_failure_code, Some(FailureCode::DisallowedIntents));
    assert_eq!(view.outcome, Outcome::Blocked);
    assert!(!serde_json::to_string(&view)
        .unwrap()
        .contains("private-close-text"));
}
