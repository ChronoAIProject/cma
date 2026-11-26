use super::*;
#[path = "response_tests.rs"]
mod response_tests;
use crate::profiles::{AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort};
use crate::sandbox::{AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState};
use crate::{
    AgentId, AgentProfileId, RuntimeCapabilities, RuntimeCapability, SandboxId, SubjectId,
    WorkspaceId, WorktreeId,
};

const NOW: &str = "2026-11-11T00:00:00Z";

fn fixture(
    id: &str,
    worktree_id: &str,
    workspace_id: &str,
    sandbox_id: &str,
    owner: &str,
    status: AgentStatus,
) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(worktree_id.into()),
        workspace_id: WorkspaceId(workspace_id.into()),
        sandbox_id: SandboxId(sandbox_id.into()),
        owner: SubjectId::new(owner),
        creation_key: format!("manual:{id}"),
        creation_fingerprint: "d".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId(format!("profile-{id}")),
            revision: 1,
            digest: "e".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("model-a".into()),
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: RuntimeCapabilities::new([
            RuntimeCapability::Conversation,
            RuntimeCapability::Usage,
            RuntimeCapability::Resources,
        ]),
        status,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: None,
            runtime_generation: None,
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: None,
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

fn agent() -> Agent {
    fixture(
        "agt_projection_fixture",
        "wt_fixture",
        "ws_fixture",
        "sbx_fixture",
        "owner-fixture",
        AgentStatus::Ready,
    )
}

fn apply(
    state: &mut AgentLogProjectionState,
    agent: &mut Agent,
    kind: AgentLogKind,
) -> Vec<AgentLogEvidenceChange> {
    let entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: state.through_seq + 1,
        appended_at: NOW.into(),
        kind,
    };
    let result = state
        .observe(agent, &entry, AgentLogProjectionState::queued_owner(&entry))
        .unwrap();
    agent.conversation.last_seq = entry.seq;
    result
}

pub(super) fn opened() -> (Agent, AgentLogProjectionState) {
    let mut agent = agent();
    let mut state = AgentLogProjectionState::default();
    let created = AgentLogKind::Created {
        workspace_id: agent.workspace_id.clone(),
        worktree_id: agent.worktree_id.clone(),
    };
    apply(&mut state, &mut agent, created);
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
    );
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    (agent, state)
}

fn start(agent: &mut Agent, state: &mut AgentLogProjectionState, model: Option<&str>) -> u64 {
    let seq = state.through_seq + 1;
    apply(
        state,
        agent,
        AgentLogKind::TurnStarted {
            turn_sequence: seq,
            origin_key: Some(format!("fixture-{seq}")),
            model: model.map(str::to_owned),
        },
    );
    apply(
        state,
        agent,
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
    );
    agent.conversation.active_turn = Some(seq);
    state.validate_aggregate(agent).unwrap();
    seq
}

#[test]
fn long_turn_keeps_original_model_and_native_identity_with_bounded_state() {
    let (mut agent, mut state) = opened();
    let sequence = start(&mut agent, &mut state, Some("original-model"));
    agent.profile.content.model = Some("changed-model".into());
    for _ in 0..30_000 {
        apply(
            &mut state,
            &mut agent,
            AgentLogKind::AgentMessage {
                content: "output".repeat(100),
            },
        );
    }
    state.validate_aggregate(&agent).unwrap();
    let facts = state.active.as_ref().unwrap();
    assert_eq!(facts.model.as_deref(), Some("original-model"));
    assert_eq!(facts.conversation_id, "native-fixture");
    assert_eq!(facts.runtime_generation, 1);
    assert_eq!(facts.first_assistant_sequence, Some(sequence + 2));
    assert!(serde_json::to_vec(&state).unwrap().len() < 4096);
    let decoded: AgentLogProjectionState =
        serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(decoded, state);
}

fn usage(turn: u64, input: u64) -> AgentLogKind {
    AgentLogKind::UsageUpdated {
        input_tokens: input,
        output_tokens: 2,
        context_used: 4,
        context_window: Some(8192),
        turn_sequence: Some(turn),
        turn_input_tokens: input,
        turn_output_tokens: 2,
    }
}

#[test]
fn terminal_and_late_usage_keep_the_original_turn_binding() {
    let (mut agent, mut state) = opened();
    let first = start(&mut agent, &mut state, None);
    apply(&mut state, &mut agent, usage(first, 10));
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::TurnCompleted {
            turn_sequence: first,
        },
    );
    agent.conversation.active_turn = None;
    assert!(state.active.is_none());
    apply(&mut state, &mut agent, usage(first, 12));
    state.validate_aggregate(&agent).unwrap();
    let closed = state.latest_completed.as_ref().unwrap();
    assert!(closed.model.is_none());
    assert_eq!(closed.usage.unwrap().input_tokens, 12);
    assert!(closed.usage_sequence > closed.end_sequence);
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::ConversationOpened {
            conversation_id: "next-native".into(),
            runtime_generation: 2,
        },
    );
    agent.conversation.conversation_id = Some("next-native".into());
    agent.conversation.runtime_generation = Some(2);
    let second = start(&mut agent, &mut state, Some("next-model"));
    apply(&mut state, &mut agent, usage(second, 20));
    state.validate_aggregate(&agent).unwrap();
    assert_eq!(
        state.latest_completed.as_ref().unwrap().runtime_generation,
        1
    );
    assert_eq!(
        state
            .latest_completed
            .as_ref()
            .unwrap()
            .usage
            .unwrap()
            .input_tokens,
        12
    );
    assert_eq!(state.active.as_ref().unwrap().runtime_generation, 2);
    assert_eq!(
        state.active.as_ref().unwrap().usage.unwrap().input_tokens,
        20
    );
}

#[test]
fn usage_without_original_turn_and_incomplete_steering_fail_closed() {
    let (mut agent, mut state) = opened();
    let turn = start(&mut agent, &mut state, None);
    let mut entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: state.through_seq + 1,
        appended_at: NOW.into(),
        kind: usage(turn + 1, 5),
    };
    assert!(state.clone().observe(&agent, &entry, None).is_err());
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::OperationDeliveryChanged {
            operation_sequence: entry.seq,
            operation: AgentOperationKind::Steer,
            state: AgentOperationDelivery::DispatchFenced,
        },
    );
    assert!(state.validate_aggregate(&agent).is_err());
    entry.seq = state.through_seq + 1;
    entry.kind = AgentLogKind::AgentMessage {
        content: "wrong adjacency".into(),
    };
    assert!(state.clone().observe(&agent, &entry, None).is_err());
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::OwnerMessage {
            content: "steering".into(),
        },
    );
    state.validate_aggregate(&agent).unwrap();
}

#[test]
fn generation_maxima_are_independent_required_source_facts() {
    let (mut agent, mut state) = opened();
    assert_eq!(
        (state.max_opened_generation, state.max_retired_generation),
        (1, 0)
    );
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::RuntimeGenerationRetired {
            previous_generation: 1,
            runtime_generation: 2,
        },
    );
    assert_eq!(
        (state.max_opened_generation, state.max_retired_generation),
        (1, 2)
    );
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::ConversationOpened {
            conversation_id: "second-native".into(),
            runtime_generation: 3,
        },
    );
    assert_eq!(
        (state.max_opened_generation, state.max_retired_generation),
        (3, 2)
    );
    agent.conversation.conversation_id = Some("second-native".into());
    agent.conversation.runtime_generation = Some(3);
    state.validate_aggregate(&agent).unwrap();
    let value = serde_json::to_value(&state).unwrap();
    for name in ["max_opened_generation", "max_retired_generation"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(name);
        assert!(serde_json::from_value::<AgentLogProjectionState>(missing).is_err());
    }
    let original = state.max_opened_generation;
    state.max_opened_generation = 0;
    assert!(state.validate_aggregate(&agent).is_err());
    state.max_opened_generation = original;
    state.max_retired_generation = original;
    assert!(state.validate_aggregate(&agent).is_err());
}

#[test]
fn generation_maxima_certify_each_boundary_kind_after_later_native_changes() {
    let (mut agent, mut state) = opened();
    assert_eq!(state.max_opened_generation, 1);
    assert_eq!(state.max_retired_generation, 0);
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::RuntimeGenerationRetired {
            previous_generation: 1,
            runtime_generation: 2,
        },
    );
    agent.conversation.runtime_generation = Some(2);
    state.validate_aggregate(&agent).unwrap();
    assert_eq!(state.max_opened_generation, 1);
    assert_eq!(state.max_retired_generation, 2);
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::ConversationOpened {
            conversation_id: "later-native".into(),
            runtime_generation: 4,
        },
    );
    agent.conversation.conversation_id = Some("later-native".into());
    agent.conversation.runtime_generation = Some(4);
    state.validate_aggregate(&agent).unwrap();
    assert_eq!(state.max_opened_generation, 4);
    assert_eq!(state.max_retired_generation, 2);
    let row = serde_json::to_value(&state).unwrap();
    for field in ["max_opened_generation", "max_retired_generation"] {
        let mut missing = row.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<AgentLogProjectionState>(missing).is_err());
    }
    state.max_opened_generation = 0;
    assert!(state.validate_aggregate(&agent).is_err());
}

#[test]
fn queued_turns_require_exact_unconsumed_source_and_complete_queue_coverage() {
    let (mut agent, mut state) = opened();
    let queued_turn = |seq, source| AgentLogEntry {
        agent_id: agent.id.clone(),
        seq,
        appended_at: NOW.into(),
        kind: AgentLogKind::TurnStarted {
            turn_sequence: seq,
            origin_key: Some(format!("queued-message:{source}")),
            model: None,
        },
    };
    let invented = queued_turn(3, 1);
    assert!(state.clone().observe(&agent, &invented, Some(1)).is_err());
    let queued =
        crate::sandbox::QueuedAgentMessage::new(3, "original input".into(), NOW.into()).unwrap();
    agent.conversation.queued_messages.push(queued.clone());
    assert!(state.validate_aggregate(&agent).is_err());
    apply(
        &mut state,
        &mut agent,
        AgentLogKind::OwnerMessage {
            content: queued.content.clone(),
        },
    );
    state.validate_aggregate(&agent).unwrap();
    let original = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::OwnerMessage {
            content: queued.content.clone(),
        },
    };
    state.validate_queued_source(&original).unwrap();
    let mut changed = original.clone();
    changed.kind = AgentLogKind::OwnerMessage {
        content: "changed input".into(),
    };
    assert!(state.validate_queued_source(&changed).is_err());
    let start = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 4,
        appended_at: NOW.into(),
        kind: AgentLogKind::TurnStarted {
            turn_sequence: 4,
            origin_key: Some("queued-message:3".into()),
            model: None,
        },
    };
    state.observe(&agent, &start, Some(3)).unwrap();
    assert!(state.unconsumed_queue.is_empty());
    assert!(state.validate_queued_source(&original).is_err());
}
