use tokio::io::{AsyncReadExt as _, BufReader};

#[path = "bridge_native_state.rs"]
mod state;
pub(super) use state::NativeObservation;

pub(super) fn spawn_diagnostics(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut stderr = BufReader::new(stderr);
        let mut buffer = [0_u8; 8 * 1024];
        while let Ok(read) = stderr.read(&mut buffer).await {
            if read == 0 {
                break;
            }
            tracing::debug!(bytes = read, "agent bridge wrote diagnostics");
        }
    });
}

pub(super) fn spawn_reaper(
    mut child: tokio::process::Child,
    managed: crate::writer_fence::ManagedChild,
    ended: tokio::sync::watch::Sender<bool>,
) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::info!(%status, "agent bridge exited"),
            Err(error) => tracing::warn!(%error, "agent bridge wait failed"),
        }
        drop(managed);
        ended.send_replace(true);
    });
}
