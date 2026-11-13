use std::sync::{Arc, Weak};
use std::time::Duration;

use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder};
use layer2_core::{now_rfc3339, AgentId};
use layer3_adapter::ConversationUpdate;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use super::{LiveAgentAttachmentIdentity, LiveAgentRegistry};

const UPDATE_RETRY_DELAY: Duration = Duration::from_secs(1);

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_update_task(
    registry: Weak<LiveAgentRegistry>,
    activated: oneshot::Receiver<LiveAgentAttachmentIdentity>,
    mut updates: mpsc::Receiver<ConversationUpdate>,
    recorder: Arc<dyn AgentUpdateRecorder>,
) -> AbortHandle {
    let task = tokio::spawn(async move {
        let Ok(attachment) = activated.await else {
            return;
        };
        let mut assembly = AgentMessageAssembly::default();
        if restore_assembly(
            recorder.as_ref(),
            &attachment.agent_id,
            attachment.runtime_generation,
            &mut assembly,
        )
        .await
        .is_err()
        {
            if let Some(registry) = registry.upgrade() {
                registry.connection_ended(attachment).await;
            }
            return;
        }
        let _budget_monitor =
            super::budget::monitor(registry.clone(), attachment.clone(), recorder.clone());
        while let Some(update) = updates.recv().await {
            if record_update(
                recorder.as_ref(),
                &attachment.agent_id,
                attachment.runtime_generation,
                &mut assembly,
                update,
            )
            .await
            .is_err()
            {
                break;
            }
        }
        if let Some(registry) = registry.upgrade() {
            registry.connection_ended(attachment).await;
        }
    });
    task.abort_handle()
}

async fn restore_assembly(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut AgentMessageAssembly,
) -> Result<(), AgentConversationError> {
    loop {
        match recorder
            .restore_assembly(agent_id, runtime_generation, assembly)
            .await
        {
            Ok(()) => return Ok(()),
            Err(AgentConversationError::Unavailable(detail)) => {
                tracing::warn!(agent_id = %agent_id, %detail, "agent checkpoint restoration is unavailable");
                tokio::time::sleep(UPDATE_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::error!(agent_id = %agent_id, %error, "agent checkpoint restoration stopped");
                return Err(error);
            }
        }
    }
}

pub(crate) async fn record_update(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut AgentMessageAssembly,
    update: ConversationUpdate,
) -> Result<(), AgentConversationError> {
    let receipt = match &update {
        ConversationUpdate::RuntimeOutput { receipt, .. } => Some(receipt),
        _ => None,
    };
    loop {
        let attempt = recorder.record_update(
            agent_id,
            runtime_generation,
            assembly,
            update.clone(),
            now_rfc3339(),
        );
        let result = if receipt.is_some() {
            crate::telemetry::agent_output::observe(attempt).await
        } else {
            attempt.await
        };
        match result {
            Ok(outcome) => {
                if outcome.appended.iter().any(|entry| matches!(&entry.kind,
                    layer2_core::sandbox::AgentLogKind::TurnFailed { failure, .. }
                        if failure.class == layer2_core::sandbox::store::AGENT_LOG_CAPACITY_FAILURE
                )) {
                    // The task is durably failed, but its refused output has not
                    // been recorded. Leave the native journal receipt unacked.
                    return Err(AgentConversationError::CapacityExceeded);
                }
                if let Some(receipt) = receipt {
                    receipt
                        .acknowledge()
                        .await
                        .map_err(|error| AgentConversationError::Unavailable(error.to_string()))?;
                }
                return Ok(());
            }
            Err(AgentConversationError::Unavailable(detail)) => {
                tracing::warn!(agent_id = %agent_id, %detail, "agent update persistence is unavailable");
                tokio::time::sleep(UPDATE_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::error!(agent_id = %agent_id, %error, "agent update recorder stopped");
                return Err(error);
            }
        }
    }
}
