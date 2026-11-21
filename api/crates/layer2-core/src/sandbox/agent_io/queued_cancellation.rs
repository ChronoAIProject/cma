//! Human/API cancellation before a response has any durable dispatch binding.
//! Cancellation receipts use the existing command record plus an additive sidecar.

mod model;
mod observation;
pub(super) mod reservation;
mod service;

pub use model::{
    AdmittedQueuedCancellation, QueuedCancellationBinding, QueuedCancellationObservation,
    QueuedCancellationOutcome, QueuedCancellationStatusView, QueuedCancellationSubmission,
    QueuedCancellationTarget,
};
pub use reservation::{queued_cancellation_eligible, QueuedCancellationReservation};
pub use service::QueuedCancellationError;
