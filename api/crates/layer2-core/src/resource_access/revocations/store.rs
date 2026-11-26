use super::*;
#[async_trait::async_trait]
impl ResourceExecutionStore for Store {
    async fn activation(&self) -> ExecutionResult<ActivationStateV1> {
        Ok(ActivationStateV1::Activated {
            epoch: generation(1),
        })
    }
    async fn revocation(
        &self,
        id: &wire::Digest,
    ) -> ExecutionResult<Option<RevocationOperationV1>> {
        self.before_read(Read::Operation).await;
        let s = self.0.lock().unwrap();
        Ok((id == &s.operation.key.operation_id).then(|| s.operation.clone()))
    }
    async fn frozen_members(
        &self,
        _: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<ExecutionRecordV1>> {
        self.before_read(Read::Members(after)).await;
        let mut s = self.0.lock().unwrap();
        s.pages.push(after);
        if s.change_owner {
            s.snapshot.lineage.sandbox.owner = crate::SubjectId::new("other");
        }
        if s.missing_page {
            return Ok(vec![]);
        }
        Ok(s.members
            .iter()
            .skip(after as usize)
            .take(limit.min(7) as usize)
            .cloned()
            .collect())
    }
    async fn effects_for(
        &self,
        registration: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<EffectRecordV1>> {
        self.before_read(Read::Effects(after)).await;
        Ok(self
            .0
            .lock()
            .unwrap()
            .effects
            .iter()
            .filter(|e| &e.permission.intent.registration == registration)
            .skip(after as usize)
            .take(limit as usize)
            .cloned()
            .collect())
    }
    async fn grant(&self, _origin: &wire::Digest) -> ExecutionResult<Option<GrantControlV1>> {
        panic!("status must not drive effects")
    }
    async fn register(
        &self,
        _authority: &ExecutionAuthority,
        _member: &wire::MemberKey,
    ) -> ExecutionResult<ExecutionRecordV1> {
        panic!("status must not drive effects")
    }
    async fn record(
        &self,
        _registration: &wire::Digest,
    ) -> ExecutionResult<Option<ExecutionRecordV1>> {
        panic!("status must not drive effects")
    }
    async fn issue_ticket(
        &self,
        _authority: &ExecutionAuthority,
        _ticket: &wire::ExecutionTicketV1,
    ) -> ExecutionResult<wire::ExecutionTicketV1> {
        panic!("status must not drive effects")
    }
    async fn continue_execution(
        &self,
        _authority: &ExecutionAuthority,
        _lease: &wire::ContinuationV1,
    ) -> ExecutionResult<wire::ContinuationV1> {
        panic!("status must not drive effects")
    }
    async fn issue_release(
        &self,
        _authority: &ExecutionAuthority,
        _permit: &wire::ReleasePermitV1,
    ) -> ExecutionResult<wire::ReleasePermitV1> {
        panic!("status must not drive effects")
    }
    async fn observe_start(&self, _observation: &wire::StartObservationV1) -> ExecutionResult<()> {
        panic!("status must not drive effects")
    }
    async fn begin_revocation(
        &self,
        _authority: &RevocationAuthority,
    ) -> ExecutionResult<RevocationOperationV1> {
        panic!("status must not drive effects")
    }
    async fn pending_revocations(
        &self,
        _after: Option<&wire::Digest>,
        _limit: u32,
    ) -> ExecutionResult<Vec<RevocationOperationV1>> {
        panic!("status must not drive effects")
    }
    async fn observe_barrier(&self, _receipt: &wire::BarrierReceiptV1) -> ExecutionResult<()> {
        panic!("status must not drive effects")
    }
    async fn observe_completion(&self, _completion: &MemberCompletionV1) -> ExecutionResult<()> {
        panic!("status must not drive effects")
    }
    async fn finish_revocation(
        &self,
        _operation: &wire::Digest,
    ) -> ExecutionResult<RevocationOperationV1> {
        panic!("status must not drive effects")
    }
    async fn prepare_effect(
        &self,
        _intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<wire::EffectPermissionV1> {
        panic!("status must not drive effects")
    }
    async fn observe_effect(
        &self,
        _observation: &wire::EffectObservationV1,
    ) -> ExecutionResult<()> {
        panic!("status must not drive effects")
    }
}

#[async_trait::async_trait]
impl ResourceAccessStore for Store {
    async fn organization_sandboxes(
        &self,
        _: &crate::SubjectId,
        _: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("no inventory")
    }
    async fn shared_sandboxes(
        &self,
        _: &crate::SubjectId,
        _: &[crate::SubjectId],
        _: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("no inventory")
    }
    async fn organization_agents(
        &self,
        _: &crate::SubjectId,
        _: &crate::sandbox::AgentListQuery,
        _: &crate::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("no inventory")
    }

    async fn resource_access_snapshot(
        &self,
        _: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.before_read(Read::Snapshot).await;
        Ok(self.0.lock().unwrap().snapshot.clone())
    }
}
#[async_trait::async_trait]
impl ResourceShareStore for Store {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        panic!("no mutation receipt")
    }
    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("no mutation")
    }
    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        panic!("no share lookup")
    }
}
struct Authority;
#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &crate::Caller,
        _: &str,
        _: &crate::SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        panic!("personal owner only")
    }
    async fn organization_choices(
        &self,
        _: &crate::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal owner only")
    }
}
pub(super) async fn setup(count: u32) -> (Arc<Store>, ResourceRevocationService) {
    let snapshot = ResourceAccessSnapshot {
        lineage: ResourceLineage {
            sandbox: super::snapshot::sandbox(),
            workspace: None,
            worktree: None,
            agent: None,
        },
        policy: None,
        shares: vec![],
    };
    let mut op = operation(count);
    op.intent.ancestry = ancestry_digest(&snapshot).unwrap();
    let store = Arc::new(Store(Mutex::new(State {
        operation: op,
        snapshot,
        members: (1..=count).map(member).collect(),
        effects: vec![],
        pages: vec![],
        missing_page: false,
        change_owner: false,
        reads: vec![],
        gates: vec![],
    })));
    let access = ResourceAccessService::new(store.clone(), Arc::new(Authority));
    let service = ResourceRevocationService::new(access, store.clone())
        .await
        .unwrap();
    (store, service)
}
