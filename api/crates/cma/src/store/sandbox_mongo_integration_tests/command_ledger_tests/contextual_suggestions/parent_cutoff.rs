use super::*;
use layer2_core::sandbox::store::{
    SandboxLifecycleCommand, SandboxLifecycleCommandResult, SandboxLifecycleCommandStore,
};
use mongodb::bson::Document;

mod fixture;
mod gate;
mod uncertainty;
use fixture::Fixture;
use gate::{Gate, Point};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated Mongo replica set"]
async fn staging_hold_witnesses_pause_before_the_first_publication() {
    use crate::config::suggestion_control::{Mode, Target};
    use crate::store::suggestion_control::SuggestionControl;
    use std::time::Duration;

    let f = Fixture::with_control(Mode::PauseFirst).await;
    let target = Target {
        sandbox_id: f.parent.id.to_string(),
        agent_id: f.agent.id.to_string(),
    };
    let operator = SuggestionControl::new(&f.other, target);
    let attempts_before = f.gate.attempts();
    let completion = f.complete();
    let held = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let report = operator.read().await.unwrap().unwrap();
            if report.held_at_ms.is_some() {
                break report;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .expect("completion reached its parent-fence checkpoint");
    assert!(!completion.is_finished());
    let claim_started_at_ms = i64::try_from(f.claim.started_at_ms.unwrap()).unwrap();
    assert_eq!(held.claim_started_at_ms, Some(claim_started_at_ms));
    assert_eq!(f.gate.attempts() - attempts_before, 1);
    assert_eq!(f.row().await.get_str("status").unwrap(), "running");
    assert!(!operator.release(claim_started_at_ms).await.unwrap());

    assert!(matches!(
        f.pause(&f.other).await.unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
    assert_eq!(f.row().await.get_str("status").unwrap(), "running");
    assert!(operator.release(claim_started_at_ms).await.unwrap());
    completion.await.unwrap().unwrap();
    assert_eq!(f.gate.attempts() - attempts_before, 2);

    let witnessed = serde_json::to_value(operator.read().await.unwrap().unwrap()).unwrap();
    assert_eq!(witnessed["durable_status"], "expired");
    assert_eq!(witnessed["durable_has_set"], false);
    assert!(
        witnessed["pause_revision"].as_u64().unwrap()
            > witnessed["parent_snapshot_revision"].as_u64().unwrap()
    );
    assert_eq!(f.row().await.get_str("status").unwrap(), "expired");
    f.assert_hidden().await;
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated Mongo replica set"]
async fn pause_acknowledged_before_completion_prevents_first_ready_publication() {
    let f = Fixture::new().await;
    let release = f.gate.arm(Point::ParentRead);
    let completion = f.complete();
    f.gate.wait().await;
    assert!(matches!(
        f.pause(&f.other).await.unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
    drop(release);
    completion.await.unwrap().unwrap();
    assert_eq!(f.row().await.get_str("status").unwrap(), "expired");
    assert_eq!(
        f.gate.attempts(),
        2,
        "one aborted completion retries against the paused Sandbox"
    );
    f.assert_hidden().await;
    assert!(f
        .other
        .claim_suggestions(unix_millis())
        .await
        .unwrap()
        .is_none());
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated Mongo replica set"]
async fn completion_parent_write_orders_pause_and_survives_lifecycle_save() {
    let f = Fixture::new().await;
    let release = f.gate.arm(Point::ParentWrite);
    let completion = f.complete();
    f.gate.wait().await;
    assert!(
        f.pause(&f.other).await.is_err(),
        "Pause cannot acknowledge while completion owns the parent write"
    );
    assert_eq!(
        f.other.get_sandbox(&f.parent.id).await.unwrap().unwrap(),
        f.parent
    );
    drop(release);
    completion.await.unwrap().unwrap();
    let ready = f.row().await;
    assert_eq!(ready.get_str("status").unwrap(), "ready");
    let row = f
        .other
        .sandboxes
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("suggestion_completion_fence").unwrap(), 1);
    assert_eq!(row.get_i64("revision").unwrap(), f.parent.revision as i64);
    assert!(matches!(
        f.pause(&f.other).await.unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
    let row = f
        .other
        .sandboxes
        .find_one(doc! {"_id":f.parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.get_i64("suggestion_completion_fence").unwrap(), 1);
    assert_eq!(
        f.row().await,
        ready,
        "Pause does not rewrite previously committed suggestions"
    );
    f.assert_hidden().await;
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an isolated Mongo replica set"]
async fn committed_completion_can_acknowledge_after_pause_without_republishing() {
    let f = Fixture::new().await;
    let release = f.gate.arm(Point::CommitAck);
    let completion = f.complete();
    f.gate.wait().await;
    let ready = f.row().await;
    assert_eq!(ready.get_str("status").unwrap(), "ready");
    assert!(matches!(
        f.pause(&f.other).await.unwrap(),
        SandboxLifecycleCommandResult::Accepted(_)
    ));
    drop(release);
    completion.await.unwrap().unwrap();
    assert_eq!(f.row().await, ready);
    f.store
        .complete_suggestions(&f.claim, f.completion())
        .await
        .unwrap();
    assert_eq!(f.row().await, ready);
    f.assert_hidden().await;
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn completion_after_pause_or_missing_parent_expires_without_a_parent_upsert() {
    for missing in [false, true] {
        let f = Fixture::new().await;
        if missing {
            f.other
                .sandboxes
                .delete_one(doc! {"_id":f.parent.id.as_str()})
                .await
                .unwrap();
        } else {
            assert!(matches!(
                f.pause(&f.other).await.unwrap(),
                SandboxLifecycleCommandResult::Accepted(_)
            ));
        }
        f.store
            .complete_suggestions(&f.claim, f.completion())
            .await
            .unwrap();
        assert_eq!(f.row().await.get_str("status").unwrap(), "expired");
        if missing {
            let reading = f
                .other
                .read_suggestions(&f.claim.owner, &f.agent.id, f.claim.scope)
                .await
                .unwrap();
            assert_eq!(reading.status, SuggestionStatus::Expired);
            assert!(reading.set.is_none());
            let (status, body) = read(&router(f.other.clone()), &f.agent, "owner").await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert!(body["set"].is_null());
        } else {
            f.assert_hidden().await;
        }
        let parent = f
            .other
            .sandboxes
            .find_one(doc! {"_id":f.parent.id.as_str()})
            .await
            .unwrap();
        assert_eq!(parent.is_none(), missing);
        assert!(parent.is_none_or(|row| !row.contains_key("suggestion_completion_fence")));
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn mismatched_parent_envelope_cannot_publish_suggestions() {
    for fields in [
        doc! {"revision":99_i64},
        doc! {"owner":"other-owner"},
        doc! {"channel_reuse_cleanup_execution":"another-writer"},
    ] {
        let f = Fixture::new().await;
        let before = f.row().await;
        f.other
            .sandboxes
            .update_one(doc! {"_id":f.parent.id.as_str()}, doc! {"$set":fields})
            .await
            .unwrap();
        assert!(f
            .store
            .complete_suggestions(&f.claim, f.completion())
            .await
            .is_err());
        assert_eq!(f.row().await, before);
        let row = f
            .other
            .sandboxes
            .find_one(doc! {"_id":f.parent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("suggestion_completion_fence"));
        f.cleanup().await;
    }
}
