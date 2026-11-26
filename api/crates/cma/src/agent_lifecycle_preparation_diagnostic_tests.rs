use super::*;
use serde_json::json;

const FAILED_AT: &str = "2026-11-04T00:00:00Z";

fn failure_fixture() -> (Agent, AgentLogEntry, i64) {
    let agent: Agent = serde_json::from_value(json!({
        "id": "agt_fixture", "worktree_id": "wkt_fixture", "workspace_id": "wks_fixture",
        "sandbox_id": "sbx_fixture", "owner": "owner", "creation_key": "fixture",
        "creation_fingerprint": "a".repeat(64),
        "profile": {"id": "agp_fixture", "revision": 1, "digest": "b".repeat(64),
            "content": {"thinking_effort": "high", "permission_level": "autonomous"}},
        "status": "failed", "conversation": {"last_seq": 7, "queued_messages": []},
        "work": layer2_core::sandbox::AgentWorkState::new(AgentWorkStep::None, FAILED_AT.into()),
        "failure": {"class": "agent_runtime_rejected", "detail": "private diagnostic fixture"},
        "created_at": FAILED_AT, "updated_at": FAILED_AT, "origin": {"type": "manual"},
        "revision": 3
    }))
    .unwrap();
    let entry = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: agent.conversation.last_seq,
        appended_at: FAILED_AT.into(),
        kind: AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::OpenConversation,
            failure: agent.failure.clone().unwrap(),
        },
    };
    let operation_ms =
        i64::try_from(layer2_core::seconds_from_rfc3339(FAILED_AT).unwrap()).unwrap() * 1000 + 180;
    (agent, entry, operation_ms)
}

#[test]
fn preparation_diagnostic_names_the_proven_step_and_uses_public_failure_text() {
    let (mut agent, mut entry, operation_ms) = failure_fixture();
    assert_eq!(
        preparation_diagnostic(&agent, &entry, operation_ms).as_deref(),
        Some("agent_runtime_rejected: Agent preparation failed while opening the Agent conversation.")
    );
    let failure = agent.failure.as_mut().unwrap();
    failure.class = "private category fixture".into();
    entry.kind = AgentLogKind::WorkStepFailed {
        step: AgentWorkStep::MaterializeHome,
        failure: failure.clone(),
    };
    assert_eq!(
        preparation_diagnostic(&agent, &entry, operation_ms).as_deref(),
        Some("agent_preparation_failed: Agent preparation failed while preparing the Agent home.")
    );
}

#[test]
fn preparation_diagnostic_requires_matching_prepublication_failure_evidence() {
    let (agent, mut entry, operation_ms) = failure_fixture();
    assert_eq!(
        preparation_diagnostic(&agent, &entry, operation_ms - 1000),
        None
    );

    let mut published = agent.clone();
    published.ready_at = Some(FAILED_AT.into());
    assert_eq!(
        preparation_diagnostic(&published, &entry, operation_ms),
        None
    );

    let mut newer = agent.clone();
    newer.conversation.last_seq += 1;
    assert_eq!(preparation_diagnostic(&newer, &entry, operation_ms), None);

    entry.agent_id = AgentId("agt_other".into());
    assert_eq!(preparation_diagnostic(&agent, &entry, operation_ms), None);
    entry.agent_id = agent.id.clone();
    let mut different_failure = agent.failure.clone().unwrap();
    different_failure.detail = "different diagnostic fixture".into();
    entry.kind = AgentLogKind::WorkStepFailed {
        step: AgentWorkStep::OpenConversation,
        failure: different_failure,
    };
    assert_eq!(preparation_diagnostic(&agent, &entry, operation_ms), None);
    entry.kind = AgentLogKind::WorkStepFailed {
        step: AgentWorkStep::RemoveHome,
        failure: agent.failure.clone().unwrap(),
    };
    assert_eq!(preparation_diagnostic(&agent, &entry, operation_ms), None);
}
