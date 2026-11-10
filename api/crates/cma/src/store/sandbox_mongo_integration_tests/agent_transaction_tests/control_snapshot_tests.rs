use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_snapshot_requires_exact_receipt_and_current_parents() {
    let (client, name) = database("agent_control_snapshot").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "snapshot").await;
    let request = AgentStopRequest {
        target: AgentControlTarget::from(&claimed),
        reason: AgentStopReason::Owner,
        requested_at: NOW.into(),
    };
    let operation = ControlOperation::new(ControlRequest::Stop(request));
    let scheduled = operation
        .request
        .plan(claimed.clone())
        .unwrap()
        .into_agent();
    let mut fields = agent_replacement_fields(&scheduled).unwrap();
    fields.extend(operation.receipt_update(&scheduled).unwrap());
    store
        .agents
        .update_one(
            doc! { "_id": claimed.id.as_str(), "revision": i64::try_from(claimed.revision).unwrap() },
            doc! { "$set": fields },
        )
        .await
        .unwrap();

    let mut session = client.start_session().snapshot(true).await.unwrap();
    let snapshot = load_control_snapshot(&store, operation.request.target(), &mut session, true)
        .await
        .unwrap();
    assert!(control_snapshot_matches(&operation, &scheduled, &snapshot));

    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, MINUTE.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            MINUTE.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    store
        .save_agent(scheduled.revision, stopped.clone())
        .await
        .unwrap();
    let delete_operation = ControlOperation::new(ControlRequest::Delete(AgentDeleteRequest {
        target: AgentControlTarget::from(&stopped),
        requested_at: TWO_MINUTES.into(),
    }));
    let deleting = delete_operation.request.plan(stopped).unwrap().into_agent();
    let mut fields = agent_replacement_fields(&deleting).unwrap();
    fields.extend(delete_operation.receipt_update(&deleting).unwrap());
    store
        .agents
        .update_one(
            doc! {
                "_id": claimed.id.as_str(),
                "revision": i64::try_from(deleting.revision - 1).unwrap(),
            },
            doc! { "$set": fields },
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let deleting_snapshot =
        load_control_snapshot(&store, operation.request.target(), &mut session, true)
            .await
            .unwrap();
    assert!(control_snapshot_matches(
        &operation,
        &scheduled,
        &deleting_snapshot
    ));
    assert!(control_snapshot_matches(
        &delete_operation,
        &deleting,
        &deleting_snapshot
    ));

    let mut deleted = deleting.clone();
    deleted
        .transition(AgentStatus::Deleted, TWO_MINUTES.into(), None)
        .unwrap();
    store.save_agent(deleting.revision, deleted).await.unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let deleted_snapshot =
        load_control_snapshot(&store, operation.request.target(), &mut session, true)
            .await
            .unwrap();
    assert!(control_snapshot_matches(
        &operation,
        &scheduled,
        &deleted_snapshot
    ));
    assert!(control_snapshot_matches(
        &delete_operation,
        &deleting,
        &deleted_snapshot
    ));

    store
        .agents
        .update_one(
            doc! { "_id": claimed.id.as_str() },
            doc! { "$unset": { STOP_RECEIPT_FIELD: "" } },
        )
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    let no_receipt = load_control_snapshot(&store, operation.request.target(), &mut session, true)
        .await
        .unwrap();
    assert!(!control_snapshot_matches(
        &operation,
        &scheduled,
        &no_receipt
    ));
    assert!(control_snapshot_matches(
        &delete_operation,
        &deleting,
        &no_receipt
    ));

    for (collection, id) in [
        (&store.worktrees, claimed.worktree_id.as_str()),
        (&store.workspaces, claimed.workspace_id.as_str()),
        (&store.sandboxes, claimed.sandbox_id.as_str()),
    ] {
        let original = collection
            .find_one(doc! { "_id": id })
            .await
            .unwrap()
            .unwrap();
        collection
            .update_one(
                doc! { "_id": id },
                doc! { "$set": { "owner": "different-owner" } },
            )
            .await
            .unwrap();
        let mut session = client.start_session().snapshot(true).await.unwrap();
        assert!(matches!(
            load_control_snapshot(&store, operation.request.target(), &mut session, true,).await,
            Err(LineageError::Domain(AgentControlStoreError::Corrupt(_)))
        ));
        collection
            .replace_one(doc! { "_id": id }, original)
            .await
            .unwrap();
    }

    store
        .agents
        .delete_one(doc! { "_id": claimed.id.as_str() })
        .await
        .unwrap();
    let mut session = client.start_session().snapshot(true).await.unwrap();
    assert!(matches!(
        load_control_snapshot(&store, operation.request.target(), &mut session, true,).await,
        Err(LineageError::Domain(AgentControlStoreError::NotFound))
    ));

    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn agent_control_parent_fence_conflicts_with_a_concurrent_replacement() {
    let (client, name) = database("agent_control_parent_fence").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, _, claimed) = claimed_agent_work_fixture(&store, "parent_fence").await;
    let target = AgentControlTarget::from(&claimed);
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let snapshot = load_control_snapshot(&store, &target, &mut session, false)
        .await
        .unwrap();

    let mut workspace = store
        .get_workspace(&claimed.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let expected_revision = workspace.revision;
    workspace.next_worktree_number += 1;
    workspace.revision += 1;
    store
        .save_workspace(expected_revision, workspace)
        .await
        .unwrap();

    match fence_control_lineage(
        &store,
        &snapshot,
        "ctl_00000000000000000000000000000000",
        &mut session,
    )
    .await
    {
        Err(LineageError::Domain(AgentControlStoreError::Conflict)) => {}
        Err(LineageError::Driver(error)) if is_transient(&error) => {}
        Err(_) => panic!("stale parent fence returned an unexpected error"),
        Ok(()) => panic!("stale parent fence must not commit"),
    }
    session.abort_transaction().await.unwrap();

    let worktree = store
        .worktrees
        .find_one(doc! { "_id": claimed.worktree_id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert!(worktree.get("last_agent_control_operation").is_none());

    client.database(&name).drop().await.unwrap();
}
