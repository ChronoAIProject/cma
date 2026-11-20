//! Bounded transport for trusted provider maintenance in an owned Pod.
//! The lifecycle adapter authorizes each command; this is not public arbitrary exec.
use std::time::Duration;

use kube::api::AttachParams;

use super::{api_error, rejected, unavailable, AgentSandboxClient};
use crate::hosted::RuntimeRecoveryCommand;
use crate::{Result, RuntimeHandle};

const RECOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const IO_LIMIT: usize = 64 * 1024;
const ARGUMENT_LIMIT: usize = 64 * 1024;

#[path = "runtime_recovery_ingress.rs"]
mod ingress;

impl AgentSandboxClient {
    pub(crate) async fn run_recovery_command(
        &self,
        runtime: &RuntimeHandle,
        command: &RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        ingress::validate_command(command).map_err(ingress::Failure::into_adapter)?;
        // Share one-shot capacity with runtime requests, without an unbounded queue.
        // Hold it through the final identity check; clones share the same budget.
        let _permit = self
            .runtime_capacity
            .acquire_request(super::RuntimeWorkloadClass::Control)
            .await?;
        tokio::time::timeout(RECOVERY_TIMEOUT, async {
            let before = self.pod_binding(runtime).await?;
            ingress::validate_binding(super::parse_handle(runtime)?, &before)
                .map_err(ingress::Failure::into_adapter)?;
            let output = self.inspect_pod(&before.pod_name, command).await?;
            ingress::validate_final_binding(&before, &self.pod_binding(runtime).await?)
                .map_err(ingress::Failure::into_adapter)?;
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
        let stdin = process
            .stdin()
            .ok_or_else(|| unavailable("runtime recovery command stdin missing"))?;
        let stdout = process
            .stdout()
            .ok_or_else(|| unavailable("runtime recovery command stdout missing"))?;
        let status = process
            .take_status()
            .ok_or_else(|| unavailable("runtime recovery command status missing"))?;
        ingress::exchange_io(
            stdin,
            stdout,
            &command.input,
            async {
                status
                    .await
                    .as_ref()
                    .and_then(|status| status.status.as_deref())
                    == Some("Success")
            },
            || async move { process.join().await.is_ok() },
            Vec::new(),
        )
        .await
        .map_err(ingress::Failure::into_adapter)
    }
}

#[cfg(test)]
#[path = "runtime_recovery_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "runtime_recovery_ingress_tests.rs"]
mod ingress_tests;

#[cfg(test)]
use crate::AdapterError;
#[cfg(test)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
