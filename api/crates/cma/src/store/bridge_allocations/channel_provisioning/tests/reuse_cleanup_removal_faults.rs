use super::*;
use layer3_adapter::{wire::WireOperation, RuntimeHandle};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_removal_unknown_runtime_result_and_stale_boot_never_release_exclusion(
) {
    let mut f = CleanupFixture::new(true).await;
    f.reset().await;
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let intent = f.prepare_removal().await.unwrap();
    f.f.f
        .original
        .backend
        .lose_next_response(WireOperation::CloseAgent);
    assert_eq!(
        f.remove_step().await.err(),
        Some(AllocationError::Unavailable)
    );
    assert!(f.prepare_removal().await.unwrap() == intent);
    let closed = f.remove_step().await.unwrap();
    assert_eq!(closed.phase, ChannelReuseRemovalPhase::ProcessClosed);
    f.f.f
        .original
        .backend
        .restart_persistent_process(&RuntimeHandle(closed.runtime.runtime_id.clone()))
        .await
        .unwrap();
    let before = f.f.f.original.backend.requests().await;
    assert_eq!(
        f.remove_step().await.err(),
        Some(AllocationError::Unavailable)
    );
    let after = f.f.f.original.backend.requests().await;
    assert_eq!(
        before
            .iter()
            .filter(|(_, request)| matches!(
                request,
                layer3_adapter::wire::RequestFrame::RemoveAgentHome(_)
            ))
            .count(),
        after
            .iter()
            .filter(|(_, request)| matches!(
                request,
                layer3_adapter::wire::RequestFrame::RemoveAgentHome(_)
            ))
            .count()
    );
    assert!(f
        .row("sandboxes", closed.seal.target.sandbox_id.as_str())
        .await
        .contains_key(CLEANUP_EXECUTION_FIELD));
    assert!(f.prepare_removal().await.unwrap() == closed);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn shared_child_cleanup_removal_releases_agent_capacity_only_after_verified_completion() {
    let mut f = CleanupFixture::with_capacity(true, 4).await;
    let next = f.reset().await;
    let next = f.f.f.drive_ready(next).await;
    // Old child + current child + ordinary sibling + reserved spare fill4.
    assert_eq!(
        f.f.f.reset(&next, "67890").await.err(),
        Some(AllocationError::Capacity)
    );
    f.request_cleanup().await.unwrap();
    f.close_admission().await.unwrap();
    let intent = f.prepare_removal().await.unwrap();
    assert_eq!(
        f.f.f.reset(&next, "67890").await.err(),
        Some(AllocationError::Capacity)
    );
    for _ in 0..3 {
        f.remove_step().await.unwrap();
    }
    assert_eq!(
        f.f.f.reset(&next, "67890").await.err(),
        Some(AllocationError::Capacity)
    );
    let completed = f.remove_step().await.unwrap();
    assert_eq!(completed.phase, ChannelReuseRemovalPhase::Complete);
    let parent = f.f.sandbox().await;
    let row = f.row(CLEANUP_TOMBSTONES, &intent.id).await;
    f.db()
        .collection::<Document>(CLEANUP_TOMBSTONES)
        .delete_one(doc! {"_id":&intent.id})
        .await
        .unwrap();
    assert!(f
        .f
        .f
        .original
        .resources
        .retired_agent_runtime_paths(&parent)
        .await
        .is_err());
    // The retained marker still names this parent in its embedded record.
    // Corrupting an owned fixture index cannot hide the missing tombstone.
    let agent_row = f.row("agents", intent.seal.target.agent_id.as_str()).await;
    let agents = f.db().collection::<Document>("agents");
    agents
        .update_one(
            doc! {"_id":intent.seal.target.agent_id.as_str()},
            doc! {"$set":{"sandbox_id":"sbx_hidden_tombstone"}},
        )
        .await
        .unwrap();
    assert!(
        f.f.f
            .original
            .resources
            .retired_agent_runtime_paths(&parent)
            .await
            .is_err(),
        "a misindexed retained marker cannot hide its missing tombstone"
    );
    agents
        .replace_one(doc! {"_id":intent.seal.target.agent_id.as_str()}, agent_row)
        .await
        .unwrap();
    assert_eq!(
        f.f.f.reset(&next, "67890").await.err(),
        Some(AllocationError::Capacity)
    );
    f.db()
        .collection::<Document>(CLEANUP_TOMBSTONES)
        .insert_one(row)
        .await
        .unwrap();
    assert_eq!(
        f.f.f
            .original
            .resources
            .retired_agent_runtime_paths(&parent)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut restoration_gaps = Vec::new();
    for (collection, id, field, value) in [
        (
            "agents",
            intent.seal.target.agent_id.as_str(),
            "owner",
            "foreign-owner",
        ),
        (
            "agents",
            intent.seal.target.agent_id.as_str(),
            "sandbox_id",
            "sbx_foreign",
        ),
        (
            "agents",
            intent.seal.target.agent_id.as_str(),
            "workspace_id",
            "wks_foreign",
        ),
        (
            "worktrees",
            intent.seal.target.worktree_id.as_str(),
            "owner",
            "foreign-owner",
        ),
        (
            "worktrees",
            intent.seal.target.worktree_id.as_str(),
            "sandbox_id",
            "sbx_foreign",
        ),
        (
            "worktrees",
            intent.seal.target.worktree_id.as_str(),
            "workspace_id",
            "wks_foreign",
        ),
        (
            "worktrees",
            intent.seal.target.worktree_id.as_str(),
            "agent_id",
            "agt_foreign",
        ),
    ] {
        let row = f.row(collection, id).await;
        let rows = f.db().collection::<Document>(collection);
        // Keep indexed and embedded fields consistent, so record decoding
        // succeeds and the completed-child proof must reject wrong lineage.
        let mut fields = doc! {format!("record.{field}"):value};
        if field != "agent_id" {
            fields.insert(field, value);
        }
        rows.update_one(doc! {"_id":id}, doc! {"$set":fields})
            .await
            .unwrap();
        if f.f
            .f
            .original
            .resources
            .retired_agent_runtime_paths(&parent)
            .await
            .is_ok()
        {
            restoration_gaps.push(format!("{collection}.{field}"));
        }
        assert_eq!(
            f.f.f.reset(&next, "67890").await.err(),
            Some(AllocationError::Corrupt),
            "capacity release accepted mismatched {collection}.{field}"
        );
        rows.replace_one(doc! {"_id":id}, row).await.unwrap();
    }
    let third = f.f.f.reset(&next, "67890").await.unwrap();
    assert_ne!(
        third.placement.target().agent_id,
        intent.seal.target.agent_id
    );
    assert_eq!(
        third.placement.target().workspace_id,
        intent.seal.target.workspace_id
    );
    f.close().await;
    assert!(
        restoration_gaps.is_empty(),
        "restore accepted mismatched completed ownership: {restoration_gaps:?}"
    );
}
