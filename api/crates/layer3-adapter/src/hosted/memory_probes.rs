use std::sync::atomic::Ordering;

use super::{InMemoryHostedBackend, InMemoryRuntimeSnapshot, MemoryRuntime};
use crate::wire::{RequestFrame, WireFailure, WireOperation};
use crate::{AdapterError, AgentSlot, ConversationUpdate, RuntimeHandle};

impl InMemoryHostedBackend {
    /// Simulate a provider outage without implying that its runtimes are gone.
    pub fn set_inventory_unavailable(&self, unavailable: bool) {
        self.inventory_unavailable
            .store(unavailable, Ordering::SeqCst);
    }
    /// Simulate a supervisor replacement while retaining provider and disk identity.
    /// Filesystem receipts survive; credentials and live Agent processes do not.
    pub async fn restart_persistent_process(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        let mut runtimes = self.runtimes.lock().await;
        let runtime = runtimes
            .get_mut(handle)
            .ok_or_else(|| AdapterError::Unavailable("runtime is absent".into()))?;
        if runtime
            .request
            .env
            .get("CHRONO_RUNTIME_HOME_IDENTITY")
            .is_none_or(String::is_empty)
        {
            return Err(AdapterError::Rejected(
                "runtime has no persistent home".into(),
            ));
        }
        let incarnation = self.next_handle.fetch_add(1, Ordering::Relaxed) + 1;
        runtime.boot_id = format!("{incarnation:032x}");
        runtime.credential_delivered = false;
        for agent in runtime.agents.values_mut() {
            agent.running = false;
        }
        Ok(())
    }

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
