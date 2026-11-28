use super::*;
use layer2_core::sandbox::store::{
    ClaimedWork, WorkAuthority, WorkAuthorityFailure, WORK_LEASE_SECONDS,
};
use std::sync::atomic::{AtomicBool, Ordering};

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
        let parent_fence = parent.work.lease.as_ref().map(|lease| lease.fence);
        let authority = WorkAuthority::new(
            ClaimedWork::Workspace(Box::new(workspace.clone())),
            claiming,
        )
        .with_parent(parent)?;
        // Keep the authority scope's stack frame bounded: workspace execution
        // is a large state machine and must not be embedded in the renewal loop.
        let executing = AtomicBool::new(false);
        let work = Box::pin(async {
            executing.store(true, Ordering::Relaxed);
            self.execute(workspace.clone(), now).await
        });
        self.dependencies
            .store
            .scope_workspace_publication(Box::pin(async {
                let result = authority
                    .run_observed(work, || {
                        Box::pin(async {
                            let at = now + claiming.elapsed();
                            let renewal = if executing.load(Ordering::Relaxed) {
                                "periodic"
                            } else {
                                "initial"
                            };
                            let remaining_ms = authority
                                .deadline()?
                                .saturating_duration_since(tokio::time::Instant::now())
                                .as_millis() as u64;
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
                                    sandbox_id = %workspace.sandbox_id,
                                    claim_fence = workspace.work.lease.as_ref().map(|lease| lease.fence),
                                    parent_fence, renewal, remaining_ms,
                                    valid_for_ms = grant.valid_for.as_millis() as u64,
                                    "Workspace work authority renewed");
                            }
                            Ok(grant)
                        })
                    })
                    .await;
                result.map(|()| true).map_err(|failure| {
                    let outcome = match &failure {
                        WorkAuthorityFailure::Authority(_) => "authority_closed",
                        WorkAuthorityFailure::RenewalTimeout => "renewal_timeout",
                        WorkAuthorityFailure::RenewalRefused => "renewal_refused",
                        WorkAuthorityFailure::Renewal(_) => "renewal_error",
                        WorkAuthorityFailure::InvalidGrant(_) => "invalid_grant",
                        WorkAuthorityFailure::Execution(_) => "execution_error",
                    };
                    let error = failure.into_store_error();
                    let store_result = workspace_store_result(Some(&error));
                    tracing::warn!(
                        workspace_id = %workspace.id,
                        sandbox_id = %workspace.sandbox_id,
                        step = ?workspace.work.step,
                        claim_fence = workspace.work.lease.as_ref().map(|lease| lease.fence),
                        parent_fence,
                        execution_started = executing.load(Ordering::Relaxed),
                        outcome,
                        store_result,
                        "Workspace work ended without renewed publication authority"
                    );
                    error
                })
            }))
            .await
    }
}

pub(super) fn workspace_store_result(error: Option<&SandboxStoreError>) -> &'static str {
    match error {
        None => "committed",
        Some(SandboxStoreError::Conflict) => "conflict",
        Some(SandboxStoreError::Unavailable(_)) => "unavailable",
        Some(SandboxStoreError::NotFound) => "not_found",
        Some(SandboxStoreError::Corrupt(_)) => "corrupt",
        Some(_) => "other",
    }
}
