use super::*;
use crate::{
    bridge::{BridgeAgentTarget, BridgeId},
    ids::{AgentId, SandboxId, WorkspaceId, WorktreeId},
    resource_access::{ResourceActor, ResourceOwner},
    SubjectId,
};

fn job() -> CleanupJob {
    let allocation_id = format!("bral_{}", "1".repeat(64));
    let actor = ResourceActor::Human {
        subject: SubjectId::new("cleanup-fixture"),
    };
    let mut admission = CleanupJobAdmission {
        schema_version: 1,
        receipt: ChildCleanupReceipt {
            id: format!("bclj_{}", "2".repeat(64)),
            bridge_id: BridgeId("brg_cleanup".into()),
            allocation_id: allocation_id.clone(),
            generation: 1,
            target: BridgeAgentTarget {
                sandbox_id: SandboxId("sbx_cleanup".into()),
                workspace_id: WorkspaceId("wks_cleanup".into()),
                worktree_id: WorktreeId("wkt_cleanup".into()),
                agent_id: AgentId("agt_cleanup".into()),
            },
            request: ChildCleanupRequest {
                expected_bridge_revision: 3,
                expected_allocation_revision: 4,
                action: ChildCleanupAction::RemoveChild,
            },
            requested_by: actor,
            accepted_at_ms: 1000,
        },
        owner: ResourceOwner::Person {
            subject: SubjectId::new("cleanup-fixture"),
        },
        issuer: "https://identity.test".into(),
        allocation_sha256: "3".repeat(64),
        request_sha256: String::new(),
        cleanup_request_id: format!("{allocation_id}:reuse-cleanup-request:v1"),
        cleanup_request_sha256: "4".repeat(64),
        digest: String::new(),
    };
    admission.request_sha256 = digest(&admission.receipt.request).unwrap();
    admission.digest = admission.compute_digest().unwrap();
    CleanupJob::new(admission, vec![ChildCleanupHold::PendingReply], 500).unwrap()
}

#[test]
fn newer_worker_claim_rejects_expired_publication_without_rewriting_acceptance() {
    let mut state = job();
    let admission = state.admission.clone();
    state.claim("old-worker", 1000, 2000).unwrap();
    let old = state.lease.clone().unwrap();
    assert!(matches!(
        state.claim("overlap", 1999, 3000),
        Err(CleanupError::Conflict)
    ));
    state.claim("new-worker", 2000, 3000).unwrap();
    let before = state.clone();
    assert!(matches!(
        state.observe(&old, ChildCleanupProgress::Complete, vec![], 2001),
        Err(CleanupError::Conflict)
    ));
    assert_eq!(state, before);
    let current = state.lease.clone().unwrap();
    state
        .observe(
            &current,
            ChildCleanupProgress::AdmissionClosed,
            vec![ChildCleanupHold::UncertainReply],
            2001,
        )
        .unwrap();
    assert_eq!(state.admission, admission);
    assert!(!state.status().unwrap().agent_capacity_released);
    assert!(matches!(
        state.claim("early", 7000, 8000),
        Err(CleanupError::Conflict)
    ));
    state.claim("eligible", 7001, 8000).unwrap();
}

#[test]
fn proved_lost_progress_retains_nonterminal_claim_and_completes_without_new_effect() {
    let mut state = job();
    let acceptance = state.admission.clone();
    state.claim("recovery", 1000, 2000).unwrap();
    let claim = state.lease.clone().unwrap();
    state
        .reconcile_progress(&claim, ChildCleanupProgress::HomeRemoved, 1001)
        .unwrap();
    assert_eq!(state.lease.as_ref(), Some(&claim));
    assert_eq!(
        state.observed_at_ms, 500,
        "proof catch-up cannot refresh historical holds"
    );
    let before = state.clone();
    assert!(matches!(
        state.reconcile_progress(&claim, ChildCleanupProgress::ProcessClosed, 1002),
        Err(CleanupError::Corrupt)
    ));
    assert_eq!(state, before);
    state
        .reconcile_progress(&claim, ChildCleanupProgress::Complete, 1003)
        .unwrap();
    assert_eq!(state.admission, acceptance);
    assert!(state.lease.is_none());
    let status = state.status().unwrap();
    assert!(
        status.holds.is_empty()
            && status.agent_capacity_released
            && status.workspace_retained
            && status.history_retained
    );
    assert!(state.claim("later", 2001, 3000).is_err());
}

#[test]
fn expired_claim_and_inconsistent_acceptance_never_publish_progress() {
    let mut state = job();
    assert_eq!(
        state.observed_at_ms, 500,
        "adopted Stage A evidence retains its original time"
    );
    state.claim("worker", 1000, 2000).unwrap();
    let before = state.clone();
    let claim = state.lease.clone().unwrap();
    assert!(state
        .reconcile_progress(&claim, ChildCleanupProgress::Complete, 2000)
        .is_err());
    assert_eq!(state, before);
    state.admission.receipt.requested_by = ResourceActor::Human {
        subject: SubjectId::new("different-actor"),
    };
    assert!(matches!(state.status(), Err(CleanupError::Corrupt)));
}
