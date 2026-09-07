use super::*;

const BEFORE: &str = "2026-09-05T00:02:59Z";
const BOUNDARY: &str = "2026-09-05T00:03:00Z";
const EXPIRES: &str = "2026-09-05T00:10:00Z";

async fn commit_next(store: &MongoSandboxStore, current: &mut Sandbox, next: Sandbox, at: &str) {
    store
        .commit_sandbox_lifecycle(mutation(current, next.clone(), at))
        .await
        .unwrap();
    *current = next;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failed_capture_completion_persists_cooldown_across_legacy_decode_and_restart() {
    let (client, name) = database("capture_completion_cooldown").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, workspace, agent) = capture_fixture(&store, "completion_cooldown").await;
    let begin = begin(&parent);
    store.commit_sandbox_lifecycle(begin.clone()).await.unwrap();
    let mut current = begin.sandbox;
    let mut next = current.clone();
    next.claim_checkpoint_capture("capture".into(), NOW.into(), EXPIRES.into())
        .unwrap();
    commit_next(&store, &mut current, next, NOW).await;
    let mut next = current.clone();
    next.require_checkpoint_writer_resume("capture", 1, MINUTE.into())
        .unwrap();
    commit_next(&store, &mut current, next, MINUTE).await;
    let mut next = current.clone();
    next.claim_checkpoint_capture("resume".into(), MINUTE.into(), EXPIRES.into())
        .unwrap();
    commit_next(&store, &mut current, next, MINUTE).await;

    let document = store
        .sandboxes
        .find_one(doc! { "_id": current.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(!document
        .get_document("record")
        .unwrap()
        .get_document("lifecycle")
        .unwrap()
        .contains_key("checkpoint_capture_completed_at"));
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert!(current.lifecycle.checkpoint_capture_completed_at.is_none());
    let mut minted = current.clone();
    minted.lifecycle.checkpoint_capture_completed_at = Some(MINUTE.into());
    minted.revision += 1;
    assert_generic_save_rejected(&reopened, &current, minted).await;

    let mut completed = current.clone();
    completed
        .complete_checkpoint_writer_resume("resume", 2, MINUTE.into())
        .unwrap();
    for timestamp in [None, Some(NOW), Some(BOUNDARY)] {
        let mut forged = completed.clone();
        forged.lifecycle.checkpoint_capture_completed_at = timestamp.map(str::to_owned);
        assert!(reopened
            .commit_sandbox_lifecycle(mutation(&current, forged, MINUTE))
            .await
            .is_err());
    }
    commit_next(&reopened, &mut current, completed, MINUTE).await;
    assert_eq!(current.lifecycle.checkpoints, parent.lifecycle.checkpoints);
    assert!(current.lifecycle.checkpoint_capture.is_none());
    assert_eq!(
        current.lifecycle.checkpoint_capture_completed_at.as_deref(),
        Some(MINUTE)
    );
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    let events = reopened.sandbox_events(&current.id, 0, 100).await.unwrap();
    for at in [MINUTE, BEFORE] {
        assert!(!layer2_core::sandbox::sandbox_checkpoint_due(&current, at));
        let mut forged = current.clone();
        forged.lifecycle.checkpoint_capture_completed_at = None;
        forged.begin_checkpoint_capture(at.into()).unwrap();
        forged.lifecycle.checkpoint_capture_completed_at = Some(MINUTE.into());
        assert_eq!(
            reopened
                .commit_sandbox_lifecycle(mutation(&current, forged, at))
                .await,
            Err(SandboxStoreError::Conflict)
        );
    }
    for timestamp in [None, Some(NOW), Some(BOUNDARY)] {
        let mut forged = current.clone();
        forged.lifecycle.checkpoint_capture_completed_at = timestamp.map(str::to_owned);
        forged.revision += 1;
        assert_generic_save_rejected(&reopened, &current, forged).await;
    }
    assert_eq!(reopened.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        reopened.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert_eq!(
        reopened.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        reopened.sandbox_events(&current.id, 0, 100).await.unwrap(),
        events
    );
    assert!(layer2_core::sandbox::sandbox_checkpoint_due(
        &current, BOUNDARY
    ));
    let mut next = current.clone();
    next.begin_checkpoint_capture(BOUNDARY.into()).unwrap();
    commit_next(&reopened, &mut current, next, BOUNDARY).await;
    assert_eq!(
        reopened.get_sandbox(&current.id).await.unwrap(),
        Some(current)
    );
    client.database(&name).drop().await.unwrap();
}
