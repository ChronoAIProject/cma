use std::path::Path;

use layer3_adapter::{SetupReceipt, WorkspaceSetupRequest};

use crate::command::{run_shell_commands, CommandLimits};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::{path_is_directory, safe_component};

pub(crate) async fn run(
    config: &RuntimeConfig,
    request: &WorkspaceSetupRequest,
) -> Result<SetupReceipt> {
    safe_component(&request.name, "workspace name")?;
    let workspace = config.home.join(&request.name);
    require_workspace(&workspace)?;
    crate::workspace_source::require_workspace_identity(&workspace, &request.workspace_id)?;
    let commands = run_shell_commands(
        &request.commands,
        &workspace,
        &config.process_env,
        CommandLimits::install(
            request.timeout_seconds,
            request.total_timeout_seconds,
            request.output_limit_bytes,
        )?,
    )
    .await?;
    Ok(SetupReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        commands,
    })
}

fn require_workspace(path: &Path) -> Result<()> {
    if !path_is_directory(path) || !path_is_directory(&path.join(".git")) {
        return Err(RuntimeError::rejected(
            "workspace_not_found",
            "workspace is not a main Git checkout",
        ));
    }
    Ok(())
}
