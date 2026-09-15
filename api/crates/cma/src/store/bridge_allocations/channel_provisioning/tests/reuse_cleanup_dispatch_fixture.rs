//! Real cleanup service/job/Stage A/B/C with owned Mongo and recorded runtime IO.
use super::*;
use crate::bridge::{
    cleanup_work::{ChannelCleanupWork, CleanupWorkAuthority},
    provisioning::reuse_cleanup::ChannelReuseCleanupAuthority,
};
use layer2_core::{
    bridge::{cleanup::*, BridgeManagementLease},
    resource_access::ResourceAccessService,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(super) struct DispatchAuthority {
    pub f: Arc<CleanupFixture>,
    pub available: AtomicBool,
    pub calls: AtomicUsize,
    pause: std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
}
impl DispatchAuthority {
    pub fn pause_once(&self) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let reached = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        *self.pause.lock().unwrap() = Some((reached.clone(), resume.clone()));
        (reached, resume)
    }
}
#[async_trait::async_trait]
impl CleanupWorkAuthority for DispatchAuthority {
    async fn resolve(
        &self,
        allocation: &AllocationRecord,
    ) -> Result<ChannelReuseCleanupAuthority, ChildCleanupHold> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.available.load(Ordering::SeqCst) || allocation != &self.f.f.record {
            return Err(ChildCleanupHold::AuthorityUnavailable);
        }
        let permit = self.f.permit().await;
        let pause = self.pause.lock().unwrap().take();
        if let Some((reached, resume)) = pause {
            reached.notify_one();
            resume.notified().await;
        }
        Ok(permit)
    }
}

struct FixtureStore(Arc<CleanupFixture>);
#[async_trait::async_trait]
impl BridgeCleanupStore for FixtureStore {
    async fn allocations(
        &self,
        manager: &BridgeManagementLease,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AllocationRecord>, CleanupError> {
        self.0
            .f
            .f
            .original
            .allocations
            .managed_cleanup_allocations(manager, after, limit)
            .await
    }
    async fn allocation(
        &self,
        manager: &BridgeManagementLease,
        id: &str,
    ) -> Result<Option<AllocationRecord>, CleanupError> {
        self.0
            .f
            .f
            .original
            .allocations
            .managed_cleanup_allocation(manager, id)
            .await
    }
    async fn status(&self, access: &CleanupAccess) -> Result<ChildCleanupStatus, CleanupError> {
        self.0
            .f
            .f
            .original
            .allocations
            .managed_cleanup_status(access)
            .await
    }
    async fn recover(
        &self,
        access: &CleanupAccess,
        id: &str,
        digest: &str,
    ) -> Result<Option<ChildCleanupStatus>, CleanupError> {
        self.0
            .f
            .f
            .original
            .allocations
            .recover_cleanup_job(access, id, digest)
            .await
    }
    async fn submit(&self, write: CleanupWrite) -> Result<ChildCleanupStatus, CleanupError> {
        self.0
            .f
            .f
            .original
            .allocations
            .admit_cleanup_job(&write, &self.0.permit().await)
            .await
    }
}

pub(super) struct DispatchFixture {
    pub f: Arc<CleanupFixture>,
    pub authority: Arc<DispatchAuthority>,
    pub service: BridgeCleanupService,
}
impl DispatchFixture {
    pub async fn new(shared_workspace: bool) -> Self {
        let mut f = CleanupFixture::new(shared_workspace).await;
        f.reset().await;
        Self::from_retired(f)
    }
    pub fn from_retired(f: CleanupFixture) -> Self {
        let f = Arc::new(f);
        let original = &f.f.f.original;
        let service = BridgeCleanupService::new(
            original.svc.clone(),
            Arc::new(ResourceAccessService::new(
                original.resources.clone(),
                Arc::new(Authority),
            )),
            Arc::new(FixtureStore(f.clone())),
        );
        let authority = Arc::new(DispatchAuthority {
            f: f.clone(),
            available: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            pause: std::sync::Mutex::new(None),
        });
        Self {
            f,
            authority,
            service,
        }
    }
    pub fn worker(&self, holder: &str) -> ChannelCleanupWork {
        ChannelCleanupWork {
            allocations: self.f.f.f.original.allocations.clone(),
            authority: self.authority.clone(),
            adapter: self.f.f.f.original.adapter.clone(),
            registry: self.f.f.registry.clone(),
            deployment_id: self.f.f.f.original.settings.deployment_id.clone(),
            holder: holder.into(),
        }
    }
    pub async fn submit(&self, key: &str) -> Result<ChildCleanupStatus, CleanupError> {
        self.submit_as(key, self.f.f.f.original.lease.actor_context())
            .await
    }
    pub async fn submit_as(
        &self,
        key: &str,
        context: &layer2_core::resource_access::CallerContext,
    ) -> Result<ChildCleanupStatus, CleanupError> {
        let original = &self.f.f.f.original;
        let bridge = original
            .bridges
            .bridge(&self.f.f.record.identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        self.service
            .submit(
                context,
                Some(original.lease.credential()),
                &bridge.id,
                ChildCleanupCommand {
                    allocation_id: self.f.f.record.id.clone(),
                    idempotency_key: key.into(),
                    request: ChildCleanupRequest {
                        expected_bridge_revision: bridge.revision,
                        expected_allocation_revision: self.f.f.record.revision,
                        action: ChildCleanupAction::RemoveChild,
                    },
                },
            )
            .await
    }
    pub async fn status(&self) -> ChildCleanupStatus {
        let original = &self.f.f.f.original;
        self.service
            .status(
                original.lease.actor_context(),
                Some(original.lease.credential()),
                &self.f.f.record.identity.bridge_id,
                &self.f.f.record.id,
            )
            .await
            .unwrap()
    }
    pub async fn due(&self, id: &str) {
        let rows = self
            .f
            .db()
            .collection::<Document>("channel_child_cleanup_jobs_v1");
        let mut row = self.f.row("channel_child_cleanup_jobs_v1", id).await;
        let mut job: CleanupJob = crate::store::sandbox_mongo::read_record(&row).unwrap();
        // Fault/time fixture only changes scheduling, never acceptance or proof.
        job.next_attempt_at_ms = job.admission.receipt.accepted_at_ms;
        job.validate().unwrap();
        row.insert("next_attempt_at_ms", job.next_attempt_at_ms);
        row.insert(
            "record",
            crate::store::sandbox_mongo::record_bson(&job).unwrap(),
        );
        rows.replace_one(doc! {"_id":id}, row).await.unwrap();
    }
    pub async fn expire_claim(&self, id: &str) {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let rows = self
            .f
            .db()
            .collection::<Document>("channel_child_cleanup_jobs_v1");
        let mut row = self.f.row("channel_child_cleanup_jobs_v1", id).await;
        let mut job: CleanupJob = crate::store::sandbox_mongo::read_record(&row).unwrap();
        job.lease.as_mut().unwrap().expires_at_ms =
            mongodb::bson::DateTime::now().timestamp_millis() - 1;
        job.next_attempt_at_ms = job.admission.receipt.accepted_at_ms;
        job.validate().unwrap();
        row.insert(
            "lease_expires_at_ms",
            job.lease.as_ref().unwrap().expires_at_ms,
        );
        row.insert("next_attempt_at_ms", job.next_attempt_at_ms);
        row.insert(
            "record",
            crate::store::sandbox_mongo::record_bson(&job).unwrap(),
        );
        rows.replace_one(doc! {"_id":id}, row).await.unwrap();
    }
    pub async fn close(self) {
        drop(self.service);
        drop(self.authority);
        match Arc::try_unwrap(self.f) {
            Ok(f) => f.close().await,
            Err(_) => panic!("dispatch fixture still has live workers"),
        }
    }
}
