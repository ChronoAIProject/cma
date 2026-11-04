//! Bounded transport for trusted provider maintenance in an owned Pod.
//! The lifecycle adapter authorizes each command; this is not public arbitrary exec.
use std::time::Duration;

use kube::api::AttachParams;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::{api_error, rejected, unavailable, AgentSandboxClient};
use crate::hosted::RuntimeRecoveryCommand;
use crate::{AdapterError, Result, RuntimeHandle};

const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const IO_LIMIT: usize = 64 * 1024;
const ARGUMENT_LIMIT: usize = 64 * 1024;

impl AgentSandboxClient {
    pub(crate) async fn run_recovery_command(
        &self,
        runtime: &RuntimeHandle,
        command: &RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        validate_command(command)?;
        // Share one-shot capacity with runtime requests, without an unbounded queue.
        // Hold it through the final identity check; clones share the same budget.
        let _permit = self
            .runtime_requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                AdapterError::AtCapacity("runtime recovery command capacity is exhausted".into())
            })?;
        tokio::time::timeout(RECOVERY_TIMEOUT, async {
            let before = self.pod_binding(runtime).await?;
            let (name, uid) = super::parse_handle(runtime)?;
            if before.sandbox_name != name || before.sandbox_uid != uid {
                return Err(rejected(
                    "runtime recovery command Sandbox identity mismatch",
                ));
            }
            if before
                .runtime_container_id
                .as_deref()
                .is_none_or(str::is_empty)
                || before.runtime_restart_count.is_none_or(|count| count < 0)
            {
                return Err(rejected(
                    "runtime recovery command container identity is missing",
                ));
            }
            let output = self.inspect_pod(&before.pod_name, command).await?;
            if self.pod_binding(runtime).await? != before {
                return Err(rejected(
                    "Sandbox Pod changed during runtime recovery command",
                ));
            }
            Ok(output)
        })
        .await
        .map_err(|_| unavailable("runtime recovery command timed out"))?
    }

    pub(super) async fn inspect_pod(
        &self,
        pod_name: &str,
        command: &RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        let mut parameters = AttachParams::default()
            .container("runtime")
            .stdin(true)
            .stdout(true)
            .stderr(false)
            .tty(false);
        parameters.max_stdin_buf_size = Some(IO_LIMIT);
        parameters.max_stdout_buf_size = Some(IO_LIMIT);
        // No shell or interpolation. The provider supplies a fixed maintenance
        // script; sensitive configuration travels only through bounded stdin.
        let mut process = self
            .pods
            .exec(
                pod_name,
                std::iter::once(command.program.as_str())
                    .chain(command.args.iter().map(String::as_str)),
                &parameters,
            )
            .await
            .map_err(api_error)?;
        // AttachedProcess aborts its transport on drop, including cancellation,
        // timeout and every bounded-I/O or status failure below.
        let mut stdin = process
            .stdin()
            .ok_or_else(|| unavailable("runtime recovery command stdin missing"))?;
        let stdout = process
            .stdout()
            .ok_or_else(|| unavailable("runtime recovery command stdout missing"))?;
        let status = process
            .take_status()
            .ok_or_else(|| unavailable("runtime recovery command status missing"))?;
        let send = async {
            stdin
                .write_all(&command.input)
                .await
                .map_err(|_| unavailable("runtime recovery command write failed"))?;
            stdin
                .shutdown()
                .await
                .map_err(|_| unavailable("runtime recovery command EOF failed"))?;
            drop(stdin);
            Ok(())
        };
        let exit = async {
            if status
                .await
                .as_ref()
                .and_then(|status| status.status.as_deref())
                != Some("Success")
            {
                return Err(unavailable(
                    "runtime recovery command command did not succeed",
                ));
            }
            Ok(())
        };
        let ((), output, ()) = tokio::try_join!(send, bounded_output(stdout), exit)?;
        // Stdout EOF and status have both resolved before joining, so the
        // transport task has finished consuming remote frames.
        process
            .join()
            .await
            .map_err(|_| unavailable("runtime recovery command stream failed"))?;
        Ok(output)
    }
}

fn validate_command(command: &RuntimeRecoveryCommand) -> Result<()> {
    if !command.program.starts_with('/')
        || command.program.as_bytes().contains(&0)
        || command.program.len() > ARGUMENT_LIMIT
        || command.args.len() > 64
        || command.args.iter().any(|arg| arg.as_bytes().contains(&0))
        || command
            .args
            .iter()
            .try_fold(command.program.len(), |total, arg| {
                total.checked_add(arg.len())
            })
            .is_none_or(|total| total > ARGUMENT_LIMIT)
        || command.input.len() > IO_LIMIT
    {
        return Err(rejected(
            "runtime recovery command command exceeds its contract",
        ));
    }
    Ok(())
}

async fn bounded_output(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(IO_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable("runtime recovery command output failed"))?;
    if bytes.len() > IO_LIMIT {
        return Err(rejected(
            "runtime recovery command output exceeds its bound",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "runtime_recovery_tests.rs"]
mod tests;
