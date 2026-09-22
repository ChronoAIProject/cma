use super::{activity::command, fixture, get};
use axum::http::StatusCode;
use layer2_core::{bridge::status::*, sandbox::agent_io::*};
use std::sync::atomic::Ordering;

fn queued(f: &fixture::Fixture, order: u64) -> PartitionCommandEvidence {
    command(
        f,
        order,
        AgentResponseStatus::Queued,
        ChannelCommandHold::Released,
        None,
    )
}
fn install(f: &fixture::Fixture, pending: Vec<PartitionCommandEvidence>) {
    *f.memory.activity.lock().unwrap() = PartitionActivityEvidence {
        latest: pending.last().cloned(),
        pending,
        observed_at_ms: 1000,
    };
}
fn path(f: &fixture::Fixture) -> String {
    format!("/{}/pending-messages", f.allocation)
}

#[tokio::test]
async fn pending_messages_return_only_authorized_unstarted_user_text() {
    let f = fixture::fixture(false).await;
    let queued = queued(&f, 1);
    let started = command(
        &f,
        2,
        AgentResponseStatus::InProgress,
        ChannelCommandHold::Released,
        Some(7),
    );
    install(&f, vec![queued, started]);
    let (status, body) = get(&f, "owner-a", &path(&f)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "available");
    assert_eq!(body["partition_id"], f.allocation);
    assert!(body["through_sequence"].is_u64());
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(body["messages"][0]["command_id"], "cmd_1");
    assert_eq!(body["messages"][0]["text"], "private fixture input");
    for secret in [
        "private-token-marker",
        "body_sha256",
        "custody",
        "key_id",
        "grant_id",
        "chat_id",
        "sender_id",
    ] {
        assert!(!body.to_string().contains(secret));
    }
    let calls = f.memory.calls.load(Ordering::SeqCst);
    assert_eq!(
        get(&f, "outsider", &path(&f)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(calls, f.memory.calls.load(Ordering::SeqCst));
}

#[tokio::test]
async fn pending_messages_withhold_text_for_shared_or_revoked_target_authority() {
    let f = fixture::fixture_for(false, true).await;
    install(&f, vec![queued(&f, 1)]);
    let (status, body) = get(&f, "owner-a", &path(&f)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"status":"redacted"}));
    let f = fixture::fixture(true).await;
    install(&f, vec![queued(&f, 1)]);
    f.memory.revoke.store(true, Ordering::SeqCst);
    let (status, body) = get(&f, "owner-a", &path(&f)).await;
    assert_ne!(status, StatusCode::OK);
    assert!(!body.to_string().contains("private fixture input"));
}

#[tokio::test]
async fn pending_messages_bound_unicode_previews_and_omit_missing_sources() {
    let f = fixture::fixture(false).await;
    let mut first = queued(&f, 1);
    first.source.as_mut().unwrap().envelope.text = Some("😀".repeat(1030));
    let mut second = queued(&f, 2);
    second.source = None;
    install(&f, vec![first, second]);
    let (status, body) = get(&f, "owner-a", &path(&f)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["messages"][0]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        1024
    );
    assert_eq!(body["messages"][0]["text_truncated"], true);
    assert_eq!(body["source_unavailable"], true);
}

#[tokio::test]
async fn pending_messages_reject_cross_partition_evidence_and_skip_previous_generations() {
    let f = fixture::fixture(false).await;
    let mut wrong = queued(&f, 1);
    wrong.sidecar.allocation_id = "another-partition".into();
    install(&f, vec![wrong]);
    let (status, body) = get(&f, "owner-a", &path(&f)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!body.to_string().contains("private fixture input"));
    f.memory.evidence.lock().unwrap().current = false;
    let (status, body) = get(&f, "owner-a", &path(&f)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"status":"unavailable"}));
}
