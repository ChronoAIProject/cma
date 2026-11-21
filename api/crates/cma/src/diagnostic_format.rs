//! Keep bounded lifecycle events independent of ambient span formatting.

use tracing::{Event, Subscriber};
use tracing_subscriber::{
    fmt::{format, FmtContext, FormatEvent, FormatFields},
    registry::LookupSpan,
};

const LIFECYCLE_TARGET: &str = "agent_scheduler_failure";

pub(crate) struct PrivacyFormat<F> {
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
        if event.metadata().target() == LIFECYCLE_TARGET {
            self.context_free.format_event(ctx, writer, event)
        } else {
            self.ordinary.format_event(ctx, writer, event)
        }
    }
}

pub(crate) fn text() -> PrivacyFormat<format::Format> {
    // Full text respects the emitter's explicit parent:None. Preserve its
    // timestamps and ordinary event context exactly as before.
    PrivacyFormat {
        ordinary: format::format(),
        context_free: format::format(),
    }
}

#[cfg(test)]
pub(crate) fn json() -> PrivacyFormat<format::Format<format::Json, ()>> {
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
