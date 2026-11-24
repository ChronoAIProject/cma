//! Process-local log correlation. These observations never authorize work.
use sha2::{Digest, Sha256};

use super::{execution_authority, Authority, OwnershipError};
use crate::{AgentId, SandboxId};

#[derive(Clone)]
pub struct ReceiverLogIdentity {
    pub agent_sha256: String,
    pub sandbox_sha256: Option<String>,
    pub instance_sha256: Option<String>,
    pub epoch: Option<u64>,
    pub receiver_sha256: Option<String>,
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// Retain only a hash from an existing Agent read, under that Agent's exact
/// execution authority. No extra IO, durable fields, or raw Sandbox ID storage.
pub fn observe_agent_sandbox(agent_id: &AgentId, sandbox_id: &SandboxId) {
    if let Some(authority) = execution_authority().filter(|a| a.token.agent_id == *agent_id) {
        authority
            .sandbox_sha256
            .get_or_init(|| digest(sandbox_id.as_str()));
    }
}

impl Authority {
    /// Retained after cancellation so maintenance can join the failed receiver
    /// with inspection logs. An unobserved Sandbox remains explicitly unknown.
    pub fn log_identity(&self) -> ReceiverLogIdentity {
        ReceiverLogIdentity {
            agent_sha256: digest(self.token.agent_id.as_str()),
            sandbox_sha256: self.sandbox_sha256.get().cloned(),
            instance_sha256: Some(digest(&self.token.instance_id)),
            epoch: Some(self.token.epoch),
            receiver_sha256: None,
        }
    }
}

impl ReceiverLogIdentity {
    pub fn for_agent(
        agent_id: &AgentId,
        sandbox_id: &SandboxId,
        instance_id: Option<&str>,
    ) -> Self {
        observe_agent_sandbox(agent_id, sandbox_id);
        let authority = execution_authority().filter(|a| a.token.agent_id == *agent_id);
        Self {
            agent_sha256: digest(agent_id.as_str()),
            sandbox_sha256: Some(digest(sandbox_id.as_str())),
            instance_sha256: authority
                .as_ref()
                .map(|a| digest(&a.token.instance_id))
                .or_else(|| instance_id.map(digest)),
            epoch: authority.map(|a| a.token.epoch),
            receiver_sha256: None,
        }
    }

    pub fn with_receiver(mut self, receiver: &str) -> Self {
        self.receiver_sha256 = Some(digest(receiver));
        self
    }

    pub fn warn(&self, stage: &'static str, error: &OwnershipError, message: &'static str) {
        self.warning(Some(stage), None, error, message);
    }

    pub fn warn_phase(&self, phase: &'static str, error: &OwnershipError, message: &'static str) {
        self.warning(None, Some(phase), error, message);
    }

    fn warning(
        &self,
        stage: Option<&'static str>,
        phase: Option<&'static str>,
        error: &OwnershipError,
        message: &'static str,
    ) {
        // Parentless events alone still inherit ambient JSON span fields. A
        // disabled safety root must suppress emission under selective filters.
        let root = tracing::warn_span!(parent: None, "receiver_diagnostic");
        if root.is_disabled() {
            return;
        }
        let _entered = root.enter();
        let error = match error {
            OwnershipError::Unavailable(_) => "ownership unavailable",
            OwnershipError::Invalid(_) => "invalid ownership evidence",
            OwnershipError::Stale => "ownership stale",
            OwnershipError::IncompatibleProtocol => "ownership protocol incompatible",
        };
        tracing::warn!(parent: &root,
            stage, phase,
            agent_sha256 = %self.agent_sha256,
            sandbox_sha256 = self.sandbox_sha256.as_deref().unwrap_or("unknown"),
            instance_sha256 = self.instance_sha256.as_deref().unwrap_or("unknown"),
            receiver_sha256 = self.receiver_sha256.as_deref().unwrap_or("unknown"),
            epoch = self.epoch,
            error,
            "{message}"
        );
    }

    pub fn inspection(
        &self,
        step: crate::sandbox::AgentWorkStep,
        elapsed: std::time::Duration,
        succeeded: bool,
        parent_overlap: bool,
    ) {
        self.inspection_event(
            Some(step),
            elapsed,
            succeeded,
            parent_overlap,
            "Agent runtime inspection completed",
        );
    }

    pub fn inspection_complete(&self, elapsed: std::time::Duration, succeeded: bool) {
        self.inspection_event(
            None,
            elapsed,
            succeeded,
            true,
            "Agent parent reads and runtime inspection completed",
        );
    }

    fn inspection_event(
        &self,
        step: Option<crate::sandbox::AgentWorkStep>,
        elapsed: std::time::Duration,
        succeeded: bool,
        parent_overlap: bool,
        message: &'static str,
    ) {
        let root = tracing::info_span!(parent: None, "runtime_inspection");
        if root.is_disabled() {
            return;
        }
        let _entered = root.enter();
        tracing::info!(parent: &root,
            agent_sha256 = %self.agent_sha256,
            sandbox_sha256 = self.sandbox_sha256.as_deref().unwrap_or("unknown"),
            instance_sha256 = self.instance_sha256.as_deref().unwrap_or("unknown"),
            epoch = self.epoch,
            step = step.as_ref().map(tracing::field::debug),
            elapsed_ms = elapsed.as_millis() as u64,
            succeeded, parent_overlap,
            "{message}"
        );
    }
}

#[cfg(test)]
mod tests;
