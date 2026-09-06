use std::sync::atomic::Ordering;

use super::{InMemoryHostedBackend, InMemoryRuntimeSnapshot, MemoryRuntime};
use crate::wire::{RequestFrame, WireFailure, WireOperation};
use crate::{AdapterError, AgentSlot, ConversationUpdate, RuntimeHandle};

impl InMemoryHostedBackend {
    pub async fn snapshot(&self, handle: &RuntimeHandle) -> crate::Result<InMemoryRuntimeSnapshot> {
        self.runtimes
            .lock()
            .await
            .get(handle)
            .map(MemoryRuntime::snapshot)
            .ok_or_else(|| AdapterError::Unavailable(format!("unknown runtime {}", handle.0)))
    }

    pub async fn requests(&self) -> Vec<(RuntimeHandle, RequestFrame)> {
        self.requests.lock().await.clone()
    }

    /// Emit one conversation update from an inspectable runtime.
    pub async fn send_agent_update(
        &self,
        handle: &RuntimeHandle,
        agent_id: &AgentSlot,
        update: ConversationUpdate,
    ) -> crate::Result<()> {
        let updates = self
            .runtimes
            .lock()
            .await
            .get(handle)
            .and_then(|runtime| runtime.agents.get(agent_id))
            .filter(|agent| agent.running)
            .map(|agent| agent.updates.clone())
            .ok_or_else(|| AdapterError::Unavailable("agent update channel is not live".into()))?;
        updates
            .send(update)
            .await
            .map_err(|_| AdapterError::Unavailable("agent update receiver ended".into()))
    }

    /// Return one typed runtime failure the next time this operation is sent.
    pub fn fail_next(
        &self,
        operation: WireOperation,
        code: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.failures
            .lock()
            .expect("in-memory failure queue lock")
            .push_back(WireFailure {
                operation,
                code: code.into(),
                detail: detail.into(),
                retryable: false,
            });
    }

    /// Apply one operation but report a lost response to exercise durable replay.
    pub fn lose_next_response(&self, operation: WireOperation) {
        self.lost_responses
            .lock()
            .expect("in-memory lost-response queue lock")
            .push_back(operation);
    }

    /// Make the next successful install reply carry an invalid receipt.
    pub fn corrupt_next_install_receipt(&self) {
        self.corrupt_install_receipt.store(true, Ordering::Relaxed);
    }

    /// Make the next captured archive disagree with its advertised digest.
    pub fn corrupt_next_capture_digest(&self) {
        self.corrupt_capture_digest.store(true, Ordering::Relaxed);
    }
}
