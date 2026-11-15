use super::*;
use layer2_core::sandbox::store::{ClaimedWork, WorkAuthority, WORK_LEASE_SECONDS};

impl WorkspaceWorker {
    pub(super) async fn run_renewable(
        &self,
        workspace: Workspace,
        now: SystemTime,
        claiming: tokio::time::Instant,
    ) -> Result<bool, SandboxStoreError> {
        let parent = tokio::time::timeout(
            Duration::from_secs(layer2_core::sandbox::store::WORK_RENEW_SECONDS),
            self.dependencies.store.get_sandbox(&workspace.sandbox_id),
        )
        .await
        .map_err(|_| SandboxStoreError::Conflict)??
        .ok_or(SandboxStoreError::NotFound)?;
        let authority = WorkAuthority::new(
            ClaimedWork::Workspace(Box::new(workspace.clone())),
            claiming,
        )
        .with_parent(parent)?;
        // Keep the authority scope's stack frame bounded: workspace execution
        // is a large state machine and must not be embedded in the renewal loop.
        let work = Box::pin(self.execute(workspace.clone(), now));
        let result = self
            .dependencies
            .store
            .scope_workspace_publication(Box::pin(async {
                authority
                    .run(work, || {
                        Box::pin(async {
                            let at = now + claiming.elapsed();
                            let grant = self
                                .dependencies
                                .store
                                .renew_workspace_authority(
                                    &workspace,
                                    &rfc3339_at(at),
                                    &rfc3339_at(at + Duration::from_secs(WORK_LEASE_SECONDS)),
                                )
                                .await?;
                            if let Some(grant) = &grant {
                                tracing::debug!(workspace_id = %workspace.id,
                                    valid_for_ms = grant.valid_for.as_millis() as u64,
                                    "Workspace work authority renewed");
                            }
                            Ok(grant)
                        })
                    })
                    .await
                    .map(|()| true)
            }))
            .await;
        if result.is_err() {
            tracing::warn!(workspace_id = %workspace.id, step = ?workspace.work.step,
                "Workspace work ended without renewed publication authority");
        }
        result
    }
}
