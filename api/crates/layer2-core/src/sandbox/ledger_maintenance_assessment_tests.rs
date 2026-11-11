use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::sandbox::ledger_maintenance_admission::{
    plan_ledger_maintenance_admission, test_support::fixture, LedgerMaintenanceAdmissionPolicy,
    LedgerMaintenanceAdmissionRequest, LedgerMaintenanceAdmissionV1, LedgerMaintenanceLeaseV1,
    LedgerMaintenanceStateV1,
};
use crate::sandbox::ledger_maintenance_plan::LedgerMaintenancePlanArtifactV1;
use crate::sandbox::store::ledger_maintenance::{
    LedgerMaintenanceAdmission, LedgerMaintenanceInspection, LedgerMaintenanceStore,
    LedgerMaintenanceTakeover,
};
use crate::{Authorization, SubjectId};

// Full mutation-capable port is deliberately supplied to exercise its read-only
// adapter; either write method fails the test immediately if assessment calls it.
struct Store {
    inspection: LedgerMaintenanceInspection,
    reads: AtomicUsize,
    error: Option<LedgerMaintenanceStoreError>,
}

#[async_trait::async_trait]
impl LedgerMaintenanceStore for Store {
    async fn inspect_ledger_maintenance(
        &self,
        caller: &Caller,
        sandbox: &SandboxId,
    ) -> Result<LedgerMaintenanceInspection, LedgerMaintenanceStoreError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        assert_eq!(sandbox, &self.inspection.source.sandbox.id);
        assert!(!caller.subject.as_str().is_empty());
        self.error.map_or_else(|| Ok(self.inspection.clone()), Err)
    }

    async fn admit_ledger_maintenance(
        &self,
        _: LedgerMaintenanceAdmissionPolicy,
        _: LedgerMaintenanceAdmission,
    ) -> Result<LedgerMaintenanceStateV1, LedgerMaintenanceStoreError> {
        panic!("assessment attempted maintenance admission")
    }

    async fn take_over_ledger_maintenance(
        &self,
        _: LedgerMaintenanceAdmissionPolicy,
        _: LedgerMaintenanceTakeover,
    ) -> Result<LedgerMaintenanceStateV1, LedgerMaintenanceStoreError> {
        panic!("assessment attempted lease takeover")
    }
}

fn owner() -> Caller {
    Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn store() -> Store {
    Store {
        inspection: LedgerMaintenanceInspection {
            source: fixture().source,
            state: None,
        },
        reads: AtomicUsize::new(0),
        error: None,
    }
}
async fn assess(
    store: &Store,
    caller: &Caller,
) -> Result<LedgerMaintenanceAssessmentV1, LedgerMaintenanceAssessmentError> {
    assess_ledger_maintenance(
        store,
        LedgerMaintenanceAssessmentPolicy { enabled: true },
        caller,
        &store.inspection.source.sandbox.id,
    )
    .await
}
fn source_bytes(store: &Store) -> Vec<u8> {
    serde_json::to_vec(&(&store.inspection.source, &store.inspection.state)).unwrap()
}
fn add_hold(store: &mut Store) {
    let mut plan = fixture();
    plan.source = store.inspection.source.clone();
    let sandbox = &plan.source.sandbox;
    let runtime = sandbox.retained_runtime_binding().unwrap();
    plan.binding.sandbox_id = sandbox.id.to_string();
    plan.binding.sandbox_revision = sandbox.revision;
    plan.binding.generation = runtime.provision_generation.unwrap();
    plan.binding.boot_id = runtime.boot_id.clone().unwrap();
    plan.binding.home_identity = sandbox
        .lifecycle
        .home
        .as_ref()
        .unwrap()
        .attachment()
        .unwrap()
        .identity()
        .unwrap();
    let accepted =
        LedgerMaintenancePlanArtifactV1::parse(&serde_json::to_vec(&plan).unwrap()).unwrap();
    let result = plan_ledger_maintenance_admission(
        LedgerMaintenanceAdmissionPolicy { enabled: true },
        &plan.source,
        None,
        LedgerMaintenanceAdmissionRequest {
            caller: &owner(),
            accepted: &accepted,
            lease: LedgerMaintenanceLeaseV1 {
                holder: "private-fixture-worker".into(),
                expires_at: "2026-09-05T00:31:02Z".into(),
            },
            now: "2026-09-05T00:30:02Z",
        },
    )
    .unwrap();
    let LedgerMaintenanceAdmissionV1::Persist(state) = result else {
        panic!("expected admission")
    };
    store.inspection.state = Some(*state);
}

#[tokio::test]
async fn default_off_and_missing_capabilities_do_not_even_read() {
    let store = store();
    assert_eq!(
        assess_ledger_maintenance(
            &store,
            LedgerMaintenanceAssessmentPolicy::default(),
            &owner(),
            &store.inspection.source.sandbox.id
        )
        .await,
        Err(LedgerMaintenanceAssessmentError::Disabled)
    );
    for capabilities in [
        Authorization::none(),
        Authorization::new([Capability::ReadAllSandboxes]),
    ] {
        let caller = Caller::new(owner().subject, capabilities);
        assert_eq!(
            assess(&store, &caller).await,
            Err(LedgerMaintenanceAssessmentError::NotFound)
        );
    }
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn assessment_reads_once_preserves_bytes_and_exposes_no_private_values() {
    let mut store = store();
    add_hold(&mut store);
    let before = source_bytes(&store);
    let result = assess(&store, &owner()).await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    assert_eq!(result.agents, 1);
    assert_eq!(result.workspaces, 1);
    assert_eq!(result.worktrees, 1);
    assert_eq!(result.recorded_command_lanes, 1);
    assert_eq!(result.leased_command_lanes, 0);
    assert_eq!(result.recovery.preserve_live, 1);
    assert_eq!(result.recovery.recorded_conversation, 1);
    assert_eq!(result.source_matches_accepted, Some(true));
    // The old lease has expired; neither readiness nor a renewal is inferred.
    assert_eq!(
        result.maintenance_phase,
        Some(LedgerMaintenancePhaseV1::Requested)
    );
    assert!(
        result.native_effects_unknown
            && result.physical_exclusion_unknown
            && result.backup_verification_required
    );
    assert!(!result.recovery_authorized);
    assert_eq!(before, source_bytes(&store));
    let source = &store.inspection.source;
    let output = format!("{} {result:?}", serde_json::to_string(&result).unwrap());
    for private in [
        source.sandbox.id.as_str(),
        source.agents[0].id.as_str(),
        source.workspaces[0].path.as_str(),
        "retained-conversation",
        "private-fixture-worker",
        "fixture-operation",
    ] {
        assert!(!output.contains(private), "private field leaked");
    }
}

#[tokio::test]
async fn absence_and_source_drift_never_report_authorized_recovery() {
    let mut store = store();
    let result = assess(&store, &owner()).await.unwrap();
    assert_eq!(result.maintenance_phase, None);
    assert_eq!(result.source_matches_accepted, None);
    assert!(!result.recovery_authorized);
    add_hold(&mut store);
    store.inspection.source.sandbox_event_head += 1;
    let result = assess(&store, &owner()).await.unwrap();
    assert_eq!(result.source_matches_accepted, Some(false));
    assert_eq!(
        result.maintenance_phase,
        Some(LedgerMaintenancePhaseV1::Requested)
    );
    assert!(!result.recovery_authorized);
}

#[tokio::test]
async fn failed_work_counts_keep_cleanup_and_attempted_input_visible_without_replay() {
    use crate::sandbox::{AgentStatus, AgentWorkStep, FirstMessageRecord, TypedFailure};
    let mut store = store();
    let source = &mut store.inspection.source;
    let agent = &mut source.agents[0];
    agent.status = AgentStatus::Failed;
    agent.failure = Some(TypedFailure {
        class: "agent_runtime_rejected".into(),
        detail: "mutation_journal_corrupt: runtime mutation journal contains too many entries"
            .into(),
    });
    agent.ready_at = None;
    agent.conversation.conversation_id = None;
    agent.conversation.runtime_generation = None;
    agent.work.step = AgentWorkStep::RemoveHome;
    agent.work.attempts = 3;
    source.workspaces[0].work.step = WorkspaceWorkStep::Remove;
    agent.conversation.first_message = Some(
        FirstMessageRecord::new(
            "fixture-pending-input",
            "private fixture input",
            "2026-09-05T00:30:02Z",
        )
        .unwrap(),
    );
    agent
        .conversation
        .first_message
        .as_mut()
        .unwrap()
        .fence_dispatch_for(1, 1, 1, "2026-09-05T00:30:02Z".into())
        .unwrap();
    let before = source_bytes(&store);
    let result = assess(&store, &owner()).await.unwrap();
    assert_eq!(result.recovery.same_agent_qualification, 1);
    assert_eq!(result.recovery.attempted_or_uncertain_input, 1);
    assert_eq!(result.recovery.no_recorded_conversation, 0);
    assert_eq!(result.recovery.pending_destructive_agent_steps, 1);
    assert_eq!(result.recovery.retained_queued_inputs, 1);
    assert_eq!(result.recovery.recorded_work_attempts, 3);
    assert_eq!(result.pending_workspace_steps, 1);
    assert_eq!(result.pending_workspace_removals, 1);
    assert!(!result.recovery_authorized);
    assert_eq!(source_bytes(&store), before);
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("private fixture input"));
}

#[tokio::test]
async fn foreign_owner_is_concealed_even_if_reader_returns_malformed_private_data() {
    let mut store = store();
    store.inspection.source.source_documents_sha256.clear();
    let caller = Caller::new(
        SubjectId("foreign-fixture-owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let before = source_bytes(&store);
    assert_eq!(
        assess(&store, &caller).await,
        Err(LedgerMaintenanceAssessmentError::NotFound)
    );
    assert_eq!(before, source_bytes(&store));
}

#[tokio::test]
async fn administrator_can_inspect_but_cannot_accept_invalid_inventory_or_state() {
    let caller = Caller::new(
        SubjectId("fixture-admin".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    assert!(assess(&store(), &caller).await.is_ok());
    for case in 0..6 {
        let mut store = store();
        add_hold(&mut store);
        match case {
            0 => store.inspection.source.source_documents_sha256.clear(),
            1 => store.inspection.source.command_lanes.clear(),
            2 => {
                store.inspection.source.agents[0].owner = SubjectId("foreign-fixture-owner".into())
            }
            3 => store
                .inspection
                .state
                .as_mut()
                .unwrap()
                .accepted_plan_bytes
                .clear(),
            4 => {
                store.inspection.source.command_lanes[0]
                    .lane
                    .as_mut()
                    .unwrap()
                    .next_fence = 0
            }
            _ => store.inspection.source.worktrees.clear(),
        }
        let before = source_bytes(&store);
        assert_eq!(
            assess(&store, &caller).await,
            Err(LedgerMaintenanceAssessmentError::InvalidSource)
        );
        assert_eq!(before, source_bytes(&store));
    }
}

#[tokio::test]
async fn store_failures_refuse_without_fallback_or_any_write() {
    for (error, expected) in [
        (
            LedgerMaintenanceStoreError::Unavailable,
            LedgerMaintenanceAssessmentError::Unavailable,
        ),
        (
            LedgerMaintenanceStoreError::PendingCommit,
            LedgerMaintenanceAssessmentError::Unavailable,
        ),
        (
            LedgerMaintenanceStoreError::Corrupt,
            LedgerMaintenanceAssessmentError::InvalidSource,
        ),
        (
            LedgerMaintenanceStoreError::Refused(LedgerMaintenanceAdmissionRefusal::NotFound),
            LedgerMaintenanceAssessmentError::NotFound,
        ),
    ] {
        let mut store = store();
        store.error = Some(error);
        let before = source_bytes(&store);
        assert_eq!(assess(&store, &owner()).await, Err(expected));
        assert_eq!(store.reads.load(Ordering::SeqCst), 1);
        assert_eq!(before, source_bytes(&store));
    }
}
