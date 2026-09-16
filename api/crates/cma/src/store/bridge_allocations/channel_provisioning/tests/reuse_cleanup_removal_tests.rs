use super::*;
#[path = "reuse_cleanup_removal_faults.rs"]
mod faults;
#[path = "reuse_cleanup_ownership_tests.rs"]
mod ownership;
#[path = "reuse_cleanup_restore_tests.rs"]
mod restore;

impl CleanupFixture {
    async fn prepare_removal(&self) -> Result<ChannelReuseRemovalState, AllocationError> {
        self.f
            .f
            .original
            .allocations
            .prepare_channel_reuse_child_removal(&self.permit().await, &self.request())
            .await
    }
    async fn remove_step(&self) -> Result<ChannelReuseRemovalState, AllocationError> {
        self.f
            .f
            .original
            .allocations
            .run_channel_reuse_child_removal(
                &self.permit().await,
                &self.request(),
                self.f.f.original.adapter.as_ref(),
                &self.f.registry,
                &self.f.f.original.settings.deployment_id,
            )
            .await
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_removal_tombstone_precedes_effects_and_preserves_siblings() {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    let saved = f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let before_parent = f.f.sandbox().await;
    let sibling = f.row("agents", f.f.f.sibling.id.as_str()).await;
    let workspace =
        f.f.f
            .original
            .resources
            .get_workspace(&saved.seal.target.workspace_id)
            .await
            .unwrap()
            .unwrap();
    let requests = f.f.f.original.backend.requests().await;
    let intent = f.prepare_removal().await.unwrap();
    assert_eq!(intent.phase, ChannelReuseRemovalPhase::Intent);
    assert_eq!(f.f.f.original.backend.requests().await, requests);
    assert!(f.prepare_removal().await.unwrap() == intent);
    assert!(f
        .f
        .f
        .original
        .resources
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            before_parent.id.clone(),
            before_parent.owner.clone(),
            layer2_core::now_rfc3339(),
            None,
        ))
        .await
        .is_err());
    assert_eq!(f.f.sandbox().await, before_parent);
    let mut completed = intent;
    for phase in [
        ChannelReuseRemovalPhase::ProcessClosed,
        ChannelReuseRemovalPhase::HomeRemoved,
        ChannelReuseRemovalPhase::WorktreeRemoved,
        ChannelReuseRemovalPhase::Complete,
    ] {
        completed = f.remove_step().await.unwrap();
        assert_eq!(completed.phase, phase);
    }
    assert!(f.remove_step().await.unwrap() == completed);
    let resources = &f.f.f.original.resources;
    let deleted = resources
        .get_agent(&saved.seal.target.agent_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(deleted.status, AgentStatus::Deleted);
    assert_eq!(
        resources
            .get_worktree(&saved.seal.target.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorktreeStatus::Removed
    );
    assert_eq!(
        resources
            .get_workspace(&workspace.id)
            .await
            .unwrap()
            .unwrap(),
        workspace
    );
    assert_eq!(f.row("agents", f.f.f.sibling.id.as_str()).await, sibling);
    assert_eq!(f.f.sandbox().await, before_parent);
    assert!(f.request_cleanup().await.unwrap() == saved);
    let paths = resources
        .retired_agent_runtime_paths(&before_parent)
        .await
        .unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].agent_id, saved.seal.target.agent_id);
    // Ordinary lifecycle becomes available only after verified physical work.
    f.f.pause().await;
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_removal_lost_publication_replays_same_effect_and_retains_exclusion() {
    let mut f = CleanupFixture::new(false).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let intent = f.prepare_removal().await.unwrap();
    f.db().run_command(doc! {"collMod":"channel_reuse_cleanup_removal_audits", "validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert_eq!(
        f.remove_step().await.err(),
        Some(AllocationError::Unavailable)
    );
    assert!(f.prepare_removal().await.unwrap() == intent);
    assert!(f
        .row("sandboxes", intent.seal.target.sandbox_id.as_str())
        .await
        .contains_key(CLEANUP_EXECUTION_FIELD));
    assert_eq!(
        f.f.f
            .original
            .resources
            .get_agent(&intent.seal.target.agent_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Ready
    );
    f.db()
        .run_command(doc! {"collMod":"channel_reuse_cleanup_removal_audits", "validator":{}})
        .await
        .unwrap();
    assert_eq!(
        f.remove_step().await.unwrap().phase,
        ChannelReuseRemovalPhase::ProcessClosed
    );
    for _ in 0..3 {
        f.remove_step().await.unwrap();
    }
    assert_eq!(
        f.prepare_removal().await.unwrap().phase,
        ChannelReuseRemovalPhase::Complete
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_removal_refuses_accepted_commands_without_tombstone_or_effect() {
    let mut f = CleanupFixture::new(true).await;
    f.f.queued("cleanup-removal-pending").await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let before = f.f.f.original.backend.requests().await;
    assert_eq!(
        f.prepare_removal().await.err(),
        Some(AllocationError::Conflict)
    );
    assert_eq!(
        f.db()
            .collection::<Document>(CLEANUP_TOMBSTONES)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert!(!f
        .row("sandboxes", f.f.f.parent.id.as_str())
        .await
        .contains_key(CLEANUP_EXECUTION_FIELD));
    assert_eq!(f.f.f.original.backend.requests().await, before);
    f.close().await;
}
