use layer3_adapter::{
    same_runtime_writer, RuntimeStatusRequest, WorkspaceRemovalObservation,
    WorkspaceRemovalOutcome, WorkspaceRemovalRequest, WorkspaceRemovalSupport,
    WorkspaceRemoveReceipt, WORKSPACE_REMOVAL_PROTOCOL,
};

use super::{writers_held_error, Supervisor, WriterFence};
use crate::failure::{Result, RuntimeError};
use crate::workspace_journal::{self, Begin};
use crate::workspace_removal_paths::{self, Presence};

impl Supervisor {
    pub(crate) fn removal_writer_guard(&self) -> Result<Option<std::fs::File>> {
        if self.config.home_identity.is_none() {
            return Ok(None);
        }
        crate::workspace_removal_writer::guard(&self.config, &self.boot_id).map(Some)
    }

    pub(super) async fn workspace_removal_support(
        &self,
        request: RuntimeStatusRequest,
    ) -> Result<WorkspaceRemovalSupport> {
        self.require_removal_process_boundary()?;
        let _writer = crate::workspace_removal_writer::guard(&self.config, &self.boot_id)?;
        Ok(WorkspaceRemovalSupport {
            runtime: self.runtime_status(request).await?,
            protocol_version: WORKSPACE_REMOVAL_PROTOCOL,
        })
    }

    pub(super) async fn reconcile_workspace_removal(
        &self,
        request: WorkspaceRemovalRequest,
    ) -> Result<WorkspaceRemovalObservation> {
        request.validate().map_err(|_| invalid())?;
        self.require_removal_process_boundary()?;
        let _operation = self.operation_gate.lock().await;
        self.mutations.require_serving().await?;
        if self.held_checkpoint.lock().await.is_some() {
            return Err(writers_held_error());
        }
        self.require_credentialed().await?;
        let _writer = crate::workspace_removal_writer::guard(&self.config, &self.boot_id)?;
        let current = self.runtime_status(request.runtime_request()).await?;
        if !same_runtime_writer(&current, &request.runtime) {
            return Err(invalid());
        }
        self.ensure_workspace_idle(&request.request.name).await?;
        let roots = self
            .agents
            .read()
            .await
            .values()
            .filter_map(|agent| agent.bridge.writer_session_root())
            .collect::<Vec<_>>();
        let mut writers = WriterFence::quiesce(roots, self.writer_boundary).map_err(|error| {
            RuntimeError::unavailable("workspace_removal_writers_unavailable", error)
        })?;
        // No await between the final home/boot authority check and filesystem IO.
        // The cross-process writer lock also excludes replacement initialization.
        crate::persistent_home::verify(&self.config)?.ok_or_else(invalid)?;
        let effect = self.reconcile_workspace_paths(&request);
        writers.resume().map_err(|error| {
            RuntimeError::unavailable("workspace_removal_writer_resume_failed", error)
        })?;
        let (receipt, outcome) = effect?;
        let runtime = self.runtime_status(request.runtime_request()).await?;
        let observation = WorkspaceRemovalObservation {
            runtime,
            receipt,
            outcome,
        };
        observation.validate_for(&request).map_err(|_| invalid())?;
        Ok(observation)
    }

    fn require_removal_process_boundary(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        if self.writer_boundary.root.is_none() && !cfg!(test) {
            return Err(invalid());
        }
        #[cfg(not(target_os = "linux"))]
        if !cfg!(test) {
            return Err(invalid());
        }
        Ok(())
    }

    fn reconcile_workspace_paths(
        &self,
        frame: &WorkspaceRemovalRequest,
    ) -> Result<(WorkspaceRemoveReceipt, WorkspaceRemovalOutcome)> {
        let config = &self.config;
        let request = &frame.request;
        // Path inspection precedes journal admission, so unsafe/mismatched bytes
        // cannot acquire a new successful canonical removal barrier.
        let presence =
            workspace_removal_paths::inspect(&config.home, &request.workspace_id, &request.name)?;
        crate::workspace_removal_intent::record(config, request)?;
        let began = workspace_journal::begin::<WorkspaceRemoveReceipt, _>(
            &config.state_root,
            &config.home,
            config.allow_local_git,
            &config.sandbox_id,
            &request.workspace_id,
            "remove",
            &request.operation_id,
            &request.request_sha256,
            request,
        )?;
        let (receipt, ticket) = match began {
            Begin::Completed(receipt) => (receipt, None),
            Begin::New(ticket) | Begin::Started(ticket) => (
                WorkspaceRemoveReceipt {
                    operation_id: request.operation_id.clone(),
                    request_sha256: request.request_sha256.clone(),
                    workspace_id: request.workspace_id.clone(),
                },
                Some(ticket),
            ),
            Begin::Failed(error) | Begin::Indeterminate(error) => return Err(error),
        };
        if presence == Presence::Successor {
            if ticket.is_some() {
                return Err(RuntimeError::rejected(
                    "workspace_incarnation_mismatch",
                    "Workspace path belongs to a successor; no current removal was performed",
                ));
            }
            return Ok((receipt, WorkspaceRemovalOutcome::SuccessorPresent));
        }
        if presence == Presence::Original {
            crate::workspace_source::remove(&config.home, &request.workspace_id, &request.name)?;
        }
        if workspace_removal_paths::inspect(&config.home, &request.workspace_id, &request.name)?
            != Presence::Absent
        {
            return Err(RuntimeError::unavailable(
                "workspace_removal_unconfirmed",
                "Workspace absence could not be confirmed",
            ));
        }
        if let Some(ticket) = ticket {
            ticket.complete(&receipt)?;
        }
        Ok((receipt, WorkspaceRemovalOutcome::CurrentlyAbsent))
    }
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected("workspace_removal_runtime_invalid", "guarded Workspace removal requires the exact current credentialed runtime boot and writer boundary")
}
