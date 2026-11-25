use super::*;

fn begin_at(current: &Sandbox, at: &str) -> SandboxLifecycleMutation {
    let mut next = current.clone();
    next.begin_checkpoint_capture(at.into()).unwrap();
    mutation(current, next, at)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn periodic_capture_quiet_boundary_is_enforced_in_the_begin_transaction() {
    let (client, name) = database("capture_quiet_boundary").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, _, agent) = capture_fixture(&store, "quiet_boundary").await;
    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 },
            doc! { "$set": { "record.appended_at": NOW } },
        )
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    for at in [NOW, MINUTE, "2026-09-05T00:01:59Z"] {
        assert_eq!(
            reopened
                .commit_sandbox_lifecycle(begin_at(&parent, at))
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            reopened.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        assert!(reopened
            .sandbox_events(&parent.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
    reopened
        .commit_sandbox_lifecycle(begin_at(&parent, TWO_MINUTES))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .sandbox_events(&parent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn periodic_capture_quiet_rejects_future_activity_and_observes_a_newer_turn() {
    let (client, name) = database("capture_quiet_newer_turn").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, _, agent) = capture_fixture(&store, "quiet_future").await;
    store
        .agent_log
        .update_one(
            doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 },
            doc! { "$set": { "record.appended_at": "2099-01-01T00:00:00Z" } },
        )
        .await
        .unwrap();
    assert_eq!(
        store.commit_sandbox_lifecycle(begin(&parent)).await,
        Err(SandboxStoreError::Conflict)
    );

    let (parent, _, agent) = capture_fixture(&store, "quiet_turn").await;
    let stale_capture = begin(&parent);
    let admitted = begin_stored_conversation_turn(&store, agent).await;
    store.commit_agent_conversation(admitted).await.unwrap();
    assert_eq!(
        store.commit_sandbox_lifecycle(stale_capture).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(store.get_sandbox(&parent.id).await.unwrap(), Some(parent));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn periodic_capture_and_concurrent_owner_turn_have_one_winner_after_quiet() {
    let (client, name) = database("capture_quiet_admission_race").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for index in 0..8 {
        let (parent, _, agent) = capture_fixture(&store, &format!("quiet_race_{index}")).await;
        let request = begin(&parent);
        let admission = begin_stored_conversation_turn(&store, agent.clone()).await;
        let (capture, turn) = tokio::join!(
            store.commit_sandbox_lifecycle(request.clone()),
            store.commit_agent_conversation(admission.clone()),
        );
        match (capture, turn) {
            (Ok(()), Err(SandboxStoreError::CheckpointInProgress)) => {
                assert_capture_race_state(&store, &request.sandbox, &agent, 1).await;
            }
            (
                capture @ (Ok(()) | Err(SandboxStoreError::Conflict)),
                Err(SandboxStoreError::Conflict),
            ) => {
                // Bounded contention can prevent either call from reaching
                // the domain fence. Verify durable state before retrying.
                if capture.is_err() {
                    assert_capture_race_state(&store, &parent, &agent, 0).await;
                    store
                        .commit_sandbox_lifecycle(request.clone())
                        .await
                        .unwrap();
                }
                assert_capture_race_state(&store, &request.sandbox, &agent, 1).await;
                assert_eq!(
                    store.commit_agent_conversation(admission).await,
                    Err(SandboxStoreError::CheckpointInProgress),
                );
                assert_capture_race_state(&store, &request.sandbox, &agent, 1).await;
            }
            (Err(SandboxStoreError::Conflict), Ok(logs)) => {
                assert_capture_race_state(&store, &parent, &admission.agent, 0).await;
                assert_eq!(
                    store
                        .agent_log(&agent.id, agent.conversation.last_seq, 10)
                        .await
                        .unwrap(),
                    logs
                );
            }
            result => panic!("periodic capture and owner turn were not exclusive: {result:?}"),
        }
    }
    client.database(&name).drop().await.unwrap();
}

async fn assert_capture_race_state(
    store: &MongoSandboxStore,
    parent: &Sandbox,
    agent: &Agent,
    sandbox_events: usize,
) {
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap().as_ref(),
        Some(parent)
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().as_ref(),
        Some(agent)
    );
    assert_eq!(
        store.sandbox_events(&parent.id, 0, 10).await.unwrap().len(),
        sandbox_events
    );
    assert_eq!(
        store
            .agent_log
            .count_documents(doc! { "agent_id": agent.id.as_str() })
            .await
            .unwrap(),
        agent.conversation.last_seq,
    );
}
