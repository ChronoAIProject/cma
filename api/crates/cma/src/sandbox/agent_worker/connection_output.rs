//! Persist quiet retained replay while reconnect and context reload are pending.
use std::future::Future;
use std::time::Duration;

use futures_util::FutureExt as _;
use layer2_core::sandbox::{
    Agent, AgentConversationError, AgentMessageAssembly, AgentUpdateRecorder,
};
use layer2_core::{now_rfc3339, AgentId};
use layer3_adapter::ConversationUpdate;
use tokio::sync::mpsc;

use super::support::{adapter_failure, WorkerFailure};
use super::AgentWorker;

impl AgentWorker {
    pub(super) async fn drain_connection_output<T>(
        &self,
        agent: &Agent,
        operation: impl Future<Output = Result<T, WorkerFailure>>,
        receiver: &mut mpsc::Receiver<ConversationUpdate>,
    ) -> Result<T, WorkerFailure> {
        let lease = agent.work.lease.as_ref().ok_or_else(unavailable)?;
        if lease.holder != self.config.holder || agent.conversation.active_turn.is_some() {
            return Err(unavailable());
        }
        tokio::time::timeout(
            self.config.open_timeout,
            drain(
                operation,
                receiver,
                self.dependencies.updates.as_ref(),
                &agent.id,
                agent
                    .conversation
                    .runtime_generation
                    .ok_or_else(unavailable)?,
                agent
                    .conversation
                    .conversation_id
                    .as_deref()
                    .ok_or_else(unavailable)?,
            ),
        )
        .await
        .map_err(|_| unavailable())?
    }
}

async fn drain<T>(
    operation: impl Future<Output = Result<T, WorkerFailure>>,
    receiver: &mut mpsc::Receiver<ConversationUpdate>,
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    generation: u64,
    conversation: &str,
) -> Result<T, WorkerFailure> {
    tokio::pin!(operation);
    let mut assembly = AgentMessageAssembly::default();
    recorder
        .restore_assembly(agent_id, generation, &mut assembly)
        .await
        .map_err(|_| unavailable())?;
    if !assembly.is_empty() {
        return Err(unavailable());
    }
    loop {
        tokio::select! {
            biased;
            result = &mut operation => return result,
            update = receiver.recv() => {
                let Some(update) = update else {
                    return prefer_failure(operation.as_mut(), unavailable());
                };
                let receipt = match &update {
                    ConversationUpdate::RuntimeOutput { receipt, .. }
                        if receipt.source.native_conversation_id == conversation => Some(receipt),
                    ConversationUpdate::RuntimeOutput { .. } | ConversationUpdate::Lost { .. } => return Err(unavailable()),
                    _ => None,
                };
                let mut completed = None;
                loop {
                    // Confirmation and recording share the Agent writer queue.
                    // Keep driving the opener while this receipt waits for its
                    // transaction, then finish the receipt before handing off.
                    let recorded = {
                        let recording = recorder.record_update(agent_id, generation, &mut assembly, update.clone(), now_rfc3339());
                        tokio::pin!(recording);
                        loop {
                            tokio::select! {
                                result = &mut operation, if completed.is_none() => completed = Some(result),
                                result = &mut recording => break result,
                            }
                        }
                    };
                    match recorded {
                        Ok(_) => {
                            if let Some(receipt) = receipt {
                                if let Err(error) = receipt.acknowledge().await {
                                    return recorded_failure(completed, operation.as_mut(), adapter_failure(error));
                                }
                            }
                            break;
                        }
                        Err(AgentConversationError::Unavailable(_)) => tokio::time::sleep(Duration::from_secs(1)).await,
                        Err(_) => return recorded_failure(completed, operation.as_mut(), unavailable()),
                    }
                }
                if let Some(result) = completed {
                    return result;
                }
            }
        }
    }
}

fn recorded_failure<T>(
    completed: Option<Result<T, WorkerFailure>>,
    operation: impl Future<Output = Result<T, WorkerFailure>>,
    failure: WorkerFailure,
) -> Result<T, WorkerFailure> {
    match completed {
        Some(Err(original)) => Err(original),
        Some(Ok(_)) => Err(failure),
        None => prefer_failure(operation, failure),
    }
}

fn prefer_failure<T>(
    operation: impl Future<Output = Result<T, WorkerFailure>>,
    failure: WorkerFailure,
) -> Result<T, WorkerFailure> {
    match operation.now_or_never() {
        Some(Err(original)) => Err(original),
        _ => Err(failure),
    }
}

fn unavailable() -> WorkerFailure {
    WorkerFailure::retryable(
        "agent_connection_output_unavailable",
        "retained connection output was not durably confirmed",
    )
}

#[cfg(test)]
#[path = "connection_output_tests.rs"]
mod tests;
