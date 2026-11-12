use super::*;

#[tokio::test]
async fn exact_stream_eof_removes_the_route_and_reports_complete_ownership() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let probe = Arc::new(ConnectionLossProbe::default());
    assert!(registry.install_connection_loss_handler(probe.clone()));
    let id = AgentId::generate();
    let (input, updates, sender) = channels();
    let attachment = registry
        .attach(
            id.clone(),
            "conversation-eof".into(),
            3,
            9,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input,
            updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    registry.publish_and_activate(&attachment).unwrap();
    let identity = attachment.identity();

    drop(sender);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !probe.seen.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("EOF callback completed");

    assert_eq!(*probe.seen.lock().unwrap(), vec![identity]);
    let (stale_input, stale_updates, _stale_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-eof".into(),
                3,
                9,
                RuntimeCapabilities::default(),
                stale_input,
                stale_updates,
                Arc::new(NoopRecorder),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    assert!(registry
        .resolve(&LiveAgentTarget {
            agent_id: id,
            conversation_id: "conversation-eof".into(),
            runtime_generation: 3,
        })
        .is_none());
}

#[tokio::test]
async fn stale_eof_cannot_remove_or_recover_a_replacement_route() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let probe = Arc::new(ConnectionLossProbe::default());
    assert!(registry.install_connection_loss_handler(probe.clone()));
    let id = AgentId::generate();
    let (old_input, old_updates, old_sender) = channels();
    let old = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            4,
            20,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            old_input,
            old_updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    registry.publish_and_activate(&old).unwrap();
    let stale_identity = old.identity();
    let (new_input, new_updates, _new_sender) = channels();
    let replacement = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            4,
            21,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            new_input,
            new_updates,
            Arc::new(NoopRecorder),
        )
        .unwrap();

    drop(old_sender);
    let target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-stable".into(),
        runtime_generation: 4,
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if registry.resolve(&target).is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stale route observed EOF");
    assert!(probe.seen.lock().unwrap().is_empty());
    registry.publish_and_activate(&replacement).unwrap();
    registry.connection_ended(stale_identity).await;

    assert!(probe.seen.lock().unwrap().is_empty());
    assert!(registry
        .resolve(&LiveAgentTarget {
            agent_id: id,
            ..target
        })
        .is_some());
}

#[tokio::test]
async fn durable_eof_schedules_and_republishes_the_same_conversation() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let admitted = harness.admit("eof-recovery", vec![]).await;
    let ready = harness.drive_ready(&admitted.id).await;
    let target = Harness::target(&ready);
    let attachment = harness
        .registry
        .attachment(&target, ready.work.next_fence - 1)
        .unwrap()
        .unwrap();

    harness
        .registry
        .connection_ended(attachment.identity())
        .await;
    let scheduled = harness.stored(&ready.id).await;
    assert_eq!(scheduled.status, AgentStatus::Ready);
    assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(scheduled.conversation, ready.conversation);

    harness
        .run(&ready.id, SystemTime::now() + Duration::from_secs(1))
        .await;
    let recovered = harness.stored(&ready.id).await;
    assert_eq!(recovered.work.step, AgentWorkStep::None);
    assert_eq!(
        recovered.conversation.conversation_id,
        ready.conversation.conversation_id
    );
    assert!(harness.registry.resolve(&target).is_some());
}

#[tokio::test]
async fn restart_reconciles_ready_working_and_waiting_approval_agents() {
    for status in [
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
    ] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let admitted = harness.admit(status.name(), vec![]).await;
        let mut agent = harness.drive_ready(&admitted.id).await;
        let target = Harness::target(&agent);
        harness.registry.fence(
            &agent.id,
            agent.conversation.runtime_generation,
            agent.work.next_fence - 1,
        );
        if status != AgentStatus::Ready {
            let expected = agent.revision;
            agent.status = status;
            agent.conversation.active_turn = Some(41);
            agent.revision += 1;
            harness
                .store
                .save_agent(expected, agent.clone())
                .await
                .unwrap();
        }

        assert_eq!(
            harness
                .worker
                .reconcile_connections(harness.now)
                .await
                .unwrap(),
            1
        );
        let scheduled = harness.stored(&agent.id).await;
        assert_eq!(scheduled.status, status);
        assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
        assert_eq!(
            scheduled.conversation.active_turn,
            agent.conversation.active_turn
        );
        harness.run(&agent.id, harness.now).await;
        let recovered = harness.stored(&agent.id).await;
        assert_eq!(recovered.status, status);
        assert_eq!(recovered.work.step, AgentWorkStep::None);
        assert_eq!(
            recovered.conversation.active_turn,
            agent.conversation.active_turn
        );
        assert!(harness.registry.resolve(&target).is_some());
    }
}
