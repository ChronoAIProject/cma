//! Final caller attribution without error payloads or credential identifiers.
use layer2_core::{
    dependency_diagnostics::Failure, diagnostic_context::DiagnosticContext, StoreError,
};

pub(super) fn record(error: &StoreError, failure: Failure) {
    let error_class = match error {
        StoreError::Unavailable(_) => "unavailable",
        StoreError::Malformed(_) => "malformed",
        StoreError::RevisionConflict { .. } => "revision_conflict",
        StoreError::ReferenceUnavailable(_) => "reference_unavailable",
    };
    // Emit in the final consumer's context: a shared verification leader's
    // provider event cannot attribute failure to each admitted caller.
    let context = DiagnosticContext::current();
    let span = tracing::warn_span!(
        target: crate::diagnostics::AUTHENTICATION_TARGET,
        parent: None,
        "authentication_failure"
    );
    // JSON formatters also inspect the entered span. A selectively disabled
    // root would leave a payload-bearing caller span current instead.
    // Shared callsite interest can retain a span handle even when this
    // subscriber statically excludes it. Per-layer `enabled` can still return
    // true, and a coexisting dynamic subscriber can force a fresh event check.
    // Respect this dispatcher's static exclusion before its runtime check.
    // Layers can disagree, so the formatter also omits diagnostic JSON ancestry.
    if span.is_disabled()
        || !span.metadata().is_some_and(|metadata| {
            tracing::dispatcher::get_default(|dispatch| {
                !dispatch.register_callsite(metadata).is_never() && dispatch.enabled(metadata)
            })
        })
    {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(
        target: crate::diagnostics::AUTHENTICATION_TARGET,
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
