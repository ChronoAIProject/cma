use crate::manager_ownership::OwnerSession;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use super::OutputRetention;

pub(super) struct BridgeInput {
    pub frame: Vec<u8>,
    pub authority: Option<OwnerSession>,
    pub stop: Option<QueuedStop>,
}

pub(super) struct QueuedStop {
    pub output: Arc<OutputRetention>,
    pub discarded: Arc<AtomicBool>,
}

pub(super) fn spawn_writer(
    mut stdin: impl AsyncWrite + Unpin + Send + 'static,
    mut input: mpsc::Receiver<BridgeInput>,
) {
    tokio::spawn(async move {
        while let Some(input) = input.recv().await {
            let permit = match input.authority {
                Some(authority) => match authority.permit().await {
                    Ok(permit) => Some(permit),
                    // Queued old-epoch bytes are rejected at the actual writer.
                    Err(_) => {
                        if let Some(stop) = &input.stop {
                            stop.discarded.store(true, Ordering::Release);
                        }
                        continue;
                    }
                },
                None => None,
            };
            if let Some(stop) = &input.stop {
                if stop.output.recovery_evidence().await.is_none()
                    || permit
                        .as_ref()
                        .and_then(|permit| permit.deadline())
                        .is_some_and(|deadline| deadline <= tokio::time::Instant::now())
                {
                    stop.discarded.store(true, Ordering::Release);
                    continue;
                }
            }
            let write = async {
                stdin.write_all(&input.frame).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await
            };
            let result = match permit.as_ref().and_then(|permit| permit.deadline()) {
                Some(deadline) => tokio::time::timeout_at(deadline, write)
                    .await
                    .unwrap_or_else(|_| {
                        Err(std::io::Error::other(
                            "transport ownership expired during input",
                        ))
                    }),
                None => write.await,
            };
            if result.is_err() {
                break;
            }
        }
    });
}
