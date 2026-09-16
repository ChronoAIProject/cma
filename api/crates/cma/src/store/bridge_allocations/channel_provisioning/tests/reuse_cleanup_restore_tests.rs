use super::*;
use layer3_adapter::{AgentSlot, RuntimeHandle};

impl CleanupFixture {
    async fn revive_to_active(&self) {
        let paused = self.f.sandbox().await;
        self.f
            .f
            .original
            .resources
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                paused.id,
                paused.owner,
                layer2_core::now_rfc3339(),
                SandboxReviveCause::ExplicitRequest,
            ))
            .await
            .unwrap();
        for _ in 0..16 {
            let parent = self.f.sandbox().await;
            if parent.status == SandboxStatus::Active {
                return;
            }
            self.f.step().await;
        }
        panic!("revival did not complete");
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_restore_of_old_checkpoint_cannot_resurrect_removed_child_paths() {
    let mut f = CleanupFixture::new(true).await;
    let target = f.original.placement.target().clone();
    let original_pause = f.f.pause().await;
    let old_checkpoint = original_pause.lifecycle.checkpoints.latest.clone().unwrap();
    assert!(old_checkpoint
        .conversations
        .iter()
        .any(|conversation| conversation.agent_id == target.agent_id));
    f.revive_to_active().await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let tombstone = f.prepare_removal().await.unwrap();
    for _ in 0..4 {
        f.remove_step().await.unwrap();
    }
    let deleted =
        f.f.f
            .original
            .resources
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap();
    let sibling_before_pause =
        f.f.f
            .original
            .resources
            .get_agent(&f.f.f.sibling.id)
            .await
            .unwrap()
            .unwrap();
    let mut paused = f.f.pause().await;
    // Select a real retained archive captured before removal, using only this
    // disposable fixture. Restore still uses the actual store/worker/adapter.
    let revision = paused.revision;
    paused.lifecycle.checkpoints.latest = Some(old_checkpoint.clone());
    paused.lifecycle.checkpoints.previous = None;
    // Simulate unavailable retained storage so ordinary revive must import the
    // real old archive, instead of adopting the already-clean persistent home.
    paused.lifecycle.home = None;
    paused.revision += 1;
    paused.validate().unwrap();
    f.f.f
        .original
        .resources
        .save_sandbox(revision, paused)
        .await
        .unwrap();
    f.revive_to_active().await;
    let active = f.f.sandbox().await;
    let handle = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let runtime = f.f.f.original.backend.snapshot(&handle).await.unwrap();
    assert_eq!(
        runtime.restored_object,
        Some(old_checkpoint.object_reference)
    );
    assert!(!runtime
        .homes
        .contains_key(&AgentSlot::new(target.agent_id.to_string())));
    assert!(!runtime.worktrees.contains_key(&(
        tombstone.workspace_name().into(),
        tombstone.worktree_name().into()
    )));
    assert!(!runtime
        .agents
        .contains_key(&AgentSlot::new(target.agent_id.to_string())));
    assert!(runtime
        .agents
        .contains_key(&AgentSlot::new(f.f.f.sibling.id.to_string())));
    assert_eq!(
        f.f.f
            .original
            .resources
            .get_agent(&target.agent_id)
            .await
            .unwrap()
            .unwrap(),
        deleted
    );
    let sibling =
        f.f.f
            .original
            .resources
            .get_agent(&f.f.f.sibling.id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(sibling.status, AgentStatus::Ready);
    assert_eq!(
        sibling.conversation.conversation_id,
        sibling_before_pause.conversation.conversation_id
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_restore_requires_retained_exact_completion_proof() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let intent = f.prepare_removal().await.unwrap();
    for _ in 0..4 {
        f.remove_step().await.unwrap();
    }
    let parent = f.f.sandbox().await;
    let resources = &f.f.f.original.resources;
    let expected = resources
        .retired_agent_runtime_paths(&parent)
        .await
        .unwrap();
    assert_eq!(expected.len(), 1);
    let before_io = f.f.f.original.backend.requests().await;
    let mut accepted_corruptions = Vec::new();
    for (collection, id) in [
        ("agents", intent.seal.target.agent_id.as_str()),
        ("worktrees", intent.seal.target.worktree_id.as_str()),
    ] {
        let rows = f.db().collection::<Document>(collection);
        let original = f.row(collection, id).await;
        for mutation in ["missing_marker", "foreign_marker", "missing_row"] {
            match mutation {
                "missing_marker" => {
                    rows.update_one(
                        doc! {"_id":id},
                        doc! {"$unset":{CLEANUP_EXECUTION_FIELD:""}},
                    )
                    .await
                    .unwrap();
                }
                "foreign_marker" => {
                    rows.update_one(
                        doc! {"_id":id},
                        doc! {"$set":{CLEANUP_EXECUTION_FIELD:"foreign-removal"}},
                    )
                    .await
                    .unwrap();
                }
                _ => {
                    rows.delete_one(doc! {"_id":id}).await.unwrap();
                }
            }
            if resources.retired_agent_runtime_paths(&parent).await.is_ok() {
                accepted_corruptions.push(format!("{collection}.{mutation}"));
            }
            rows.replace_one(doc! {"_id":id}, original.clone())
                .upsert(true)
                .await
                .unwrap();
            assert_eq!(
                resources
                    .retired_agent_runtime_paths(&parent)
                    .await
                    .unwrap(),
                expected
            );
        }
    }
    for (collection, id, field, value) in [
        (
            "workspaces",
            intent.seal.target.workspace_id.as_str(),
            "owner",
            "foreign-owner",
        ),
        (
            "agents",
            intent.seal.target.agent_id.as_str(),
            "created_at",
            "2020-01-01T00:00:00Z",
        ),
    ] {
        let rows = f.db().collection::<Document>(collection);
        let original = f.row(collection, id).await;
        rows.update_one(
            doc! {"_id":id},
            doc! {"$set":{field:value,format!("record.{field}"):value}},
        )
        .await
        .unwrap();
        if resources.retired_agent_runtime_paths(&parent).await.is_ok() {
            accepted_corruptions.push(format!("{collection}.{field}"));
        }
        rows.replace_one(doc! {"_id":id}, original).await.unwrap();
    }
    assert_eq!(
        resources
            .retired_agent_runtime_paths(&parent)
            .await
            .unwrap(),
        expected
    );
    assert_eq!(f.f.f.original.backend.requests().await, before_io);
    f.close().await;
    assert!(
        accepted_corruptions.is_empty(),
        "restore accepted corrupt completed proof: {accepted_corruptions:?}"
    );
}
