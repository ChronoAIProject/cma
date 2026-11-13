use crate::manager_ownership::OwnerSession;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub(super) struct BridgeInput {
    pub frame: Vec<u8>,
    pub authority: Option<OwnerSession>,
}

pub(super) fn spawn_writer(
    mut stdin: tokio::process::ChildStdin,
    mut input: mpsc::Receiver<BridgeInput>,
) {
    tokio::spawn(async move {
        while let Some(input) = input.recv().await {
            let permit = match input.authority {
                Some(authority) => match authority.permit().await {
                    Ok(permit) => Some(permit),
                    // Queued old-epoch bytes are rejected at the actual writer.
                    Err(_) => continue,
                },
                None => None,
            };
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
