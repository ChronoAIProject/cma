use super::*;

pub(super) fn conversation_agent_for_rules(id: &str) -> Agent {
    let mut agent = agent_for_worker(id, NOW);
    agent.status = AgentStatus::Ready;
    agent.conversation.conversation_id = Some(format!("conversation-{id}"));
    agent.conversation.runtime_generation = Some(1);
    agent.conversation.last_seq = 2;
    agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
    agent.ready_at = Some(NOW.into());
    agent.validate_conversation().unwrap();
    agent
}

pub(super) fn begin_conversation_turn(mut agent: Agent) -> AgentConversationMutation {
    let expected_agent_revision = agent.revision;
    let expected_last_seq = agent.conversation.last_seq;
    let turn_sequence = expected_last_seq + 1;
    agent
        .transition(AgentStatus::Working, NOW.into(), None)
        .unwrap();
    agent.conversation.active_turn = Some(turn_sequence);
    agent.conversation.last_seq += 2;
    AgentConversationMutation {
        settings_fence: None,
        command_claim: None,
        execution_fence: None,
        trigger_execution_fence: None,
        admission_guard: None,
        expected_agent_revision,
        expected_last_seq,
        runtime_generation: 1,
        agent,
        appended_at: NOW.into(),
        agent_logs: vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Inspect the durable state.".into(),
            },
        ],
    }
}

pub(super) async fn begin_stored_conversation_turn(
    store: &MongoSandboxStore,
    agent: Agent,
) -> AgentConversationMutation {
    let parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let mut mutation = begin_conversation_turn(agent);
    mutation.admission_guard = Some(layer2_core::sandbox::store::SandboxMutationGuard::capture(
        &parent,
    ));
    mutation
}

pub(super) async fn claim_interaction_fence(store: &MongoSandboxStore, id: &AgentId) -> Agent {
    use layer2_core::rfc3339_at;
    use std::time::Duration;

    // Only the lease needs database time. The cleanup fixtures use their fixed
    // timeline to publish events and drive the subsequent steps.
    let hello = store
        .client
        .database("admin")
        .run_command(doc! { "hello": 1 })
        .await
        .unwrap();
    let now = hello.get_datetime("localTime").unwrap().to_system_time();
    let claimed = store
        .claim_agent_work(
            id,
            "worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, AgentWorkStep::FenceInteraction);
    claimed
}
