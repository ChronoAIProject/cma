use std::sync::{Arc, Weak};
use std::time::Duration;

use layer2_core::sandbox::{AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder};
use layer2_core::{now_rfc3339, AgentId};
use layer3_adapter::ConversationUpdate;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use super::LiveAgentRegistry;

const UPDATE_RETRY_DELAY: Duration = Duration::from_secs(1);

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_update_task(
    registry: Weak<LiveAgentRegistry>,
    agent_id: AgentId,
    runtime_generation: u64,
    nonce: u64,
    activated: oneshot::Receiver<()>,
    mut updates: mpsc::Receiver<ConversationUpdate>,
    recorder: Arc<dyn AgentUpdateRecorder>,
) -> AbortHandle {
    let task_agent_id = agent_id.clone();
    let task = tokio::spawn(async move {
        if activated.await.is_err() {
            return;
        }
        let mut assembly = AgentMessageAssembly::default();
        while let Some(update) = updates.recv().await {
            if record_update(
                recorder.as_ref(),
                &task_agent_id,
                runtime_generation,
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
            registry.remove_connection(&task_agent_id, runtime_generation, nonce);
        }
    });
    task.abort_handle()
}

async fn record_update(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut AgentMessageAssembly,
    update: ConversationUpdate,
) -> Result<(), AgentConversationError> {
    loop {
        match recorder
            .record_update(
                agent_id,
                runtime_generation,
                assembly,
                update.clone(),
                now_rfc3339(),
            )
            .await
        {
            Ok(_) => return Ok(()),
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
