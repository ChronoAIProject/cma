use super::*;

impl WorkspaceWorker {
    pub(super) async fn setup(
        &self,
        workspace: Workspace,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let workspace = self.pin_execution(workspace).await?;
        let (_, runtime) = match self.runtime(&workspace).await {
            Ok(value) => value,
            Err(failure) => return self.handle_failure(workspace, failure, now).await,
        };
        let execution = workspace
            .work
            .execution
            .expect("execution policy was persisted");
        let request = WorkspaceSetupRequest::with_timeouts(
            workspace.operation_id(WorkspaceWorkStep::Setup),
            workspace.id.to_string(),
            workspace.name.clone(),
            workspace.profile.content.preparation_commands(),
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        );
        layer2_core::sandbox::store::WorkAuthority::current()?;
        match self
            .dependencies
            .adapter
            .run_workspace_setup(&runtime, &request)
            .await
        {
            Ok(receipt) if valid_setup_receipt(&request, &receipt) => {
                let commands = workspace.profile.content.credential_commands();
                if !commands.is_empty() {
                    let binding = WorkspaceSetupRequest::with_timeouts(
                        format!("workspace:{}:credential-setup:v1", workspace.id),
                        workspace.id.to_string(),
                        workspace.name.clone(),
                        commands,
                        execution.command_timeout_seconds,
                        execution.total_timeout_seconds,
                        layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
                    );
                    layer2_core::sandbox::store::WorkAuthority::current()?;
                    match self
                        .dependencies
                        .adapter
                        .run_workspace_setup(&runtime, &binding)
                        .await
                    {
                        Ok(receipt) if valid_setup_receipt(&binding, &receipt) => (),
                        Ok(_) => {
                            return self
                                .handle_failure(workspace, WorkerFailure::invalid_receipt(), now)
                                .await
                        }
                        Err(error) => {
                            return self.handle_failure(workspace, failure(error), now).await
                        }
                    }
                }
                self.mark_ready(workspace, now).await
            }
            Ok(_) => {
                self.handle_failure(workspace, WorkerFailure::invalid_receipt(), now)
                    .await
            }
            Err(error) => self.handle_failure(workspace, failure(error), now).await,
        }
    }
}
