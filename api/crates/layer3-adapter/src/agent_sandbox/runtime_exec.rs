//! Fixed stdio relay inside the owned runtime container; credentials use stdin only.
use super::*;
use kube::api::AttachParams;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::Instant;

pub(super) const READY: &[u8] = b"CMA-EXEC-1\n";
#[cfg(test)]
pub(super) const RELAY: &str = crate::runtime_control_relay::SCRIPT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelayReadinessFailure {
    Unreadable,
    Invalid,
}

async fn read_ready(
    stream: &mut (impl AsyncRead + Unpin),
) -> std::result::Result<(), RelayReadinessFailure> {
    let mut bytes = [0; READY.len()];
    let mut received = 0;
    while received < bytes.len() {
        let count = stream
            .read(&mut bytes[received..])
            .await
            .map_err(|_| RelayReadinessFailure::Unreadable)?;
        if count == 0 {
            return Err(RelayReadinessFailure::Unreadable);
        }
        received += count;
        if bytes[..received] != READY[..received] {
            return Err(RelayReadinessFailure::Invalid);
        }
    }
    Ok(())
}

pub(super) async fn open(
    client: &AgentSandboxClient,
    expected: &PodBinding,
    permit: Option<RuntimePermit>,
    deadline: Instant,
) -> Result<RuntimeStream> {
    let mut parameters = AttachParams::default()
        .container("runtime")
        .stdin(true)
        .stdout(true)
        .stderr(false)
        .tty(false);
    parameters.max_stdin_buf_size = Some(64 * 1024);
    parameters.max_stdout_buf_size = Some(64 * 1024);
    let mut process = client
        .pods
        .exec(
            &expected.pod_name,
            crate::runtime_control_relay::command(),
            &parameters,
        )
        .await
        .map_err(stream_error)?;
    let stdin = process
        .stdin()
        .ok_or_else(|| unavailable("Kubernetes runtime stdin is absent"))?;
    let stdout = process
        .stdout()
        .ok_or_else(|| unavailable("Kubernetes runtime stdout is absent"))?;
    let status = process
        .take_status()
        .ok_or_else(|| unavailable("Kubernetes runtime status is absent"))?;
    // AttachedProcess aborts on drop, including cancellation before readiness/fencing.
    let mut stream = RuntimeStream {
        stream: Box::new(tokio::io::join(stdout, stdin)),
        owner: StreamOwner::Exec(process),
        terminal: Some(Box::pin(async move {
            status.await.as_ref().and_then(|s| s.status.as_deref()) == Some("Success")
        })),
        closed: false,
        failed: false,
        _permit: permit,
    };
    match read_ready(&mut stream).await {
        Ok(()) => super::super::relay_readiness_retry::marker_ready(),
        Err(RelayReadinessFailure::Unreadable) => {
            super::super::relay_readiness_retry::marker_failed(deadline);
            return Err(unavailable("Kubernetes runtime relay did not become ready"));
        }
        Err(RelayReadinessFailure::Invalid) => {
            super::super::relay_readiness_retry::marker_invalid();
            return Err(rejected("Kubernetes runtime relay readiness is invalid"));
        }
    }
    Ok(stream)
}

#[cfg(test)]
#[path = "runtime_exec_tests.rs"]
mod tests;
