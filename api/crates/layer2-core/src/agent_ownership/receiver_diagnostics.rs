use super::super::OwnershipToken;
use layer3_adapter::owner_protocol::{OwnershipFailure, OwnershipFailureReason};
use sha2::{Digest, Sha256};

pub(super) enum Event {
    First(OwnershipFailure),
    Operation {
        stage: &'static str,
        reason: OwnershipFailureReason,
    },
    Recorded,
    Unconfirmed(OwnershipFailureReason),
}

pub(super) fn emit(token: &OwnershipToken, event: Event) {
    // JSON formatters include the current span even on parent:none events.
    // Suppress output if a selective filter disables this field-free root.
    let root =
        tracing::error_span!(target: "receiver_ownership", parent: None, "receiver_ownership");
    if root.is_disabled() {
        return;
    }
    let _entered = root.enter();
    let agent_sha256 = format!("{:x}", Sha256::digest(token.agent_id.as_str().as_bytes()));
    let instance_sha256 = format!("{:x}", Sha256::digest(token.instance_id.as_bytes()));
    let epoch = token.epoch;
    match event {
        Event::First(failure) => tracing::warn!(target: "receiver_ownership", parent: None,
            %agent_sha256, %instance_sha256, epoch,
            phase = failure.phase.as_str(), reason = failure.reason.as_str(),
            http_status = failure.http_status, protocol = failure.protocol, lease_ms = failure.lease_ms,
            "Receiver ownership negotiation failed"),
        Event::Operation { stage, reason } => {
            tracing::warn!(target: "receiver_ownership", parent: None,
            %agent_sha256, %instance_sha256, epoch, stage, reason = reason.as_str(),
            "Receiver ownership operation failed")
        }
        Event::Recorded => tracing::info!(target: "receiver_ownership", parent: None,
            %agent_sha256, %instance_sha256, epoch, "Receiver ownership failure durably recorded"),
        Event::Unconfirmed(reason) => tracing::warn!(target: "receiver_ownership", parent: None,
            %agent_sha256, %instance_sha256, epoch, reason = reason.as_str(),
            "Receiver ownership failure persistence unconfirmed"),
    }
}
