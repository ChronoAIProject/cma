//! Admission boundaries around the pinned WebSocket protocol implementation.

mod admission_io;
mod header_gate;

pub use admission_io::{AdmissionIo, HeaderAdmission};

#[cfg(test)]
mod tests;
