//! Keep bounded diagnostic events independent of ambient span formatting.

use tracing::{Event, Subscriber};
use tracing_subscriber::{
    fmt::{format, FmtContext, FormatEvent, FormatFields},
    registry::LookupSpan,
};

const LIFECYCLE_TARGET: &str = "agent_scheduler_failure";
pub use crate::diagnostics::{
    AUTHENTICATION_TARGET, ORGANIZATION_PRINCIPAL_TARGET, PROVIDER_IDENTITY_TARGET,
};

pub const OPENING_DECISION_TARGET: &str = "cma::store::agent_opening_reads";
pub const OPENING_DECISION_ROOT: &str = "agent_opening_decision";

pub struct PrivacyFormat<F> {
    ordinary: F,
    context_free: F,
}

impl<S, N, F> FormatEvent<S, N> for PrivacyFormat<F>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let target = event.metadata().target();
        let root = match target {
            AUTHENTICATION_TARGET => Some("authentication_failure"),
            PROVIDER_IDENTITY_TARGET => Some("identity_verification"),
            ORGANIZATION_PRINCIPAL_TARGET => Some("organization_principal"),
            OPENING_DECISION_TARGET => Some(OPENING_DECISION_ROOT),
            _ => None,
        };
        if let Some(root) = root {
            // A registered root can be hidden by this writer's own filter.
            // Resolve the explicit parent in the context of this output.
            let has_root = event
                .parent()
                .and_then(|id| ctx.span(id))
                .is_some_and(|span| {
                    span.name() == root
                        && span.metadata().target() == target
                        && span.metadata().fields().is_empty()
                        && span.parent().is_none()
                });
            if !has_root {
                return Ok(());
            }
        }
        if target == LIFECYCLE_TARGET
            || target == layer2_core::agent_ownership::receiver_diagnostics::TARGET
            || root.is_some()
        {
            self.context_free.format_event(ctx, writer, event)
        } else {
            self.ordinary.format_event(ctx, writer, event)
        }
    }
}

pub fn text() -> PrivacyFormat<format::Format> {
    // Full text respects the emitter's explicit parent:None. Preserve its
    // timestamps and ordinary event context exactly as before.
    PrivacyFormat {
        ordinary: format::format(),
        context_free: format::format(),
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn json() -> PrivacyFormat<format::Format<format::Json, ()>> {
    // A registry span can exist while a formatting layer rejects it. The
    // standard JSON formatter then falls back to an older private span, even
    // with parent:None. Privacy must be enforced at the formatting boundary.
    PrivacyFormat {
        ordinary: format::format().json().without_time(),
        context_free: format::format()
            .json()
            .without_time()
            .with_current_span(false)
            .with_span_list(false),
    }
}
