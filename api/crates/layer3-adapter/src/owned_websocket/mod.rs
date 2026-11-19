//! Admission boundaries around the pinned WebSocket protocol implementation.

mod admission_io;
mod header_gate;
mod header_parser;

pub use admission_io::{AdmissionIo, HeaderAdmission};
pub use header_parser::{HeaderParser, Tungstenite024, Tungstenite029};

#[cfg(test)]
mod tests;
