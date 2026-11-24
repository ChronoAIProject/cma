use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_concurrent_stop_schedules_once_and_replays_exactly() {
    let (client, name) = database("agent_control_stop").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "control_stop").await;
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = Arc::clone(&store);
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            store.schedule_agent_stop(request).await.unwrap()
        }));
    }
    let mut scheduled_count = 0;
    let mut winner = None;
    for task in tasks {
        match task.await.unwrap() {
            AgentControlResult::Scheduled(agent) => {
                scheduled_count += 1;
                winner = Some(agent);
            }
            AgentControlResult::Existing(agent) => {
                winner.get_or_insert(agent);
            }
        };
    }
    assert_eq!(scheduled_count, 1);
    let winner = winner.unwrap();
    assert_eq!(winner.requested_stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(winner.work.step, AgentWorkStep::FenceInteraction);
    assert!(winner.work.lease.is_none());
    assert_eq!(
        store.schedule_agent_stop(request.clone()).await.unwrap(),
        AgentControlResult::Existing(winner.clone())
    );
    let mut changed = request;
    changed.reason = AgentStopReason::Operator;
    assert_eq!(
        store.schedule_agent_stop(changed).await,
        Err(AgentControlStoreError::StopReasonConflict)
    );
    assert_eq!(store.get_agent(&winner.id).await.unwrap(), Some(winner));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_delete_schedules_once_and_hides_foreign_owners() {
    use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;

    let (client, name) = database("agent_control_delete").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, _, claimed) = claimed_agent_work_fixture(&store, "control_delete").await;
    let expected_revision = claimed.revision;
    let mut stopped = claimed;
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    store
        .save_agent(expected_revision, stopped.clone())
        .await
        .unwrap();
    let request = AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: TWO_MINUTES.into(),
    };
    let mut foreign = request.clone();
    foreign.target.owner = SubjectId("another-owner".into());
    assert_eq!(
        store.schedule_agent_delete(foreign).await,
        Err(AgentControlStoreError::NotFound)
    );

    let agent_key = doc! {"_id": stopped.id.as_str()};
    let parent_key = doc! {"_id": parent.id.as_str()};
    let before = store.agents.find_one(agent_key.clone()).await.unwrap();
    let mut hold = Document::new();
    hold.insert(LEDGER_MAINTENANCE_FIELD, doc! {"phase": "requested"});
    store
        .sandboxes
        .update_one(parent_key.clone(), doc! {"$set": hold.clone()})
        .await
        .unwrap();
    let parent_held = store.sandboxes.find_one(parent_key.clone()).await.unwrap();
    assert_eq!(
        store.schedule_agent_delete(request.clone()).await,
        Err(AgentControlStoreError::Conflict)
    );
    assert_eq!(
        store.agents.find_one(agent_key.clone()).await.unwrap(),
        before
    );
    assert_eq!(
        store.sandboxes.find_one(parent_key.clone()).await.unwrap(),
        parent_held
    );

    store
        .sandboxes
        .update_one(parent_key.clone(), doc! {"$unset": hold.clone()})
        .await
        .unwrap();
    let scheduled = match store.schedule_agent_delete(request.clone()).await.unwrap() {
        AgentControlResult::Scheduled(agent) => agent,
        AgentControlResult::Existing(_) => panic!("new delete must be scheduled"),
    };
    assert_eq!(scheduled.work.step, AgentWorkStep::EnsureProcessClosed);
    store
        .sandboxes
        .update_one(parent_key.clone(), doc! {"$set": hold})
        .await
        .unwrap();
    let agent_scheduled = store.agents.find_one(agent_key.clone()).await.unwrap();
    let parent_held = store.sandboxes.find_one(parent_key.clone()).await.unwrap();
    assert_eq!(
        store.schedule_agent_delete(request).await.unwrap(),
        AgentControlResult::Existing(scheduled)
    );
    assert_eq!(
        store.agents.find_one(agent_key).await.unwrap(),
        agent_scheduled
    );
    assert_eq!(
        store.sandboxes.find_one(parent_key).await.unwrap(),
        parent_held
    );

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_receipts_and_lineage_fences_survive_worker_progression() {
    let (client, name) = database("agent_control_receipts").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, worktree, claimed) = claimed_agent_work_fixture(&store, "receipts").await;
    let workspace_before = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let scheduled = store
        .schedule_agent_stop(request)
        .await
        .unwrap()
        .into_agent();

    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    assert_eq!(
        store.get_workspace(&workspace_before.id).await.unwrap(),
        Some(workspace_before.clone())
    );
    assert_eq!(
        store.get_worktree(&worktree.id).await.unwrap(),
        Some(worktree.clone())
    );

    let agent_document = store
        .agents
        .find_one(doc! { "_id": scheduled.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    let stop_receipt = agent_document.get(STOP_RECEIPT_FIELD).unwrap().clone();
    assert!(agent_document.get(DELETE_RECEIPT_FIELD).is_none());
    let markers = [
        store
            .worktrees
            .find_one(doc! { "_id": scheduled.worktree_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .workspaces
            .find_one(doc! { "_id": scheduled.workspace_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
        store
            .sandboxes
            .find_one(doc! { "_id": scheduled.sandbox_id.as_str() })
            .await
            .unwrap()
            .unwrap(),
    ];
    let marker = markers[0].get_str("last_agent_control_operation").unwrap();
    assert!(marker.starts_with("ctl_"));
    assert!(markers
        .iter()
        .all(|document| { document.get_str("last_agent_control_operation").ok() == Some(marker) }));

    let mut advanced_workspace = workspace_before;
    let expected_workspace_revision = advanced_workspace.revision;
    advanced_workspace.next_worktree_number += 1;
    advanced_workspace.revision += 1;
    store
        .save_workspace(expected_workspace_revision, advanced_workspace.clone())
        .await
        .unwrap();
    let advanced_workspace_document = store
        .workspaces
        .find_one(doc! { "_id": advanced_workspace.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        advanced_workspace_document
            .get_str("last_agent_control_operation")
            .unwrap(),
        marker
    );

    let mut claimed_stop = store
        .claim_agent_work(&scheduled.id, "stop-worker", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .unwrap();
    claimed_stop.work.step = AgentWorkStep::PublishStopped;
    claimed_stop.work.cleanup_not_before = None;
    replace_agent_fixture(&store, &claimed_stop).await;
    let stop_fence = claimed_stop.work.lease.as_ref().unwrap().fence;
    let mut stopped = claimed_stop.clone();
    stopped
        .work
        .advance(
            "stop-worker",
            stop_fence,
            AgentWorkStep::None,
            MINUTE.into(),
        )
        .unwrap();
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    stopped.conversation.last_seq += 1;
    let mut failed_worktree = worktree;
    let expected_worktree_revision = failed_worktree.revision;
    failed_worktree.transition(WorktreeStatus::Failed).unwrap();
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed_stop.revision,
            holder: "stop-worker".into(),
            fence: stop_fence,
            agent: stopped.clone(),
            worktree: Some(VersionedWorktree {
                expected_revision: expected_worktree_revision,
                worktree: failed_worktree.clone(),
            }),
            appended_at: MINUTE.into(),
            sandbox_event: Some(SandboxEventKind::AgentStatusChanged {
                agent_id: stopped.id.clone(),
                from: AgentStatus::Starting,
                to: AgentStatus::Stopped,
            }),
            agent_logs: vec![AgentLogKind::Stopped {
                reason: AgentStopReason::Owner.as_str().into(),
            }],
        })
        .await
        .unwrap();
    let after_stop = store
        .agents
        .find_one(doc! { "_id": stopped.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_stop.get(STOP_RECEIPT_FIELD), Some(&stop_receipt));

    let deleting = store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: TWO_MINUTES.into(),
        })
        .await
        .unwrap()
        .into_agent();
    let after_delete_request = store
        .agents
        .find_one(doc! { "_id": deleting.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after_delete_request.get(STOP_RECEIPT_FIELD),
        Some(&stop_receipt)
    );
    let delete_receipt = after_delete_request
        .get(DELETE_RECEIPT_FIELD)
        .unwrap()
        .clone();

    let mut claimed_delete = store
        .claim_agent_work(
            &deleting.id,
            "delete-worker",
            TWO_MINUTES.into(),
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    claimed_delete.work.step = AgentWorkStep::PublishDeleted;
    replace_agent_fixture(&store, &claimed_delete).await;
    failed_worktree.status = WorktreeStatus::Removed;
    replace_worktree_fixture(&store, &failed_worktree).await;
    let delete_fence = claimed_delete.work.lease.as_ref().unwrap().fence;
    let mut deleted = claimed_delete.clone();
    deleted
        .work
        .advance(
            "delete-worker",
            delete_fence,
            AgentWorkStep::None,
            TWO_MINUTES.into(),
        )
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    deleted.conversation.last_seq += 1;
    store
        .commit_agent_work(AgentWorkMutation {
            expected_agent_revision: claimed_delete.revision,
            holder: "delete-worker".into(),
            fence: delete_fence,
            agent: deleted.clone(),
            worktree: None,
            appended_at: TWO_MINUTES.into(),
            sandbox_event: Some(SandboxEventKind::AgentDeleted {
                agent_id: deleted.id.clone(),
            }),
            agent_logs: vec![AgentLogKind::Deleted],
        })
        .await
        .unwrap();
    let after_deleted = store
        .agents
        .find_one(doc! { "_id": deleted.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_deleted.get(STOP_RECEIPT_FIELD), Some(&stop_receipt));
    assert_eq!(
        after_deleted.get(DELETE_RECEIPT_FIELD),
        Some(&delete_receipt)
    );

    client.database(&name).drop().await.unwrap();
}
