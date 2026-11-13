//! Summarize observed sends without changing the caller's delivery receipt.
use crate::telemetry::turn_trace::{BoundOperation, Outcome};
use std::{cell::Cell, future::Future};

tokio::task_local! {
    static PARTS: Cell<(bool, bool)>;
}

pub(super) fn record(outcome: Outcome) {
    let _ = PARTS.try_with(|parts| {
        let (confirmed, uncertain) = parts.get();
        parts.set((
            confirmed || matches!(outcome, Outcome::Confirmed),
            uncertain
                || matches!(
                    outcome,
                    Outcome::Unknown | Outcome::Rejected | Outcome::Partial
                ),
        ));
    });
}

pub(super) async fn observe<T>(
    trace: BoundOperation,
    future: impl Future<Output = T>,
    classify: fn(&T) -> Outcome,
) -> T {
    PARTS
        .scope(Cell::new((false, false)), async {
            let result = trace.within(future).await;
            let outcome = classify(&result);
            // A successful existing retry can follow a rate-limit rejection.
            // The final complete receipt remains authoritative in that case.
            let outcome = if !matches!(outcome, Outcome::Confirmed)
                && PARTS.with(|parts| parts.get()) == (true, true)
            {
                Outcome::Partial
            } else {
                outcome
            };
            trace.finish(outcome);
            result
        })
        .await
}
