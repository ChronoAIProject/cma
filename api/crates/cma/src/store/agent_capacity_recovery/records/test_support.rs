//! Synthetic codec/receipt fixtures; these do not grant admission authority.
use layer2_core::sandbox::agent_capacity_recovery::state::{
    RecoveryPhase, RecoveryRequest, RecoveryTarget, RecoveryWork,
};
use layer2_core::sandbox::{Agent, AgentWorkReceipt, AgentWorkStep, RuntimeBinding};
use layer2_core::AgentId;
pub(in crate::store) const NOW: &str = "2026-11-11T00:00:00Z";

pub(in crate::store) fn work() -> RecoveryWork {
    let request = RecoveryRequest {
        agent_id: AgentId("carrier-fixture".into()),
        creation_key: "original-creation".into(),
        creation_fingerprint: "a".repeat(64),
        failure_seq: 10_001,
        requested_at: NOW.into(),
    };
    RecoveryWork {
        format_version: 1,
        target: RecoveryTarget {
            agent_id: request.agent_id.clone(),
            creation_key: request.creation_key.clone(),
            creation_fingerprint: request.creation_fingerprint.clone(),
            conversation_id: "retained-native".into(),
            conversation_generation: 1,
            runtime: RuntimeBinding {
                boot_id: Some("b".repeat(32)),
                runtime_id: "original-runtime".into(),
                runtime_version: "test".into(),
                provision_generation: Some(3),
                bound_at: NOW.into(),
            },
            home_identity: "original-home".into(),
            original_open: AgentWorkReceipt {
                step: AgentWorkStep::OpenConversation,
                operation_id: format!("agent:{}:open:v1", request.agent_id),
                request_sha256: "c".repeat(64),
                completed_at: NOW.into(),
            },
            active_turn: None,
            input_sha256: "d".repeat(64),
        },
        request,
        phase: RecoveryPhase::Admitted,
        revision: 1,
        attempts: 0,
        next_fence: 8,
        lease: None,
        ready_at: NOW.into(),
        completed_at: None,
    }
}

pub(in crate::store) fn agent() -> Agent {
    // Metadata-only fixture; source capture and native handshake are not stubbed.
    serde_json::from_value(serde_json::json!({
        "id": "carrier-fixture", "worktree_id": "worktree", "workspace_id": "workspace",
        "sandbox_id": "sandbox", "owner": "owner", "creation_key": "original-creation",
        "creation_fingerprint": "a".repeat(64),
        "profile": {"id": "profile", "revision": 1, "digest": "a".repeat(64),
            "content": {"isolated_agent": false, "model": "fixture", "thinking_effort": "high",
                "permission_level": "guarded", "skills": [], "system_prompt": ""}},
        "status": "failed", "conversation": {
            "preferences": {}, "managed_guidance_revision": 0,
            "observations": layer2_core::sandbox::AgentObservations::default(),
            "settings": null, "conversation_id": "retained-native", "runtime_generation": 1,
            "last_seq": 10001, "first_message": null, "active_turn": null,
            "operation": null, "queued_messages": []},
        "work": layer2_core::sandbox::AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        "created_at": NOW, "updated_at": NOW, "origin": {"type": "manual"}, "revision": 10
    }))
    .unwrap()
}

pub(in crate::store) fn stored() -> super::StoredRecovery {
    super::StoredRecovery::new(&agent(), work(), &[], NOW).unwrap()
}
