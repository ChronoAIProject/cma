//! Closed fields already understood by the measurement collectors. No payloads.
use crate::auth::verification_error::{ProviderVerificationError, VerificationFailure};
use layer2_core::{diagnostic_context::DiagnosticContext, resource_access::MembershipError};

#[derive(Clone, Copy)]
pub(super) enum Operation {
    Choices,
    Membership,
}

#[derive(Clone, Copy)]
pub(super) enum Phase {
    Configuration,
    Restriction,
    Metadata,
    Verification,
    Permissions,
    Subject,
    Context,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::Configuration => "principal_configuration",
            Self::Restriction => "principal_restriction",
            Self::Metadata => "principal_metadata",
            Self::Verification => "principal_verification",
            Self::Permissions => "principal_permissions",
            Self::Subject => "principal_subject",
            Self::Context => "principal_context",
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Configuration => "missing_configuration",
            Self::Restriction => "restricted_credential",
            Self::Metadata | Self::Verification => "unknown",
            Self::Permissions => "inactive_permissions",
            Self::Subject => "subject_mismatch",
            Self::Context => "invalid_context",
        }
    }
}

pub(super) fn record(
    operation: Operation,
    phase: Phase,
    error: &MembershipError,
    verification: Option<&VerificationFailure>,
) {
    let operation = match operation {
        Operation::Choices => "choices",
        Operation::Membership => "membership",
    };
    let outcome = match error {
        MembershipError::Unavailable => "unavailable",
        MembershipError::NotMember => "not_member",
        MembershipError::InvalidEvidence => "invalid_evidence",
    };
    let result = verification.map(|failure| match &failure.error {
        ProviderVerificationError::Rejected(_) => "rejected",
        ProviderVerificationError::Unavailable(_) => "unavailable",
        ProviderVerificationError::Transient(_) => "transient",
        ProviderVerificationError::RateLimited(_) => "rate_limited",
    });
    let reason = verification.map_or_else(|| phase.reason(), VerificationFailure::reason);
    let status = verification.and_then(VerificationFailure::status);
    let context = DiagnosticContext::current();
    let span = tracing::warn_span!(parent: None, "organization_principal");
    // Callsite interest is shared across subscribers. Check this dispatcher as
    // well: a concurrently enabled subscriber may leave a filtered root with
    // an ID, letting JSON formatting fall back to a sensitive entered ancestor.
    if span.is_disabled()
        || !span.metadata().is_some_and(|metadata| {
            tracing::dispatcher::get_default(|dispatch| dispatch.enabled(metadata))
        })
    {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(parent: &span,
        event = "organization_principal_failure",
        request_hash = context.request_hash.as_ref(),
        operation, phase = phase.label(), reason, outcome, result, status,
        "Organization principal verification failed"
    );
}
