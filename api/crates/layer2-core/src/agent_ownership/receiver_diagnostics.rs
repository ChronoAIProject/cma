use super::{Authority, OwnershipError, OwnershipToken};
use crate::AgentId;
use layer3_adapter::owner_protocol::{OwnershipFailure, OwnershipFailureReason};
use sha2::{Digest, Sha256};

pub const TARGET: &str = "receiver_ownership";

pub(super) enum Event {
    First(OwnershipFailure),
    Operation {
        stage: &'static str,
        reason: OwnershipFailureReason,
    },
    Recorded,
    Unconfirmed(OwnershipFailureReason),
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// Retain lineage from an existing Agent read under its exact execution authority.
pub fn observe_agent_sandbox(agent: &AgentId, sandbox: &crate::SandboxId) {
    if let Some(authority) = super::execution_authority().filter(|a| a.token.agent_id == *agent) {
        authority.observe_diagnostic_sandbox(sandbox.as_str());
    }
}

pub struct Correlation {
    pub agent_sha256: String,
    pub sandbox_sha256: Option<String>,
    pub instance_sha256: String,
    pub receiver_sha256: Option<String>,
    pub epoch: Option<u64>,
}

impl Correlation {
    pub fn new(agent: &AgentId, sandbox: Option<&str>, instance: &str) -> Self {
        Self {
            agent_sha256: digest(agent.as_str()),
            sandbox_sha256: sandbox.map(digest),
            instance_sha256: digest(instance),
            receiver_sha256: None,
            epoch: None,
        }
    }
}

impl Authority {
    /// Retain only hashed diagnostic lineage on this exact owner incarnation.
    pub fn observe_diagnostic_sandbox(&self, sandbox_id: &str) {
        self.sandbox_sha256.get_or_init(|| digest(sandbox_id));
    }

    pub fn receiver_correlation(&self) -> Correlation {
        let receivers = self.receivers.lock().unwrap();
        let mut identities = receivers.keys();
        let identity = identities.next().filter(|_| identities.next().is_none());
        self.correlation_for_receiver(identity.map(String::as_str))
    }

    fn correlation_for_receiver(&self, receiver: Option<&str>) -> Correlation {
        let mut fields = Correlation::new(&self.token.agent_id, None, &self.token.instance_id);
        fields.sandbox_sha256 = self.sandbox_sha256.get().cloned();
        fields.receiver_sha256 = receiver.map(digest);
        fields.epoch = Some(self.token.epoch);
        fields
    }

    pub fn log_receiver_handoff_failure(
        &self,
        stage: &'static str,
        error: &OwnershipError,
        message: &'static str,
    ) {
        let fields = self.receiver_correlation();
        with_root(|| {
            tracing::warn!(target: TARGET, parent: None,
            agent_sha256 = %fields.agent_sha256, sandbox_sha256 = fields.sandbox_sha256.as_deref(),
            instance_sha256 = %fields.instance_sha256, receiver_sha256 = fields.receiver_sha256.as_deref(),
            epoch = self.token.epoch, stage, reason = super::transport::failure_reason(error).as_str(),
            message)
        });
    }

    pub(super) fn log_receiver_cleanup_failure(
        &self,
        receiver: &str,
        after_local_drain: bool,
        error: &OwnershipError,
    ) {
        let fields = self.correlation_for_receiver(Some(receiver));
        with_root(|| {
            tracing::warn!(target: TARGET, parent: None,
            agent_sha256 = %fields.agent_sha256, sandbox_sha256 = fields.sandbox_sha256.as_deref(),
            instance_sha256 = %fields.instance_sha256, receiver_sha256 = fields.receiver_sha256.as_deref(),
            epoch = self.token.epoch, stage = "receiver_revocation", after_local_drain,
            reason = super::transport::failure_reason(error).as_str(),
            "Receiver revocation could not be confirmed")
        });
    }
}

pub fn with_root(emit: impl FnOnce()) {
    // JSON formatters include the current span even on parent:none events.
    // Suppress output if a selective filter disables this field-free root.
    let root = tracing::error_span!(target: TARGET, parent: None, "receiver_ownership");
    if root.is_disabled() {
        return;
    }
    let _entered = root.enter();
    emit();
}

pub(super) fn emit(token: &OwnershipToken, fields: &Correlation, event: Event) {
    let agent_sha256 = &fields.agent_sha256;
    let instance_sha256 = &fields.instance_sha256;
    let sandbox_sha256 = fields.sandbox_sha256.as_deref();
    let receiver_sha256 = fields.receiver_sha256.as_deref();
    let epoch = token.epoch;
    with_root(|| match event {
        Event::First(failure) => tracing::warn!(target: TARGET, parent: None,
            %agent_sha256, %instance_sha256, sandbox_sha256, receiver_sha256, epoch,
            phase = failure.phase.as_str(), reason = failure.reason.as_str(),
            http_status = failure.http_status, protocol = failure.protocol, lease_ms = failure.lease_ms,
            "Receiver ownership negotiation failed"),
        Event::Operation { stage, reason } => {
            tracing::warn!(target: TARGET, parent: None,
            %agent_sha256, %instance_sha256, sandbox_sha256, receiver_sha256, epoch, stage, reason = reason.as_str(),
            "Receiver ownership operation failed")
        }
        Event::Recorded => tracing::info!(target: TARGET, parent: None,
            %agent_sha256, %instance_sha256, sandbox_sha256, receiver_sha256, epoch, "Receiver ownership failure durably recorded"),
        Event::Unconfirmed(reason) => tracing::warn!(target: TARGET, parent: None,
            %agent_sha256, %instance_sha256, sandbox_sha256, receiver_sha256, epoch, reason = reason.as_str(),
            "Receiver ownership failure persistence unconfirmed"),
    });
}
