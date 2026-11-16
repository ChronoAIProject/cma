//! Retained creation inputs share the terminal capacity refusal boundary.
use super::super::ceilings_tests::configure;
use super::*;
use layer2_core::admission::AdmissionCeilings;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_first_message_capacity_refusal_is_terminal_without_dispatch_or_replay() {
    let (client, name) = database("legacy_session_capacity").await;
    let (store, mut agent, _, execution, _, sends) = composed(&client, &name).await;
    let revision = agent.revision;
    agent.conversation.first_message =
        Some(FirstMessageRecord::new("creation-capacity", "Begin", NOW).unwrap());
    agent.work = AgentWorkState::new(AgentWorkStep::DispatchFirstMessage, NOW.into());
    agent.work.next_fence = 2;
    agent.work.lease = Some(AgentWorkLease {
        holder: "legacy-capacity-worker".into(),
        fence: 1,
        expires_at: "2099-01-01T00:00:00Z".into(),
    });
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    configure(
        &store,
        AdmissionCeilings {
            active_sessions: Some(0),
            ..Default::default()
        },
    )
    .await;
    let authority = AgentWorkAuthority::new("legacy-capacity-worker".into(), 1).unwrap();
    let outcome = execution
        .operations
        .dispatch_queued_first_message(&agent.id, authority.clone())
        .await
        .unwrap();
    let FirstMessageDispatchOutcome::Accepted(refused) = outcome else {
        panic!("capacity must finish the retained input");
    };
    assert_eq!(refused.state, FirstMessageDelivery::Failed);
    assert_eq!(
        refused.failure.as_ref().unwrap().code,
        "session_capacity_unavailable"
    );
    assert!(!refused.failure.as_ref().unwrap().retryable);
    assert_eq!(refused.turn_sequence, None);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let retained = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(retained.status, AgentStatus::Ready);
    assert!(retained.conversation.active_turn.is_none());
    configure(&store, AdmissionCeilings::default()).await;
    assert_eq!(
        execution
            .operations
            .dispatch_queued_first_message(&agent.id, authority)
            .await
            .unwrap(),
        FirstMessageDispatchOutcome::Accepted(refused)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(retained));
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    client.database(&name).drop().await.unwrap();
}
