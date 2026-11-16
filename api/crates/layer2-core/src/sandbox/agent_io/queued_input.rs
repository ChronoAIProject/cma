//! Revisions of input before runtime dispatch, preserving immutable admission receipts.
mod model;
mod recovery;
mod reservation;
mod service;
pub use model::*;
pub use reservation::QueuedInputReservation;
