use super::*;
use layer2_core::sandbox::{SandboxWorkStep, WorkspacePrepareRecoveryCompletion};
use layer3_adapter::{WorkspacePrepareRecoveryReceipt, WorkspacePrepareRecoveryRequest};

impl WorkspaceWorker {
    pub(super) async fn recover_prepare(
        &self,
        mut workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        workspace.validate_prepare_recovery_binding()?;
        let recovery = workspace
            .work
            .prepare_recovery
            .as_ref()
            .ok_or(SandboxStoreError::WorkspaceRecoveryConflict)?;
        let attempt = recovery
            .attempts
            .last()
            .ok_or(SandboxStoreError::WorkspaceRecoveryConflict)?;
        if workspace.status != WorkspaceStatus::Preparing
            || workspace.ready_at.is_some()
            || workspace.resolved_commit.is_some()
            || attempt.failed.is_some()
            || attempt.completion.is_some()
        {
            return Err(SandboxStoreError::WorkspaceRecoveryConflict);
        }
        let request = recovery
            .current_request()
            .ok_or(SandboxStoreError::WorkspaceRecoveryConflict)?;
        if request.validate().is_err() {
            return Err(SandboxStoreError::WorkspaceRecoveryConflict);
        }
        let (parent, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => {
                return self
                    .handle_failure(workspace, failure.observed(INSPECTION_FAILURE), now)
                    .await;
            }
        };
        if parent.status != SandboxStatus::Active
            || parent.lifecycle.operation.is_some()
            || parent.lifecycle.checkpoint_capture.is_some()
            || parent.lifecycle.runtime_provision.is_some()
            || parent.lifecycle.home_recovery.is_some()
            || parent.lifecycle.unpublished_recovery.is_some()
            || parent.lifecycle.failed_provision.is_some()
            || parent.work.lease.is_some()
            || !matches!(
                parent.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
            )
        {
            return self
                .handle_failure(
                    workspace,
                    WorkerFailure::retryable("sandbox lifecycle is busy")
                        .observed(INSPECTION_FAILURE),
                    now,
                )
                .await;
        }
        layer2_core::sandbox::store::WorkAuthority::current()?;
        let call = self
            .dependencies
            .adapter
            .recover_workspace_prepare(&runtime, &request);
        let (reply, observation) = self.observe_prepare_rpc(call).await;
        match reply {
            Ok(receipt) if valid_receipt(&request, &receipt, &workspace.path) => {
                if let WorkspaceSource::RepositorySnapshot { source_commit, .. }
                | WorkspaceSource::PublicRepositorySnapshot { source_commit, .. } =
                    &mut workspace.source
                {
                    source_commit.clone_from(&receipt.prepared.source_commit);
                }
                workspace
                    .record_resolved_commit(receipt.prepared.resolved_commit.clone())
                    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                workspace
                    .work
                    .prepare_recovery
                    .as_mut()
                    .expect("validated recovery")
                    .attempts
                    .last_mut()
                    .expect("validated attempt")
                    .completion = Some(WorkspacePrepareRecoveryCompletion {
                    prepared: receipt.prepared,
                    preserved_path: receipt.preserved_path,
                    original_journal_sha256: receipt.original_journal_sha256,
                });
                workspace.validate_prepare_recovery_binding()?;
                self.advance(workspace, WorkspaceWorkStep::Setup, now).await
            }
            Ok(_) => {
                // A malformed reply is not proof that the effect failed. Keep
                // the same operation so a later exact journal replay can settle it.
                self.handle_failure(
                    workspace,
                    WorkerFailure::retryable("workspace recovery completion is not yet verified")
                        .observed(INVALID_RECEIPT),
                    now,
                )
                .await
            }
            Err(error) => {
                self.handle_failure(
                    workspace,
                    failure(error).observed_rpc(observation, observations::Stage::RecoveryRpc),
                    now,
                )
                .await
            }
        }
    }
}

fn valid_receipt(
    request: &WorkspacePrepareRecoveryRequest,
    receipt: &WorkspacePrepareRecoveryReceipt,
    path: &str,
) -> bool {
    let mut prepared_request = request.original.clone();
    prepared_request
        .operation_id
        .clone_from(&request.operation_id);
    prepared_request
        .request_sha256
        .clone_from(&request.request_sha256);
    valid_prepare_receipt(&prepared_request, &receipt.prepared, path)
        && receipt.preserved_path == request.preserved_path()
        && receipt.original_journal_sha256.len() == 64
        && receipt
            .original_journal_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_receipt_requires_real_original_identity_and_preserved_archive() {
        let original = WorkspaceRequest::new(
            "workspace:wks_test:prepare:v1",
            "wks_test",
            "project",
            RuntimeWorkspaceSource::Init,
        );
        let request = WorkspacePrepareRecoveryRequest::new("first", original, None);
        let receipt = WorkspacePrepareRecoveryReceipt {
            prepared: layer3_adapter::WorkspaceReceipt {
                operation_id: request.operation_id.clone(),
                request_sha256: request.request_sha256.clone(),
                workspace_id: request.original.workspace_id.clone(),
                name: request.original.name.clone(),
                path: "/home/cma/project".into(),
                resolved_commit: "a".repeat(40),
                source_commit: None,
            },
            preserved_path: request.preserved_path(),
            original_journal_sha256: "b".repeat(64),
        };
        assert!(valid_receipt(&request, &receipt, "/home/cma/project"));
        let mutations: [fn(&mut WorkspacePrepareRecoveryReceipt); 7] = [
            |r| r.prepared.operation_id = "old".into(),
            |r| r.prepared.request_sha256 = "changed".into(),
            |r| r.prepared.workspace_id = "foreign".into(),
            |r| r.prepared.path = "/home/cma/other".into(),
            |r| r.prepared.resolved_commit.clear(),
            |r| r.preserved_path = "/tmp/archive".into(),
            |r| r.original_journal_sha256.clear(),
        ];
        for mutation in mutations {
            let mut changed = receipt.clone();
            mutation(&mut changed);
            assert!(!valid_receipt(&request, &changed, "/home/cma/project"));
        }
    }

    #[test]
    fn snapshot_recovery_requires_a_source_commit_and_other_sources_refuse_one() {
        for source in [
            RuntimeWorkspaceSource::Init,
            RuntimeWorkspaceSource::Clone {
                url: "https://github.com/example/project".into(),
                reference: None,
            },
            RuntimeWorkspaceSource::RepositorySnapshot {
                url: "https://github.com/example/project".into(),
                reference: None,
            },
        ] {
            let snapshot = matches!(source, RuntimeWorkspaceSource::RepositorySnapshot { .. });
            let original = WorkspaceRequest::new(
                "workspace:wks_test:prepare:v1",
                "wks_test",
                "project",
                source,
            );
            let request = WorkspacePrepareRecoveryRequest::new("first", original, None);
            let mut receipt = WorkspacePrepareRecoveryReceipt {
                prepared: layer3_adapter::WorkspaceReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256.clone(),
                    workspace_id: request.original.workspace_id.clone(),
                    name: request.original.name.clone(),
                    path: "/home/cma/project".into(),
                    resolved_commit: "a".repeat(40),
                    source_commit: None,
                },
                preserved_path: request.preserved_path(),
                original_journal_sha256: "b".repeat(64),
            };
            assert_eq!(
                valid_receipt(&request, &receipt, "/home/cma/project"),
                !snapshot
            );
            for commit in [String::new(), "x".repeat(40), "a".repeat(39)] {
                receipt.prepared.source_commit = Some(commit);
                assert!(!valid_receipt(&request, &receipt, "/home/cma/project"));
            }
            for length in [40, 64] {
                receipt.prepared.source_commit = Some("c".repeat(length));
                assert_eq!(
                    valid_receipt(&request, &receipt, "/home/cma/project"),
                    snapshot
                );
            }
        }
    }
}
