use super::lifecycle_test_support::{agent, checkpoint, sandbox};
use super::*;

#[test]
fn image_drift_selects_only_active_bound_sandboxes() {
    let mut sandbox = sandbox();
    assert!(!image_rotation_required(&sandbox, "image-v1"));
    assert!(image_rotation_required(&sandbox, "image-v2"));
    assert!(!image_rotation_required(&sandbox, ""));

    sandbox.status = SandboxStatus::Rotating;
    assert!(!image_rotation_required(&sandbox, "image-v2"));
    sandbox.status = SandboxStatus::Active;
    sandbox.runtime = None;
    assert!(!image_rotation_required(&sandbox, "image-v2"));
}

#[test]
fn rotation_identity_keeps_sandbox_profile_and_every_live_agent_id() {
    let before_sandbox = sandbox();
    let before_agents = vec![
        agent("agt_b", "conversation-b", 8),
        agent("agt_a", "conversation-a", 4),
    ];
    let identity = SandboxRotationIdentity::capture(&before_sandbox, &before_agents).unwrap();
    assert_eq!(identity.agents[0].agent_id.as_str(), "agt_a");
    assert_eq!(identity.agents[1].agent_id.as_str(), "agt_b");

    let mut replacement = before_sandbox.clone();
    replacement.runtime.as_mut().unwrap().runtime_id = "runtime-new".into();
    replacement.runtime.as_mut().unwrap().runtime_version = "image-v2".into();
    let mut reopened = before_agents.clone();
    for agent in &mut reopened {
        agent.conversation.runtime_generation = Some(5);
        agent.status = AgentStatus::Paused;
    }
    assert!(identity.preserved_by(&replacement, &reopened));

    let mut wrong_conversation = reopened.clone();
    wrong_conversation[0].conversation.conversation_id = Some("conversation-other".into());
    assert!(!identity.preserved_by(&replacement, &wrong_conversation));

    let mut wrong_worktree = reopened.clone();
    wrong_worktree[0].worktree_id.0.push_str("-other");
    assert!(!identity.preserved_by(&replacement, &wrong_worktree));

    let mut wrong_profile = replacement;
    wrong_profile.profile.revision += 1;
    assert!(!identity.preserved_by(&wrong_profile, &reopened));
    assert!(!identity.preserved_by(&before_sandbox, &reopened[..1]));
}

#[test]
fn rotation_identity_refuses_partial_or_cross_sandbox_agent_sets() {
    let sandbox = sandbox();
    let mut partial = agent("agt_a", "conversation-a", 4);
    partial.conversation.conversation_id = None;
    assert!(SandboxRotationIdentity::capture(&sandbox, &[partial]).is_err());

    let mut foreign = agent("agt_a", "conversation-a", 4);
    foreign.sandbox_id.0 = "sbx_other".into();
    assert!(SandboxRotationIdentity::capture(&sandbox, &[foreign]).is_err());
    assert!(SandboxRotationIdentity::capture(&sandbox, &[]).is_err());
}

#[test]
fn rotation_accounting_is_checkpoint_idempotent_and_monotonic() {
    let first = checkpoint(1, "2026-09-05T00:05:00Z");
    let second = checkpoint(2, "2026-09-05T00:10:00Z");
    let mut accounting = SandboxRotationAccounting::default();

    assert_eq!(
        accounting.account(&first).unwrap(),
        SandboxRotationAccountingOutcome::Accounted { rotations: 1 }
    );
    assert_eq!(
        accounting.account(&first).unwrap(),
        SandboxRotationAccountingOutcome::AlreadyAccounted
    );
    assert_eq!(
        accounting.account(&second).unwrap(),
        SandboxRotationAccountingOutcome::Accounted { rotations: 2 }
    );
    assert_eq!(accounting.rotations, 2);
    assert_eq!(
        accounting.rotated_at.as_deref(),
        Some("2026-09-05T00:10:00Z")
    );

    let older = checkpoint(3, "2026-09-05T00:09:59Z");
    assert!(accounting.account(&older).is_err());
    assert_eq!(accounting.rotations, 2);
}

#[test]
fn rotation_accounting_rejects_corrupt_state_and_overflow() {
    let checkpoint = checkpoint(2, "2026-09-05T00:10:00Z");
    let mut inconsistent = SandboxRotationAccounting {
        rotations: 1,
        rotated_at: None,
    };
    assert!(inconsistent.account(&checkpoint).is_err());

    let mut exhausted = SandboxRotationAccounting {
        rotations: u32::MAX,
        rotated_at: Some("2026-09-05T00:05:00Z".into()),
    };
    assert!(exhausted.account(&checkpoint).is_err());
}
