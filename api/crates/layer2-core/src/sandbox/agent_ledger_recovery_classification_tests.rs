use super::*;
use crate::sandbox::*;
use crate::{Authorization, SubjectId};

use super::super::lifecycle_test_support;

fn owner() -> Caller {
    Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn fixture(parent: SandboxStatus) -> (Sandbox, Agent) {
    let mut sandbox = lifecycle_test_support::sandbox();
    sandbox.status = parent;
    let mut agent = lifecycle_test_support::agent("fixture-ledger-failed", "unused", 1);
    agent.status = AgentStatus::Failed;
    agent.failure = Some(TypedFailure {
        class: "agent_runtime_rejected".into(),
        detail: "mutation_journal_corrupt: runtime mutation journal contains too many entries"
            .into(),
    });
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent.ready_at = None;
    (sandbox, agent)
}

#[test]
fn retiring_parent_requires_migration_and_preserves_every_source_byte() {
    for step in [
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::None,
    ] {
        let (sandbox, mut agent) = fixture(SandboxStatus::Retiring);
        agent.work.step = step;
        agent.work.attempts = 4;
        let before = serde_json::to_vec(&(&sandbox, &agent)).unwrap();
        let result = assess_ledger_recovery(&owner(), &sandbox, &agent).unwrap();
        assert_eq!(result.route, LedgerRecoveryRouteV1::OwnerSelectedMigration);
        assert_eq!(
            result.history,
            LedgerRecoveryHistoryV1::NoRecordedConversation
        );
        assert_eq!(
            result.pending_destructive_steps,
            u64::from(step != AgentWorkStep::None)
        );
        assert_eq!(result.recorded_work_attempts, 4);
        assert!(result.native_inspection_required);
        assert!(result.original_hold_required);
        assert_eq!(serde_json::to_vec(&(&sandbox, &agent)).unwrap(), before);
    }
}

#[test]
fn active_paused_failed_and_retired_parents_have_distinct_routes() {
    for (parent, expected) in [
        (
            SandboxStatus::Active,
            LedgerRecoveryRouteV1::SameAgentQualification,
        ),
        (
            SandboxStatus::Paused,
            LedgerRecoveryRouteV1::ParentRecoveryRequired,
        ),
        (
            SandboxStatus::Failed,
            LedgerRecoveryRouteV1::ParentRecoveryRequired,
        ),
        (
            SandboxStatus::Retired,
            LedgerRecoveryRouteV1::PreserveUnknown,
        ),
        (
            SandboxStatus::Reviving,
            LedgerRecoveryRouteV1::PreserveUnknown,
        ),
    ] {
        let (sandbox, agent) = fixture(parent);
        let result = assess_ledger_recovery(&owner(), &sandbox, &agent).unwrap();
        assert_eq!(result.route, expected);
        assert!(result.original_hold_required);
        assert!(result.native_inspection_required);
    }
}

#[test]
fn missing_scope_or_foreign_owner_is_concealed_before_source_validation() {
    let (mut sandbox, agent) = fixture(SandboxStatus::Retiring);
    sandbox.revision = 0;
    for caller in [
        Caller::new(SubjectId("owner".into()), Authorization::none()),
        Caller::new(
            SubjectId("foreign".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        Caller::new(
            SubjectId("foreign".into()),
            Authorization::new([Capability::ReadAllSandboxes]),
        ),
    ] {
        assert_eq!(
            assess_ledger_recovery(&caller, &sandbox, &agent),
            Err(LedgerRecoveryAssessmentRefusal::NotFound)
        );
    }
    assert_eq!(
        assess_ledger_recovery(&owner(), &sandbox, &agent),
        Err(LedgerRecoveryAssessmentRefusal::InvalidSource)
    );
}

#[test]
fn administrator_still_cannot_assess_mixed_owner_or_parent_lineage() {
    let caller = Caller::new(
        SubjectId("administrator".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    let (sandbox, original) = fixture(SandboxStatus::Retiring);
    assert!(assess_ledger_recovery(&caller, &sandbox, &original).is_ok());
    for case in 0..4 {
        let mut agent = original.clone();
        match case {
            0 => agent.owner = SubjectId("another-owner".into()),
            1 => agent.sandbox_id.0 = "another-parent".into(),
            2 => agent.revision = 0,
            _ => agent.work.next_fence = 0,
        }
        assert_eq!(
            assess_ledger_recovery(&caller, &sandbox, &agent),
            Err(LedgerRecoveryAssessmentRefusal::InvalidSource)
        );
    }
}

#[test]
fn live_and_terminal_agents_are_preserved_even_with_a_retained_failure() {
    let (sandbox, mut agent) = fixture(SandboxStatus::Retiring);
    for status in [
        AgentStatus::Starting,
        AgentStatus::Paused,
        AgentStatus::Deleted,
        AgentStatus::Stopped,
    ] {
        agent.status = status;
        if status == AgentStatus::Paused {
            agent.conversation.conversation_id = Some("retained-native".into());
            agent.conversation.runtime_generation = Some(1);
        }
        let result = assess_ledger_recovery(&owner(), &sandbox, &agent).unwrap();
        assert_eq!(
            result.route,
            if status.is_live() {
                LedgerRecoveryRouteV1::PreserveLive
            } else {
                LedgerRecoveryRouteV1::PreserveTerminal
            }
        );
    }
}

#[test]
fn only_exact_legacy_or_typed_ledger_failures_select_recovery() {
    let (sandbox, original) = fixture(SandboxStatus::Retiring);
    for detail in [
        "mutation_journal_corrupt: runtime mutation journal contains too many entries",
        "mutation_authority_corrupt: runtime mutation authority contains too many records",
        "mutation_authority_corrupt: mutation authority contains too many records",
        "mutation_ledger_capacity: runtime mutation ledger capacity is exhausted",
    ] {
        let mut agent = original.clone();
        agent.failure.as_mut().unwrap().detail = detail.into();
        assert_eq!(
            assess_ledger_recovery(&owner(), &sandbox, &agent)
                .unwrap()
                .route,
            LedgerRecoveryRouteV1::OwnerSelectedMigration
        );
        agent.failure.as_mut().unwrap().detail.push(' ');
        assert_eq!(
            assess_ledger_recovery(&owner(), &sandbox, &agent)
                .unwrap()
                .route,
            LedgerRecoveryRouteV1::PreserveUnknown
        );
    }
    let mut agent = original;
    agent.failure = Some(TypedFailure {
        class: store::AGENT_RUNTIME_LEDGER_HOLD.into(),
        detail: store::AGENT_RUNTIME_LEDGER_HOLD_DETAIL.into(),
    });
    assert_eq!(
        assess_ledger_recovery(&owner(), &sandbox, &agent)
            .unwrap()
            .route,
        LedgerRecoveryRouteV1::OwnerSelectedMigration
    );
    agent.failure = None;
    assert_eq!(
        assess_ledger_recovery(&owner(), &sandbox, &agent)
            .unwrap()
            .route,
        LedgerRecoveryRouteV1::PreserveUnknown
    );
}

#[test]
fn conversations_and_prior_attempts_remain_visible_without_replaying_input() {
    let (mut sandbox, mut agent) = fixture(SandboxStatus::Retiring);
    let caller = Caller::new(
        SubjectId("fixture-private-principal".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    sandbox.owner = caller.subject.clone();
    agent.owner = caller.subject.clone();
    agent.conversation.conversation_id = Some("native-history".into());
    agent.conversation.runtime_generation = Some(1);
    let result = assess_ledger_recovery(&caller, &sandbox, &agent).unwrap();
    assert_eq!(
        result.history,
        LedgerRecoveryHistoryV1::RecordedConversation
    );
    agent.conversation.first_message = Some(
        FirstMessageRecord::new(
            "fixture-first",
            "preserved content",
            lifecycle_test_support::NOW,
        )
        .unwrap(),
    );
    let first = agent.conversation.first_message.as_mut().unwrap();
    first
        .fence_dispatch_for(1, 1, 1, lifecycle_test_support::NOW.into())
        .unwrap();
    let before = serde_json::to_vec(&agent).unwrap();
    let result = assess_ledger_recovery(&caller, &sandbox, &agent).unwrap();
    assert_eq!(
        result.history,
        LedgerRecoveryHistoryV1::AttemptedOrUncertainInput
    );
    assert_eq!(result.retained_queued_inputs, 1);
    assert_eq!(serde_json::to_vec(&agent).unwrap(), before);
    let emitted = serde_json::to_string(&result).unwrap();
    for private in [
        agent.id.as_str(),
        sandbox.id.as_str(),
        agent.owner.as_str(),
        "native-history",
        "preserved content",
    ] {
        assert!(!emitted.contains(private));
    }
}
