use super::*;
use crate::resource_access::*;
use crate::sandbox::agent_capacity_recovery::preflight::CapacityRecoveryPreflightService;
use crate::sandbox::store::PageRequest;
use crate::{AgentId, Authorization, Caller, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

struct Observations {
    first: ResourceAccessSnapshot,
    latest: ResourceAccessSnapshot,
    reads: AtomicUsize,
}

#[async_trait::async_trait]
impl ResourceAccessStore for Observations {
    async fn organization_sandboxes(
        &self,
        _: &SubjectId,
        _: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn shared_sandboxes(
        &self,
        _: &SubjectId,
        _: &[SubjectId],
        _: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn organization_agents(
        &self,
        _: &SubjectId,
        _: &crate::sandbox::AgentListQuery,
        _: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        Ok(if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first.clone()
        } else {
            self.latest.clone()
        })
    }
}

#[async_trait::async_trait]
impl ResourceShareStore for Observations {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        unreachable!()
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("dry-run must not mutate")
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        unreachable!()
    }
}

struct NoOrganization;
#[async_trait::async_trait]
impl OrganizationAuthority for NoOrganization {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        unreachable!("personal owner")
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        unreachable!()
    }
}

fn snapshot(f: &Fixture) -> ResourceAccessSnapshot {
    ResourceAccessSnapshot {
        lineage: ResourceLineage {
            agent: Some(f.agent.clone()),
            sandbox: f.sandbox.clone(),
            workspace: Some(f.workspace.clone()),
            worktree: Some(f.worktree.clone()),
        },
        policy: None,
        shares: vec![],
    }
}

fn service(
    first: &Fixture,
    latest: &Fixture,
) -> (CapacityRecoveryPreflightService, Arc<Observations>, Caller) {
    let store = Arc::new(Observations {
        first: snapshot(first),
        latest: snapshot(latest),
        reads: AtomicUsize::new(0),
    });
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(NoOrganization),
    ));
    let caller = Caller::new(
        first.agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    (CapacityRecoveryPreflightService::new(access), store, caller)
}

#[tokio::test]
async fn dry_run_reads_current_parent_intent_without_exposing_native_or_input_facts() {
    let first = Fixture::new();
    for (status, expected) in [
        (
            SandboxStatus::Failed,
            CapacityRecoveryDisposition::ParentRecoveryRequired,
        ),
        (
            SandboxStatus::Retiring,
            CapacityRecoveryDisposition::CleanupOnly,
        ),
        (
            SandboxStatus::CredentialRotating,
            CapacityRecoveryDisposition::ParentNotReady,
        ),
    ] {
        let mut latest = first.clone();
        latest.sandbox.status = status;
        let (svc, store, caller) = service(&first, &latest);
        let result = svc.read(&caller, &first.agent.id).await.unwrap();
        assert_eq!(result.disposition, expected);
        assert_eq!(store.reads.load(Ordering::SeqCst), 2);
        let public = serde_json::to_string(&result).unwrap();
        assert!(!public.contains("original-native"));
        assert!(!public.contains("original-home"));
        assert!(!public.contains("creation_fingerprint"));
        assert_eq!(store.latest, snapshot(&latest));
    }
}

#[tokio::test]
async fn dry_run_ready_parent_is_only_an_evidence_candidate_without_admission() {
    let f = Fixture::new();
    let (svc, store, caller) = service(&f, &f);
    let result = svc.read(&caller, &f.agent.id).await.unwrap();
    assert_eq!(
        result.disposition,
        CapacityRecoveryDisposition::EvidenceRequired
    );
    assert_eq!(
        store.latest.lineage.agent.as_ref().unwrap().status,
        AgentStatus::Failed
    );
    assert_eq!(store.latest, snapshot(&f));
}

#[tokio::test]
async fn dry_run_rejects_foreign_callers_and_ownership_change_before_release() {
    let f = Fixture::new();
    let (svc, _, mut caller) = service(&f, &f);
    caller.subject = SubjectId::new("another-owner");
    assert!(svc.read(&caller, &f.agent.id).await.is_err());
    let mut latest = f.clone();
    let owner = SubjectId::new("new-owner");
    latest.agent.owner = owner.clone();
    latest.sandbox.owner = owner.clone();
    latest.workspace.owner = owner.clone();
    latest.worktree.owner = owner;
    let (svc, _, caller) = service(&f, &latest);
    assert!(matches!(
        svc.read(&caller, &f.agent.id).await,
        Err(ResourceAccessError::Store(
            ResourceAccessStoreError::Conflict
        ))
    ));
}

#[tokio::test]
async fn dry_run_rejects_wrong_target_and_missing_ancestry() {
    let f = Fixture::new();
    let (svc, _, caller) = service(&f, &f);
    assert!(svc
        .read(&caller, &AgentId("foreign-target".into()))
        .await
        .is_err());
    let mut latest = f.clone();
    latest.worktree.agent_id = None;
    let (svc, _, caller) = service(&f, &latest);
    assert!(svc.read(&caller, &f.agent.id).await.is_err());
}
