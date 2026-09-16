//! Bounded operational observations. Durable records remain the recovery authority.
use std::time::Duration;

use layer2_core::bridge::outbox::{DeliveryFailure, DeliveryOutcome};

/// Fixed values only: never use Bridge, sender or resource identifiers as labels.
#[derive(Clone, Copy)]
pub(crate) enum WorkKind {
    Inbox,
    Allocation,
    Renewal,
    ChildCleanup,
    WorkspaceCleanup,
    #[cfg(test)]
    Fixture,
}

impl WorkKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Allocation => "allocation",
            Self::Renewal => "renewal",
            Self::ChildCleanup => "child_cleanup",
            Self::WorkspaceCleanup => "workspace_cleanup",
            #[cfg(test)]
            Self::Fixture => "fixture",
        }
    }
}

pub(crate) fn elapsed_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn delivery_labels(outcome: &DeliveryOutcome) -> (&'static str, &'static str) {
    match outcome {
        DeliveryOutcome::Confirmed(_) => ("confirmed", "none"),
        DeliveryOutcome::DefinitiveRejection(reason) => ("rejected", failure(*reason)),
        DeliveryOutcome::RetryableNotSent { reason, .. } => ("not_sent", failure(*reason)),
        DeliveryOutcome::Unknown(reason) => ("unknown", failure(*reason)),
        DeliveryOutcome::RateLimitedUnknown { .. } => ("unknown", "rate_limited"),
    }
}

fn failure(reason: DeliveryFailure) -> &'static str {
    match reason {
        DeliveryFailure::CredentialExpired => "credential_expired",
        DeliveryFailure::CredentialRevoked => "credential_revoked",
        DeliveryFailure::BotInactive => "bot_inactive",
        DeliveryFailure::RouteUnavailable => "route_unavailable",
        DeliveryFailure::CapabilityUnsupported => "capability_unsupported",
        DeliveryFailure::RateLimited => "rate_limited",
        DeliveryFailure::RequestRejected => "request_rejected",
        DeliveryFailure::ConnectUnavailable => "connect_unavailable",
        DeliveryFailure::ProviderFailure => "provider_failure",
        DeliveryFailure::ResponseLost => "response_lost",
        DeliveryFailure::WorkerLost => "worker_lost",
        DeliveryFailure::ReceiptInvalid => "receipt_invalid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::bridge::outbox::{DeliveryReceipt, NotSentEvidence};

    #[test]
    fn delivery_metrics_preserve_ambiguity_without_exporting_provider_identity() {
        let confirmed = DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "private-relay-id".into(),
            platform_message_id: Some("private-chat-message".into()),
        });
        assert_eq!(delivery_labels(&confirmed), ("confirmed", "none"));
        assert_eq!(
            delivery_labels(&DeliveryOutcome::RateLimitedUnknown { not_before_ms: 100 }),
            ("unknown", "rate_limited")
        );
        assert_eq!(
            delivery_labels(&DeliveryOutcome::RetryableNotSent {
                reason: DeliveryFailure::RateLimited,
                retry_at_ms: 100,
                evidence: NotSentEvidence::NoRequestBytesWritten,
            }),
            ("not_sent", "rate_limited")
        );
        assert_eq!(
            delivery_labels(&DeliveryOutcome::Unknown(DeliveryFailure::ResponseLost)),
            ("unknown", "response_lost")
        );
    }
}
