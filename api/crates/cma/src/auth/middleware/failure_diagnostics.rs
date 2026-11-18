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
    // Shared callsite interest can retain a span handle even when the current
    // dispatcher excludes it, so check that dispatcher too. Different output
    // layers can still disagree: the diagnostic formatter separately omits
    // JSON span context for this event's explicit target.
    if span.is_disabled()
        || !span.metadata().is_some_and(|metadata| {
            tracing::dispatcher::get_default(|dispatch| dispatch.enabled(metadata))
        })
    {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(
        target: crate::diagnostic_format::AUTHENTICATION_TARGET,
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
