use super::*;

#[tokio::test]
async fn registry_rejects_stale_generations_fences_and_attachment_tokens() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let recorder = Arc::new(NoopRecorder);
    let id = AgentId::generate();
    let (first_input, first_updates, _first_sender) = channels();
    let first = registry
        .attach(
            id.clone(),
            "conversation-1".into(),
            1,
            10,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            first_input,
            first_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&first).unwrap();
    let first_target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    };
    assert!(registry.resolve(&first_target).is_some());
    let runtime_target = AgentRuntimeTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    };
    let connection = AgentRuntimeRegistry::resolve(registry.as_ref(), &runtime_target)
        .await
        .unwrap();
    assert!(connection
        .capabilities
        .supports(RuntimeCapability::Conversation));
    assert_eq!(
        AgentRuntimeRegistry::resolve(
            registry.as_ref(),
            &AgentRuntimeTarget {
                runtime_generation: 2,
                ..runtime_target.clone()
            }
        )
        .await
        .unwrap_err(),
        AgentRuntimeError::StaleGeneration
    );
    assert_eq!(
        AgentRuntimeRegistry::resolve(
            registry.as_ref(),
            &AgentRuntimeTarget {
                conversation_id: "conversation-other".into(),
                ..runtime_target.clone()
            }
        )
        .await
        .unwrap_err(),
        AgentRuntimeError::NotFound
    );
    assert_eq!(
        AgentRuntimeRegistry::resolve(
            registry.as_ref(),
            &AgentRuntimeTarget {
                agent_id: AgentId::generate(),
                ..runtime_target
            }
        )
        .await
        .unwrap_err(),
        AgentRuntimeError::NotFound
    );

    let (stale_input, stale_updates, _stale_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-1".into(),
                1,
                9,
                RuntimeCapabilities::default(),
                stale_input,
                stale_updates,
                recorder.clone(),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    registry.fence(&id, Some(1), 9);
    assert!(registry.resolve(&first_target).is_some());

    let (newer_input, newer_updates, _newer_sender) = channels();
    let newer = registry
        .attach(
            id.clone(),
            "conversation-1".into(),
            1,
            11,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            newer_input,
            newer_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.rollback(&first);
    assert!(registry.target_is_attached(&first_target));
    registry.publish_and_activate(&first).unwrap();
    registry.publish_and_activate(&newer).unwrap();
    registry.fence(&id, Some(1), 10);
    assert!(registry.resolve(&first_target).is_some());
    registry.fence(&id, Some(1), 11);
    assert!(registry.resolve(&first_target).is_none());
    let (fenced_input, fenced_updates, _fenced_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-1".into(),
                1,
                11,
                RuntimeCapabilities::default(),
                fenced_input,
                fenced_updates,
                recorder.clone(),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );

    let (next_input, next_updates, _next_sender) = channels();
    let next = registry
        .attach(
            id.clone(),
            "conversation-2".into(),
            2,
            12,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            next_input,
            next_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&next).unwrap();
    let (old_input, old_updates, _old_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id.clone(),
                "conversation-1".into(),
                1,
                13,
                RuntimeCapabilities::default(),
                old_input,
                old_updates,
                recorder,
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
    let final_target = AgentRuntimeTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-2".into(),
        runtime_generation: 2,
    };
    assert!(
        AgentRuntimeRegistry::resolve(registry.as_ref(), &final_target)
            .await
            .is_ok()
    );
    AgentRuntimeRegistry::fence(registry.as_ref(), &id, 13)
        .await
        .unwrap();
    assert_eq!(
        AgentRuntimeRegistry::resolve(registry.as_ref(), &final_target)
            .await
            .unwrap_err(),
        AgentRuntimeError::NotFound
    );
    let (late_input, late_updates, _late_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id,
                "conversation-2".into(),
                2,
                13,
                RuntimeCapabilities::default(),
                late_input,
                late_updates,
                Arc::new(NoopRecorder),
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
}

#[tokio::test]
async fn registry_orders_authority_lexicographically_by_runtime_generation() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let recorder = Arc::new(NoopRecorder);
    let id = AgentId::generate();
    let (old_input, old_updates, _old_sender) = channels();
    let old = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            7,
            90,
            RuntimeCapabilities::default(),
            old_input,
            old_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&old).unwrap();
    registry.fence(&id, Some(7), 100);

    let (new_input, new_updates, _new_sender) = channels();
    let replacement = registry
        .attach(
            id.clone(),
            "conversation-stable".into(),
            8,
            2,
            RuntimeCapabilities::default(),
            new_input,
            new_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&replacement).unwrap();
    let target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-stable".into(),
        runtime_generation: 8,
    };
    assert!(registry.resolve(&target).is_some());

    registry.fence(&id, Some(7), 1_000);
    assert!(registry.resolve(&target).is_some());
    let (stale_input, stale_updates, _stale_sender) = channels();
    assert_eq!(
        registry
            .attach(
                id,
                "conversation-stable".into(),
                7,
                1_001,
                RuntimeCapabilities::default(),
                stale_input,
                stale_updates,
                recorder,
            )
            .unwrap_err(),
        LiveAgentRegistryError::StaleGeneration
    );
}

#[tokio::test]
async fn staged_registry_replacement_preserves_active_connection_until_promotion() {
    let registry = Arc::new(LiveAgentRegistry::default());
    let recorder = Arc::new(NoopRecorder);
    let id = AgentId::generate();
    let (active_input, active_updates, _active_sender) = channels();
    let active = registry
        .attach(
            id.clone(),
            "conversation-1".into(),
            1,
            10,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            active_input,
            active_updates,
            recorder.clone(),
        )
        .unwrap();
    registry.publish_and_activate(&active).unwrap();
    let active_target = LiveAgentTarget {
        agent_id: id.clone(),
        conversation_id: "conversation-1".into(),
        runtime_generation: 1,
    };

    let (candidate_input, candidate_updates, _candidate_sender) = channels();
    let candidate = registry
        .attach(
            id.clone(),
            "conversation-2".into(),
            2,
            11,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            candidate_input,
            candidate_updates,
            recorder,
        )
        .unwrap();
    let candidate_target = LiveAgentTarget {
        agent_id: id,
        conversation_id: "conversation-2".into(),
        runtime_generation: 2,
    };
    assert!(registry.resolve(&active_target).is_some());
    assert!(registry.resolve(&candidate_target).is_none());

    registry.rollback(&candidate);
    assert!(registry.resolve(&active_target).is_some());
    assert_eq!(
        registry.publish_and_activate(&candidate).unwrap_err(),
        LiveAgentRegistryError::NotAttached
    );
}
