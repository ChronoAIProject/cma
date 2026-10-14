//! Fixed stdio relay inside the owned runtime container; credentials use stdin only.
use super::*;
use kube::api::AttachParams;
use tokio::io::AsyncReadExt;

pub(super) const READY: &[u8] = b"CMA-EXEC-1\n";
pub(super) const RELAY: &str = include_str!("runtime_exec_relay.py");

pub(super) async fn open(
    client: &AgentSandboxClient,
    expected: &PodBinding,
    permit: OwnedSemaphorePermit,
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
            ["/usr/bin/python3", "-I", "-u", "-c", RELAY],
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
    let mut ready = [0; READY.len()];
    stream
        .read_exact(&mut ready)
        .await
        .map_err(|_| unavailable("Kubernetes runtime relay did not become ready"))?;
    if ready != READY {
        return Err(rejected("Kubernetes runtime relay readiness is invalid"));
    }
    Ok(stream)
}

#[cfg(test)]
#[path = "runtime_exec_tests.rs"]
mod tests;
