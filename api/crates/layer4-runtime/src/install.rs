use layer3_adapter::{InstallReceipt, InstallRequest};

use crate::command::{run_shell_commands, CommandLimits};
use crate::config::RuntimeConfig;
use crate::failure::Result;

pub(crate) async fn run(
    config: &RuntimeConfig,
    request: &InstallRequest,
) -> Result<InstallReceipt> {
    let limits = CommandLimits::install(request.timeout_seconds, request.output_limit_bytes)?;
    let commands =
        run_shell_commands(&request.commands, &config.home, &config.install_env, limits).await?;
    Ok(InstallReceipt { commands })
}
