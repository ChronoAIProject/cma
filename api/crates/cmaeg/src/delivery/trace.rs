//! Interpret existing delivery receipts without changing send or retry policy.
use super::intents::{DeliveryOutcome, IntentReceipt};
use crate::telemetry::turn_trace::{BoundOperation, Outcome, Stage};
use std::future::Future;

pub(super) mod native;
mod parts;

pub(super) fn observe<T>(
    parent: &tracing::Span,
    platform: bool,
    future: impl Future<Output = T>,
    classify: fn(&T) -> Outcome,
) -> impl Future<Output = T> {
    let trace = platform
        .then(|| BoundOperation::child_of(parent, Stage::Platform))
        .flatten();
    match trace {
        // Native media dispatch has a large future. Keep one inline legacy
        // future when OFF and allocate the enabled wrapper only after gating.
        Some(trace) => {
            futures_util::future::Either::Left(parts::observe(trace, Box::pin(future), classify))
        }
        None => futures_util::future::Either::Right(future),
    }
}

async fn observed<T>(
    trace: Option<BoundOperation>,
    future: impl Future<Output = T>,
    classify: impl Fn(&T) -> Outcome,
) -> T {
    match trace {
        Some(trace) => {
            let result = trace.within(future).await;
            trace.finish(classify(&result));
            result
        }
        None => future.await,
    }
}

pub(super) fn receipt(receipt: &IntentReceipt) -> Outcome {
    match &receipt.delivery {
        DeliveryOutcome::Confirmed { .. } => Outcome::Confirmed,
        _ if !receipt.message_parts.is_empty() => Outcome::Partial,
        DeliveryOutcome::Unknown { reason }
            if matches!(
                reason.as_str(),
                "partial_delivery"
                    | "native_whatsapp_partial_delivery"
                    | "native_lark_partial_delivery"
            ) =>
        {
            Outcome::Partial
        }
        DeliveryOutcome::Unknown { .. } => Outcome::Unknown,
        DeliveryOutcome::Rejected { .. }
        | DeliveryOutcome::RateLimited { .. }
        | DeliveryOutcome::Unsupported => Outcome::Rejected,
    }
}

#[cfg(test)]
mod tests;
