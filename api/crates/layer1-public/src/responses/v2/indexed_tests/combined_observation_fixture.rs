use super::*;

pub(super) struct CountedAccess {
    pub(super) inner: Arc<InMemorySandboxStore>,
    pub(super) calls: AtomicUsize,
}
#[async_trait]
impl ResourceAccessStore for CountedAccess {
    async fn organization_sandboxes(
        &self,
        organization: &SubjectId,
        query: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_sandboxes(organization, query).await
    }

    async fn shared_sandboxes(
        &self,
        recipient: &SubjectId,
        organizations: &[SubjectId],
        query: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .shared_sandboxes(recipient, organizations, query)
            .await
    }

    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &layer2_core::sandbox::AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .organization_agents(organization, query, page)
            .await
    }

    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.resource_access_snapshot(target).await
    }
}
#[async_trait]
impl ResourceShareStore for CountedAccess {
    async fn share_mutation_receipt(
        &self,
        scope: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.inner.share_mutation_receipt(scope).await
    }
    async fn mutate_share(
        &self,
        command: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        self.inner.mutate_share(command).await
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        self.inner.resource_share(id).await
    }
}
