//! Final caller attribution without error payloads or credential identifiers.
use layer2_core::{
    dependency_diagnostics::Failure, diagnostic_context::DiagnosticContext, StoreError,
};

pub(super) fn record(error: &StoreError, failure: Failure) {
    let error_class = match error {
        StoreError::Unavailable(_) => "unavailable",
        StoreError::Malformed(_) => "malformed",
        StoreError::RevisionConflict { .. } => "revision_conflict",
    };
    // Emit in the final consumer's context: a shared verification leader's
    // provider event cannot attribute failure to each admitted caller.
    let context = DiagnosticContext::current();
    let span = tracing::warn_span!(parent: None, "authentication_failure");
    // JSON formatters also inspect the entered span. A selectively disabled
    // root would leave a payload-bearing caller span current instead.
    if span.is_disabled() {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(
        parent: &span,
        event = "authentication_failure",
        request_hash = context.request_hash.as_ref(),
        auth_stage = "request_resolution",
        error_class,
        dependency = failure.dependency.label(),
        reason = failure.reason.label(),
        "Request authentication failed"
    );
}

#[cfg(test)]
mod tests;
