//! Preserve the failing boundary without retaining provider or caller payloads.
use std::time::Instant;

use layer2_core::resource_access::MembershipError;

use layer2_core::resource_access::organization_diagnostics::Failure;
pub(super) use layer2_core::resource_access::organization_diagnostics::{Operation, Reason, Stage};

pub(super) fn record(
    operation: Operation,
    stage: Stage,
    reason: Reason,
    status: Option<u16>,
    started: Instant,
    error: &MembershipError,
) {
    Failure {
        operation,
        stage,
        reason,
        status,
    }
    .record();
    let disposition = match error {
        MembershipError::NotMember => "not_member",
        MembershipError::Unavailable => "unavailable",
        MembershipError::InvalidEvidence => "invalid_evidence",
    };
    let context = layer2_core::diagnostic_context::DiagnosticContext::current();
    // A root span also keeps JSON formatters from attaching sensitive ancestors.
    let span = tracing::warn_span!(parent: None, "organization_authority");
    let _entered = span.enter();
    tracing::warn!(
        parent: &span,
        event = "organization_authority_failure",
        request_hash = context.request_hash.as_ref(),
        operation = operation.label(),
        stage = stage.label(),
        reason = reason.label(),
        status,
        elapsed_ms = started.elapsed().as_millis() as u64,
        disposition,
        "Organization authority lookup failed"
    );
}
