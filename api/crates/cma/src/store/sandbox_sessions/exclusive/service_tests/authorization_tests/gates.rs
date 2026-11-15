use super::*;
use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStore, ResourceAccessStoreError, ResourceShare,
    ResourceShareStore, ResourceTarget, ShareMutation, ShareMutationReceipt,
};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::{Notify, Semaphore};

pub(super) struct Gate {
    pause_at: usize,
    calls: AtomicUsize,
    pub(super) active: AtomicBool,
    pub(super) cancelled: AtomicUsize,
    pub(super) deny: AtomicBool,
    pub(super) entered: Notify,
    resume: Semaphore,
}

impl Gate {
    pub(super) fn new(pause_at: usize) -> Arc<Self> {
        Arc::new(Self {
            pause_at,
            calls: AtomicUsize::new(0),
            active: AtomicBool::new(false),
            cancelled: AtomicUsize::new(0),
            deny: AtomicBool::new(false),
            entered: Notify::new(),
            resume: Semaphore::new(0),
        })
    }

    pub(super) fn release(&self) {
        self.resume.add_permits(1);
    }

    async fn check(&self) -> Result<(), ()> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 != self.pause_at {
            return Ok(());
        }
        self.active.store(true, Ordering::SeqCst);
        let mut pending = Pending(self, true);
        self.entered.notify_one();
        self.resume.acquire().await.unwrap().forget();
        pending.1 = false;
        if self.deny.load(Ordering::SeqCst) {
            Err(())
        } else {
            Ok(())
        }
    }
}

struct Pending<'a>(&'a Gate, bool);
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::SeqCst);
        if self.1 {
            self.0.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[async_trait::async_trait]
impl layer2_core::sandbox::DesktopAuthorization for Gate {
    async fn current(&self, _: &Caller) -> Result<(), DesktopError> {
        self.check().await.map_err(|_| DesktopError::Unavailable)
    }
}

pub(super) struct Access {
    pub(super) store: MongoSandboxStore,
    pub(super) gate: Arc<Gate>,
}

#[async_trait::async_trait]
impl ResourceAccessStore for Access {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        self.gate
            .check()
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        self.store.resource_access_snapshot(target).await
    }

    async fn organization_sandboxes(
        &self,
        _: &layer2_core::SubjectId,
        _: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("desktop must not read organization inventory")
    }

    async fn shared_sandboxes(
        &self,
        _: &layer2_core::SubjectId,
        _: &[layer2_core::SubjectId],
        _: &layer2_core::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("desktop must not read shared inventory")
    }

    async fn organization_agents(
        &self,
        _: &layer2_core::SubjectId,
        _: &layer2_core::sandbox::AgentListQuery,
        _: &layer2_core::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        panic!("desktop must not read organization inventory")
    }
}

#[async_trait::async_trait]
impl ResourceShareStore for Access {
    async fn share_mutation_receipt(
        &self,
        _: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        panic!("desktop must not read share mutation receipts")
    }

    async fn mutate_share(
        &self,
        _: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        panic!("desktop must not mutate shares")
    }

    async fn resource_share(
        &self,
        _: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        panic!("desktop must not read share management records")
    }
}
