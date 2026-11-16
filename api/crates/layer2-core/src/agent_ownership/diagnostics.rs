//! Process-local log correlation. These observations never authorize work.
use sha2::{Digest, Sha256};

use super::{execution_authority, Authority, OwnershipError};
use crate::{AgentId, SandboxId};

pub struct ReceiverLogIdentity {
    pub agent_sha256: String,
    pub sandbox_sha256: Option<String>,
    pub instance_sha256: Option<String>,
    pub epoch: Option<u64>,
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
        }
    }

    pub fn warn(&self, stage: &'static str, error: &OwnershipError, message: &'static str) {
        tracing::warn!(
            stage,
            agent_sha256 = %self.agent_sha256,
            sandbox_sha256 = self.sandbox_sha256.as_deref().unwrap_or("unknown"),
            instance_sha256 = self.instance_sha256.as_deref().unwrap_or("unknown"),
            epoch = self.epoch,
            error = %error,
            "{message}"
        );
    }
}

#[cfg(test)]
mod tests;
