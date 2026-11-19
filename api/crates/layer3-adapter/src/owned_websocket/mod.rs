//! Admission boundaries around the pinned WebSocket protocol implementation.

mod admission_io;
mod header_gate;
mod header_parser;
mod message_owner;

pub use admission_io::{AdmissionIo, HeaderAdmission};
pub use header_parser::{HeaderParser, Tungstenite024, Tungstenite029};
pub use message_owner::{
    copy_conversation_024, copy_conversation_029, copy_desktop_024, copy_desktop_029,
};

#[cfg(test)]
mod tests;
