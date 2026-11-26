use super::*;
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort,
};
use layer2_core::sandbox::*;
use layer2_core::{
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

pub(in crate::store::agent_log_segments) fn opened() -> (Agent, Head, AgentLogProjectionState) {
    let mut agent = fixture(
        "agt_delta_fixture",
        "wt_fixture",
        "ws_fixture",
        "sbx_fixture",
        "owner-fixture",
        AgentStatus::Ready,
    );
    agent.conversation.last_seq = 2;
    agent.conversation.conversation_id = Some("native-fixture".into());
    agent.conversation.runtime_generation = Some(1);
    let mut fold = BoundedAgentLogFold::default();
    for (index, kind) in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: "native-fixture".into(),
            runtime_generation: 1,
        },
    ]
    .into_iter()
    .enumerate()
    {
        fold.observe(
            &agent,
            &AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: index as u64 + 1,
                appended_at: NOW.into(),
                kind,
            },
        )
        .unwrap();
    }
    let state = fold.finish(&agent).unwrap();
    let head = Head {
        agent_id: agent.id.0.clone(),
        format_version: 1,
        incarnation: digest::incarnation(&agent),
        revision: 1,
        active_ordinal: 0,
        active_start_seq: 1,
        last_seq: 2,
        active_count: 2,
        active_bytes: 512,
        previous_seal: None,
        lifetime_count: 2,
        lifetime_bytes: 512,
        projection_format: 1,
        evidence_generation: "fixture-generation".into(),
        projection_through: 2,
        migration_id: super::super::MIGRATION.into(),
        required_reader: super::super::READER.into(),
    };
    (agent, head, state)
}

#[test]
fn delta_coalesces_incomplete_turn_intermediates_before_validating_evidence() {
    let (agent, head, state) = opened();
    let logs = [
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: Some("fixture-command".into()),
            model: Some("original-model".into()),
        },
        AgentLogKind::OwnerMessage {
            content: "fixture input".into(),
        },
        AgentLogKind::UsageUpdated {
            input_tokens: 8,
            output_tokens: 2,
            context_used: 10,
            context_window: Some(8192),
            turn_sequence: Some(3),
            turn_input_tokens: 8,
            turn_output_tokens: 2,
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, kind)| AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 3 + offset as u64,
        appended_at: NOW.into(),
        kind,
    })
    .collect::<Vec<_>>();
    let mut next = agent.clone();
    next.conversation.last_seq = 6;
    next.revision += 1;
    let mut after = head.clone();
    after.revision = 2;
    after.last_seq = 6;
    after.active_count = 6;
    after.active_bytes += 1024;
    after.lifetime_count = 6;
    after.lifetime_bytes = after.active_bytes;
    after.projection_through = 6;
    let delta = plan(
        &agent,
        &next,
        &head,
        &after,
        &state,
        &logs,
        SourceFormat::Segmented,
    )
    .unwrap();
    assert_eq!(delta.expected_revision, 1);
    assert_eq!(delta.expected_through, 2);
    assert_eq!(delta.evidence.len(), 2);
    let turn = delta
        .evidence
        .iter()
        .find(|row| row.get_str("kind").ok() == Some("turn"))
        .unwrap();
    let projection::EvidenceRecord::Turn { facts } =
        projection::read_evidence(&after, "turn", "3", turn.clone()).unwrap()
    else {
        panic!("turn evidence")
    };
    assert_eq!(facts.end_sequence, Some(6));
    assert_eq!(facts.model.as_deref(), Some("original-model"));
    assert_eq!(facts.usage.unwrap().input_tokens, 8);
    let stored: Projection = bson::deserialize_from_document(delta.projection).unwrap();
    assert!(stored.record.active.is_none());
    assert_eq!(stored.record.latest_completed.unwrap().turn_sequence, 3);
    assert_eq!(state.through_seq, 2);
}

#[test]
fn metadata_only_publication_advances_proof_without_fabricating_events() {
    let (agent, head, state) = opened();
    let mut after = head.clone();
    after.revision += 1;
    let delta = plan(
        &agent,
        &agent,
        &head,
        &after,
        &state,
        &[],
        SourceFormat::Segmented,
    )
    .unwrap();
    assert!(delta.evidence.is_empty());
    let stored: Projection = bson::deserialize_from_document(delta.projection).unwrap();
    assert_eq!(stored.record, state);
    assert_eq!(stored.through_seq, head.last_seq);
    assert_eq!(stored.head_revision, head.revision + 1);
    assert!(plan(
        &agent,
        &agent,
        &head,
        &head,
        &state,
        &[],
        SourceFormat::Segmented
    )
    .is_err());
}

#[test]
fn invalid_source_or_semantic_batch_leaves_the_input_projection_untouched() {
    let (agent, head, state) = opened();
    let original = state.clone();
    let mut stale = state.clone();
    stale.through_seq -= 1;
    let mut after = head.clone();
    after.revision += 1;
    assert!(plan(
        &agent,
        &agent,
        &head,
        &after,
        &stale,
        &[],
        SourceFormat::Segmented
    )
    .is_err());
    after.incarnation = "f".repeat(64);
    assert!(plan(
        &agent,
        &agent,
        &head,
        &after,
        &state,
        &[],
        SourceFormat::Segmented
    )
    .is_err());
    let invalid = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 3,
        appended_at: NOW.into(),
        kind: AgentLogKind::AgentMessage {
            content: "no active turn".into(),
        },
    };
    let mut next = agent.clone();
    next.conversation.last_seq = 3;
    after = head.clone();
    after.revision += 1;
    after.last_seq = 3;
    after.active_count = 3;
    after.lifetime_count = 3;
    after.projection_through = 3;
    assert!(plan(
        &agent,
        &next,
        &head,
        &after,
        &state,
        &[invalid],
        SourceFormat::Segmented
    )
    .is_err());
    assert_eq!(state, original);
}
