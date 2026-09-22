use super::*;
use crate::bridge::workspace_cleanup_work::WorkspaceCleanupWork;

pub(super) struct WorkspaceFixture {
    pub d: DispatchFixture,
    pub adapter: Arc<adapter::WorkspaceAdapter>,
    pub request: WorkspaceCleanupRequest,
}
impl WorkspaceFixture {
    pub async fn new(shared: bool) -> Self {
        Self::with_capacity(shared, 4).await
    }
    pub async fn with_capacity(shared: bool, capacity: u8) -> Self {
        let mut original = CleanupFixture::with_capacity(shared, capacity).await;
        original.reset().await;
        let d = DispatchFixture::from_retired(original);
        let child = d.submit("original-child-removal").await.unwrap();
        let id = &child.receipt.as_ref().unwrap().id;
        let worker = d.worker("child-before-workspace");
        for _ in 0..6 {
            d.due(id).await;
            worker.run_one(id).await.unwrap();
        }
        assert_eq!(d.status().await.progress, ChildCleanupProgress::Complete);
        drop(worker);
        let original = &d.f.f.f.original;
        let bridge = original
            .bridges
            .bridge(&d.f.f.record.identity.bridge_id)
            .await
            .unwrap()
            .unwrap();
        let workspace = original
            .resources
            .get_workspace(&d.f.f.record.placement.target().workspace_id)
            .await
            .unwrap()
            .unwrap();
        let request = WorkspaceCleanupRequest {
            expected_bridge_revision: bridge.revision,
            expected_allocation_revision: d.f.f.record.revision,
            expected_workspace_revision: workspace.revision,
            child_cleanup_receipt_id: id.clone(),
            action: WorkspaceCleanupAction::RemoveOwnedWorkspace,
        };
        let adapter = Arc::new(adapter::WorkspaceAdapter::new(original.adapter.clone()));
        Self {
            d,
            adapter,
            request,
        }
    }
    pub fn store(&self) -> &Arc<crate::store::MongoBridgeAllocationStore> {
        &self.d.f.f.f.original.allocations
    }
    pub fn worker(&self) -> WorkspaceCleanupWork {
        WorkspaceCleanupWork {
            allocations: self.store().clone(),
            authority: self.d.authority.clone(),
            adapter: self.adapter.clone(),
            deployment_id: self.d.f.f.f.original.settings.deployment_id.clone(),
            holder: "workspace-worker".into(),
        }
    }
    pub async fn submit(&self, key: &str) -> Result<WorkspaceCleanupStatus, CleanupError> {
        self.submit_request(key, self.request.clone()).await
    }
    pub async fn submit_request(
        &self,
        key: &str,
        request: WorkspaceCleanupRequest,
    ) -> Result<WorkspaceCleanupStatus, CleanupError> {
        let original = &self.d.f.f.f.original;
        self.d
            .service
            .workspace_submit(
                original.lease.actor_context(),
                Some(original.lease.credential()),
                &self.d.f.f.record.identity.bridge_id,
                WorkspaceCleanupCommand {
                    allocation_id: self.d.f.f.record.id.clone(),
                    idempotency_key: key.into(),
                    request,
                },
            )
            .await
    }
    pub async fn status(&self) -> Result<WorkspaceCleanupStatus, CleanupError> {
        let original = &self.d.f.f.f.original;
        self.d
            .service
            .workspace_status(
                original.lease.actor_context(),
                Some(original.lease.credential()),
                &self.d.f.f.record.identity.bridge_id,
                &self.d.f.f.record.id,
            )
            .await
    }
    pub async fn workspace(&self) -> Workspace {
        self.d
            .f
            .f
            .f
            .original
            .resources
            .get_workspace(&self.d.f.f.record.placement.target().workspace_id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn due(&self, id: &str) {
        let mut row = self.d.f.row(JOBS, id).await;
        let mut job: WorkspaceCleanupJob = crate::store::sandbox_mongo::read_record(&row).unwrap();
        job.next_attempt_at_ms = job.admission.receipt.accepted_at_ms;
        row.insert("next_attempt_at_ms", job.next_attempt_at_ms);
        row.insert(
            "record",
            crate::store::sandbox_mongo::record_bson(&job).unwrap(),
        );
        self.d
            .f
            .db()
            .collection::<Document>(JOBS)
            .replace_one(doc! {"_id":id}, row)
            .await
            .unwrap();
    }
    pub async fn advance(&self, id: &str, count: usize) {
        let worker = self.worker();
        for _ in 0..count {
            self.due(id).await;
            worker.run_one(id).await.unwrap();
        }
    }
    pub async fn release_proof(
        &self,
    ) -> Result<bool, crate::store::bridge_allocations::MutationError> {
        let mut session = self.d.f.f.f.original.client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let result = self
            .store()
            .cleanup_released_workspace_capacity(&self.d.f.f.record, &mut session)
            .await;
        let _ = session.abort_transaction().await;
        result
    }
    pub async fn plan(&self) -> Result<RetiredRuntimeCleanupPlan, SandboxStoreError> {
        self.d
            .f
            .f
            .f
            .original
            .resources
            .retired_runtime_cleanup_plan(&self.d.f.f.sandbox().await)
            .await
    }
    pub fn manual(&self, workspace: &Workspace, key: &str) -> AgentAdmission {
        AgentAdmission {
            binding: None,
            created_by: Some(caller().actor),
            owner: workspace.owner.clone(),
            workspace_id: workspace.id.clone(),
            agent_id: layer2_core::AgentId::generate(),
            worktree_id: layer2_core::WorktreeId::generate(),
            profile: self.d.f.f.f.sibling.profile.clone(),
            capabilities: self.d.f.f.f.sibling.capabilities.clone(),
            title: None,
            first_message: None,
            origin: AgentOrigin::Manual,
            creation_key: key.into(),
            event_at: layer2_core::now_rfc3339(),
        }
    }
    pub async fn close(self) {
        drop(self.adapter);
        self.d.close().await;
    }
}
