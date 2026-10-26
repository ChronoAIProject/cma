//! Revisions of input before runtime dispatch, preserving immutable admission receipts.
mod model;
mod reservation;
mod service;
pub use model::*;
pub use reservation::QueuedInputReservation;
