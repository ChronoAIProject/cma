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
    let commands = run_shell_commands(
        &request.commands,
        &workspace,
        &config.process_env,
        CommandLimits::setup(),
    )
    .await?;
    Ok(SetupReceipt { commands })
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
