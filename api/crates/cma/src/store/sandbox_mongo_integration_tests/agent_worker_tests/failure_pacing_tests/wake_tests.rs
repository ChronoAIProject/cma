use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_wakes_on_each_parent_change_and_resets_before_base_due() {
    let (client, name) = database("failure_pacing_parent_wake").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for parent in ["sandbox", "workspace", "worktree"] {
        let agent = ready_conversation_fixture(&store, parent).await;
        let observed = observation(&store, &agent).await;
        fail(&store, &agent.id, 0, Some(observed.clone()), CLASS).await;
        fail(&store, &agent.id, 30, Some(observed), CLASS).await;
        assert!(!store
            .due_agent_work(&stamp(31), 200)
            .await
            .unwrap()
            .contains(&agent.id));
        let (collection, id) = match parent {
            "sandbox" => (&store.sandboxes, agent.sandbox_id.as_str()),
            "workspace" => (&store.workspaces, agent.workspace_id.as_str()),
            _ => (&store.worktrees, agent.worktree_id.as_str()),
        };
        collection
            .update_one(
                doc! {"_id": id},
                doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
            )
            .await
            .unwrap();
        assert!(store
            .due_agent_work(&stamp(31), 200)
            .await
            .unwrap()
            .contains(&agent.id));
        let changed = observation(&store, &agent).await;
        fail(&store, &agent.id, 31, Some(changed), CLASS).await;
        assert_eq!(until(&store, &agent.id).await, stamp(61));
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_wakes_changed_agent_and_never_delays_owner_stop() {
    let (client, name) = database("failure_pacing_work_wake").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "work_wake").await;
    let observed = observation(&store, &agent).await;
    fail(&store, &agent.id, 0, Some(observed.clone()), CLASS).await;
    let prior = fail(&store, &agent.id, 30, Some(observed.clone()), CLASS).await;
    let mut changed = store.get_agent(&agent.id).await.unwrap().unwrap();
    let expected = changed.revision;
    changed.conversation.managed_guidance_revision += 1;
    changed.revision += 1;
    changed.updated_at = stamp(31);
    store.save_agent(expected, changed).await.unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(31), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    let latest = fail(&store, &agent.id, 31, Some(observed.clone()), CLASS).await;
    assert_eq!(until(&store, &agent.id).await, stamp(61));
    // A delayed hint writer cannot restore an old streak after a work change.
    crate::store::agent_failure_pacing::record(&store, &prior, &observed)
        .await
        .unwrap();
    assert_eq!(until(&store, &agent.id).await, stamp(61));
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.get_document("record").unwrap().contains_key(FIELD));
    assert_eq!(
        crate::store::agents::read_agent(&row).unwrap(),
        latest.agent
    );
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&latest.agent),
            reason: AgentStopReason::Owner,
            requested_at: stamp(32),
        })
        .await
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(32), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    let stop = store
        .claim_agent_work(&agent.id, "stop-worker", stamp(32), stamp(632))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stop.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(stop.work.attempts, 0);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_stale_observation_cannot_hold_new_authority() {
    let (client, name) = database("failure_pacing_race").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "race").await;
    let observed = observation(&store, &agent).await;
    fail(&store, &agent.id, 0, Some(observed.clone()), CLASS).await;
    // Authority changed after inspection and before failure publication.
    store
        .sandboxes
        .update_one(
            doc! {"_id": agent.sandbox_id.as_str()},
            doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
        )
        .await
        .unwrap();
    fail(&store, &agent.id, 30, Some(observed), CLASS).await;
    assert_eq!(
        store.due_agent_work(&stamp(31), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    assert!(store
        .claim_agent_work(&agent.id, "new-authority", stamp(31), stamp(631))
        .await
        .unwrap()
        .is_some());
    // The ordinary lease still prevents concurrent duplicate execution.
    assert!(store
        .claim_agent_work(&agent.id, "competitor", stamp(31), stamp(631))
        .await
        .unwrap()
        .is_none());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_never_installs_a_hint_for_a_refused_publication() {
    let (client, name) = database("failure_pacing_refused").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "refused").await;
    let observed = observation(&store, &agent).await;
    let claimed = store
        .claim_agent_work(&agent.id, "worker", stamp(0), stamp(600))
        .await
        .unwrap()
        .unwrap();
    let mutation = retry_mutation(&claimed, 0, CLASS);
    let mut stale = mutation.clone();
    stale.holder = "superseded-worker".into();
    assert_eq!(
        store.commit_agent_work_retry(stale, observed.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(claimed));
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(!row.contains_key(FIELD));
    store
        .commit_agent_work_retry(mutation, observed)
        .await
        .unwrap();
    assert_eq!(until(&store, &agent.id).await, stamp(30));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn failure_pacing_concurrent_agent_change_wakes_after_failure_publication() {
    let (client, name) = database("failure_pacing_agent_race").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let agent = ready_conversation_fixture(&store, "agent_race").await;
    let observed = observation(&store, &agent).await;
    let mut claimed = store
        .claim_agent_work(&agent.id, "worker", stamp(0), stamp(600))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observed.agent_revision, claimed.revision);
    let expected = claimed.revision;
    claimed.conversation.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            &claimed.creation_key,
            "pending work",
            stamp(0),
        )
        .unwrap(),
    );
    claimed.revision += 1;
    store.save_agent(expected, claimed.clone()).await.unwrap();
    store
        .commit_agent_work_retry(retry_mutation(&claimed, 0, CLASS), observed)
        .await
        .unwrap();
    assert_eq!(
        store.due_agent_work(&stamp(1), 1).await.unwrap(),
        vec![agent.id.clone()]
    );
    let woken = store
        .claim_agent_work(&agent.id, "changed-work", stamp(1), stamp(601))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        woken.conversation.first_message,
        claimed.conversation.first_message
    );
    client.database(&name).drop().await.unwrap();
}
