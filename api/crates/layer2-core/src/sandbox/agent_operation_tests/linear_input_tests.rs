use super::*;
use std::collections::BTreeMap;

async fn fixture() -> (Arc<InMemorySandboxStore>, Agent) {
    let store = Arc::new(InMemorySandboxStore::default());
    seed_active_parent(&store).await;
    let mut agent = test_agent(AgentStatus::Ready, None);
    agent.conversation.last_seq = 2;
    store.create_agent(agent.clone()).await.unwrap();
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::Created {
                workspace_id: agent.workspace_id.clone(),
                worktree_id: agent.worktree_id.clone(),
            },
        )
        .await
        .unwrap();
    store
        .append_agent_log(
            &agent.id,
            NOW.into(),
            AgentLogKind::ConversationOpened {
                conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                runtime_generation: 3,
            },
        )
        .await
        .unwrap();
    (store, agent)
}

#[tokio::test]
async fn linear_admission_persists_metadata_with_the_turn_before_dispatch() {
    let (store, agent) = fixture().await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    let metadata = BTreeMap::from([("topic".into(), "investigation".into())]);
    let receipt = service
        .send_linear_message(
            &owner(),
            &agent.id,
            "Inspect".into(),
            None,
            metadata.clone(),
        )
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let logs = store.agent_log(&agent.id, 0, 40).await.unwrap();
    assert!(logs.iter().any(|entry| matches!(&entry.kind, AgentLogKind::TurnMetadataSet { turn_sequence, metadata: stored } if *turn_sequence == receipt.turn_sequence && stored == &metadata)));
    let current = store.get_agent(&agent.id).await.unwrap().unwrap();
    let projected = crate::sandbox::agent_reads_fold::fold_agent_log(&current, &logs).unwrap();
    assert_eq!(projected.transcript.turns[0].metadata, metadata);
}

#[tokio::test]
async fn completed_predecessor_is_required_and_stale_requests_never_dispatch() {
    let (store, agent) = fixture().await;
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            completing_sender(store.clone(), agent.id.clone(), false),
        ),
    );
    let first = service
        .send_linear_message(&owner(), &agent.id, "First".into(), None, BTreeMap::new())
        .await
        .unwrap();
    let before = store.get_agent(&agent.id).await.unwrap().unwrap();
    for previous in [None, Some(first.turn_sequence + 1)] {
        assert!(matches!(
            service
                .send_linear_message(
                    &owner(),
                    &agent.id,
                    "Stale".into(),
                    previous,
                    BTreeMap::new()
                )
                .await,
            Err(AgentOperationError::PreviousTurnConflict)
        ));
        assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), before);
    }
    let next = service
        .send_linear_message(
            &owner(),
            &agent.id,
            "Second".into(),
            Some(first.turn_sequence),
            BTreeMap::new(),
        )
        .await
        .unwrap();
    assert!(next.turn_sequence > first.turn_sequence);
}

#[tokio::test]
async fn concurrent_linear_requests_have_only_one_winner_even_when_the_turn_completes_immediately()
{
    let (store, agent) = fixture().await;
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(
            full_capabilities(),
            completing_sender(store.clone(), agent.id.clone(), false),
        ),
    );
    let caller = owner();
    let (left, right) = tokio::join!(
        service.send_linear_message(&caller, &agent.id, "Left".into(), None, BTreeMap::new()),
        service.send_linear_message(&caller, &agent.id, "Right".into(), None, BTreeMap::new()),
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let logs = store.agent_log(&agent.id, 0, 40).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn metadata_bounds_and_owner_authority_are_checked_before_dispatch() {
    let (store, agent) = fixture().await;
    let (sender, inputs) = recording_sender(Ok(()));
    let service = AgentOperationService::new(
        store.clone(),
        TestRuntimeRegistry::new(full_capabilities(), sender),
    );
    for metadata in [
        BTreeMap::from([("k".repeat(65), String::new())]),
        BTreeMap::from([("key".into(), "v".repeat(513))]),
        (0..17)
            .map(|index| (index.to_string(), String::new()))
            .collect(),
    ] {
        assert!(matches!(
            service
                .send_linear_message(&owner(), &agent.id, "Invalid".into(), None, metadata)
                .await,
            Err(AgentOperationError::InvalidRequest(_))
        ));
    }
    assert!(matches!(
        service
            .send_linear_message(
                &other_owner(),
                &agent.id,
                "Foreign".into(),
                None,
                BTreeMap::new()
            )
            .await,
        Err(AgentOperationError::NotFound)
    ));
    assert!(matches!(
        service
            .send_linear_message(
                &unauthorized_owner(),
                &agent.id,
                "Forbidden".into(),
                None,
                BTreeMap::new()
            )
            .await,
        Err(AgentOperationError::Forbidden)
    ));
    assert!(inputs.lock().unwrap().is_empty());
    assert_eq!(store.get_agent(&agent.id).await.unwrap().unwrap(), agent);
}
