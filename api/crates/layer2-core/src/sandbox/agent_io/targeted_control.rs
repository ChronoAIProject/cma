//! Explicit response controls retain ordinary v1 commands and immutable target
//! evidence separately. Control delivery never proves response cancellation.

mod model;
mod observation;
mod public_admission;
mod reservation;
mod service;

pub use model::{
    AdmittedTargetedControl, TargetedControlBinding, TargetedControlStatusView,
    TargetedControlSubmission, TargetedResponse,
};
pub use observation::{ResponseControlObservation, ResponseControlRuntimeSupport};
pub use public_admission::TargetedControlError;
pub use reservation::TargetedControlReservation;

#[cfg(test)]
mod tests;
