use std::future::Future;
use std::time::Duration;

use layer2_core::sandbox::store::AgentReopenGuard;
use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder};
use layer2_core::{now_rfc3339, AgentId};
use layer3_adapter::{AdapterError, ConversationUpdate};
use tokio::sync::mpsc;

/// Drain before open returns; ACK ownership passes only after a fenced commit.
pub(super) async fn drain_while<T>(
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    guard: &AgentReopenGuard,
) -> layer3_adapter::Result<T> {
    let expires = guard
        .sandbox
        .lease
        .as_ref()
        .and_then(|lease| layer2_core::seconds_from_rfc3339(&lease.expires_at))
        .ok_or_else(|| {
            AdapterError::Rejected("reopen output requires a finite lifecycle lease".into())
        })?;
    let now = layer2_core::seconds_from_rfc3339(&now_rfc3339())
        .expect("clock produces a valid timestamp");
    let remaining = expires.saturating_sub(now);
    tokio::time::timeout(
        Duration::from_secs(remaining),
        drain(operation, receiver, recorder, agent_id, guard),
    )
    .await
    .map_err(|_| AdapterError::Unavailable("reopen output lifecycle lease expired".into()))?
}

async fn drain<T>(
    operation: impl Future<Output = layer3_adapter::Result<T>>,
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    guard: &AgentReopenGuard,
) -> layer3_adapter::Result<T> {
    tokio::pin!(operation);
    let mut assembly = AgentMessageAssembly::default();
    loop {
        tokio::select! {
            result = &mut operation => return result,
            update = receiver.recv() => {
                let update = update.ok_or_else(|| AdapterError::Unavailable("reopen output receiver ended".into()))?;
                let receipt = match &update {
                    ConversationUpdate::RuntimeOutput { receipt, .. } => Some(receipt),
                    ConversationUpdate::Lost { .. } => return Err(AdapterError::Unavailable("reopen output contains a gap".into())),
                    _ => None,
                };
                loop {
                    match recorder.record_reopen_update(agent_id, guard, &mut assembly, update.clone(), now_rfc3339()).await {
                        Ok(_) => {
                            if let Some(receipt) = receipt { receipt.acknowledge().await?; }
                            break;
                        }
                        Err(AgentConversationError::Unavailable(_)) => tokio::time::sleep(Duration::from_secs(1)).await,
                        Err(error) => return Err(AdapterError::Unavailable(format!("durable reopen output was refused: {error}"))),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "reopen_output_tests.rs"]
mod tests;
