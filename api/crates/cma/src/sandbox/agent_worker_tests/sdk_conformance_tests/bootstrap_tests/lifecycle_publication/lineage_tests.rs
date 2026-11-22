use super::*;

pub(super) async fn repair_workspace_commit(store: &MongoSandboxStore, id: &AgentId) {
    let agent = store.get_agent(id).await.unwrap().unwrap();
    let mut workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        workspace.resolved_commit.as_ref(),
        Some(&worktree.base_commit)
    );
    let revision = workspace.revision;
    workspace.resolved_commit = Some(worktree.base_commit);
    workspace.revision += 1;
    store.save_workspace(revision, workspace).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing to a disposable replica set"]
async fn rejected_lineage_is_fenced_on_every_publication_attempt() {
    for repair in [true, false] {
        let fixture = Fixture::new().await;
        let h = &fixture.bootstrap.harness;
        let waiting = fixture.bootstrap.admit(None).await;
        fixture.bootstrap.ready().await;
        assert!(h
            .worker
            .run_one(&waiting.id, SystemTime::now())
            .await
            .unwrap());
        let (claimed, now) = fixture.claim(&waiting.id).await;
        assert_eq!(claimed.work.step, AgentWorkStep::AddWorktree);
        let mut workspace = h
            .store
            .get_workspace(&claimed.workspace_id)
            .await
            .unwrap()
            .unwrap();
        let original_worktree = h
            .store
            .get_worktree(&claimed.worktree_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            workspace.resolved_commit.as_ref(),
            Some(&original_worktree.base_commit)
        );
        if repair {
            let revision = workspace.revision;
            workspace.resolved_commit = Some("b".repeat(40));
            workspace.revision += 1;
            h.store
                .save_workspace(revision, workspace.clone())
                .await
                .unwrap();
            *fixture.trace.repair_after_abort.lock().unwrap() = Some(claimed.id.clone());
        } else {
            // A consistently foreign Workspace cannot be constructed through the public store.
            let changed = fixture.admin.database(&fixture.database)
                .collection::<Document>("workspaces")
                .update_one(
                    doc! {"_id":claimed.workspace_id.as_str()},
                    doc! {
                        "$set":{"owner":"foreign-workspace-fixture", "record.owner":"foreign-workspace-fixture"},
                        "$inc":{"revision":1_i64, "record.revision":1_i64},
                    },
                ).await.unwrap();
            assert_eq!(changed.matched_count, 1);
            workspace = h
                .store
                .get_workspace(&claimed.workspace_id)
                .await
                .unwrap()
                .unwrap();
        }
        fixture.transient(doc! {"times":1_i32}.into()).await;
        let requests = h.backend.requests().await.len();
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), now)
            .await;
        fixture.disable().await;
        let agent = h.stored(&claimed.id).await;
        let parent = h
            .store
            .get_workspace(&claimed.workspace_id)
            .await
            .unwrap()
            .unwrap();
        let worktree = h
            .store
            .get_worktree(&claimed.worktree_id)
            .await
            .unwrap()
            .unwrap();
        let logs = h.store.agent_log(&claimed.id, 0, 100).await.unwrap();
        let failures = logs.iter().filter(|entry| matches!(&entry.kind,
            AgentLogKind::WorkStepFailed { failure, .. } if failure.class == "agent_lineage_invalid"
        )).count();
        assert_eq!(h.backend.requests().await.len(), requests);
        fixture.cleanup().await;

        assert!(fixture.trace.repair_after_abort.lock().unwrap().is_none());
        assert_eq!(fixture.trace.bodies.load(Ordering::SeqCst), 2);
        if repair {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(agent, claimed);
            assert_eq!(worktree, original_worktree);
            assert_eq!(parent.resolved_commit.as_ref(), Some(&worktree.base_commit));
            assert_eq!(failures, 0);
        } else {
            result.unwrap();
            assert_eq!(agent.status, AgentStatus::Failed);
            assert_eq!(worktree.status, WorktreeStatus::Failed);
            assert_eq!(parent, workspace);
            assert_eq!(failures, 1);
        }
    }
}
