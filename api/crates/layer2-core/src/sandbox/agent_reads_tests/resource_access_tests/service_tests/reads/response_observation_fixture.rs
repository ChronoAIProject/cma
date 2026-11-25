use super::*;

pub(super) struct CountedAccess {
    pub(super) inner: Arc<InMemorySandboxStore>,
    pub(super) delays: Mutex<Vec<Duration>>,
    pub(super) calls: AtomicUsize,
    pub(super) gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}
#[async_trait]
impl ResourceAccessStore for CountedAccess {
    async fn organization_sandboxes(
        &self,
        organization: &crate::SubjectId,
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner.organization_sandboxes(organization, query).await
    }

    async fn shared_sandboxes(
        &self,
        recipient: &crate::SubjectId,
        organizations: &[crate::SubjectId],
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        self.inner
            .shared_sandboxes(recipient, organizations, query)
            .await
    }

    async fn organization_agents(
        &self,
        organization: &SubjectId,
        query: &AgentListQuery,
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
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == 3 {
            if let Some((entered, resume)) = &self.gate {
                entered.notify_one();
                resume.notified().await;
            }
        }
        let delay = self
            .delays
            .lock()
            .unwrap()
            .get(call - 1)
            .copied()
            .unwrap_or_default();
        tokio::time::sleep(delay).await;
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
