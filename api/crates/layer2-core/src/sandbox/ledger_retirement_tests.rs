use super::*;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

fn fixture() -> SandboxHierarchyRecord {
    let mut parent = super::super::lifecycle_test_support::sandbox();
    parent.runtime.as_mut().unwrap().provision_generation = Some(1);
    parent.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-home".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-pvc".into(),
        storage_generation: "fixture-pvc-generation".into(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    let workspace = workspace(&parent, "workspace", None);
    let mut worktree = worktree(&workspace, "worktree", "agent");
    worktree.status = WorktreeStatus::Removed;
    let mut agent = agent(&workspace, &worktree, "agent");
    agent.status = AgentStatus::Deleted;
    agent.deleted_at = Some(NOW.into());
    agent.work.step = AgentWorkStep::None;
    SandboxHierarchyRecord {
        sandbox: parent,
        workspaces: vec![workspace],
        worktrees: vec![worktree],
        agents: vec![agent],
    }
}

fn observation(
    value: &SandboxHierarchyRecord,
) -> layer3_adapter::runtime_ledger_retirement::RuntimeLedgerRetirementSnapshot {
    value
        .ledger_retirement_observation(NOW.into(), "a".repeat(64))
        .unwrap()
}

#[test]
fn ledger_retirement_observes_settled_deletion_without_changing_hierarchy() {
    let value = fixture();
    let before = value.clone();
    let result = observation(&value);
    assert!(result.contains_agent("agent"));
    assert!(result.contains_worktree("workspace", "wt-1"));
    assert_eq!(result.agents_observed, 1);
    assert_eq!(value, before);
}

#[test]
fn ledger_retirement_preserves_live_unknown_and_unsettled_resources() {
    for case in 0..8 {
        let mut value = fixture();
        match case {
            0 => value.agents[0].status = AgentStatus::Stopped,
            1 => value.agents[0].deleted_at = None,
            2 => value.agents[0].work.step = AgentWorkStep::RemoveHome,
            3 => {
                value.agents[0].work.lease = Some(AgentWorkLease {
                    holder: "fixture-worker".into(),
                    fence: 1,
                    expires_at: NOW.into(),
                })
            }
            4 => value.worktrees[0].status = WorktreeStatus::Ready,
            5 => value.worktrees[0].agent_id = None,
            6 => value.agents.clear(),
            _ => {
                let mut other = value.agents[0].clone();
                other.id = AgentId("live-shared-agent".into());
                other.status = AgentStatus::Ready;
                other.conversation.conversation_id = Some("fixture-conversation".into());
                other.conversation.runtime_generation = Some(1);
                other.deleted_at = None;
                value.agents.push(other);
            }
        }
        let result = observation(&value);
        assert!(result.deleted_agents.is_empty(), "case {case}");
        assert!(result.removed_worktrees.is_empty(), "case {case}");
    }
}

#[test]
fn ledger_retirement_rejects_aliases_foreign_lineage_and_missing_binding() {
    for case in 0..8 {
        let mut value = fixture();
        match case {
            0 => value.agents[0].owner = SubjectId::new("foreign"),
            1 => value.workspaces[0].sandbox_id = SandboxId("foreign".into()),
            2 => value.worktrees[0].workspace_id = WorkspaceId("missing".into()),
            3 => value.sandbox.lifecycle.home = None,
            4 => value.sandbox.runtime.as_mut().unwrap().provision_generation = None,
            5 => value.agents.push(value.agents[0].clone()),
            6 => {
                let mut alias = value.workspaces[0].clone();
                alias.id = WorkspaceId("alias".into());
                value.workspaces.push(alias);
            }
            _ => {
                let mut alias = value.worktrees[0].clone();
                alias.id = WorktreeId("alias".into());
                value.worktrees.push(alias);
            }
        }
        assert!(
            value
                .ledger_retirement_observation(NOW.into(), "a".repeat(64))
                .is_err(),
            "case {case}"
        );
    }
}

#[test]
fn ledger_retirement_keeps_debt_pending_skill_turn_queue_and_cooldown() {
    for case in 0..5 {
        let mut value = fixture();
        let agent = &mut value.agents[0];
        match case {
            0 => agent.work.cleanup_debt.push(AgentCleanupDebt {
                step: AgentWorkStep::RemoveHome,
                operation_id: "fixture-removal".into(),
                request_sha256: "a".repeat(64),
            }),
            1 => agent.work.live_skills.push(AgentSkillInstallation::new(
                crate::profiles::SkillRef {
                    source: crate::profiles::SkillSource::Ornn,
                    id: crate::SkillId::new("fixture-skill"),
                    version: Some("1.0".into()),
                },
            )),
            2 => agent.conversation.active_turn = Some(1),
            3 => agent
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(1, "fixture message".into(), NOW.into()).unwrap()),
            _ => agent.work.cleanup_not_before = Some(NOW.into()),
        }
        let result = observation(&value);
        assert!(result.deleted_agents.is_empty(), "case {case}");
        assert!(result.removed_worktrees.is_empty(), "case {case}");
    }
}

#[test]
fn ledger_retirement_accepts_settled_deletion_with_retained_warm_history() {
    let mut value = fixture();
    value.agents[0].work.warm_conversation_id = Some("fixture-warm-conversation".into());
    let result = observation(&value);
    assert!(result.contains_agent("agent"));
    assert!(result.contains_worktree("workspace", "wt-1"));
}
