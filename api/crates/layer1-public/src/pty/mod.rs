//! PTY interface (priority 3) — for CLI-like streaming clients.
//!
//! A projection of the Layer 2 core session model into PTY idiom.

use crate::{InterfaceProfile, Operation};

/// The PTY projection of the core session model.
#[derive(Debug, Default)]
pub struct PtyInterface;

impl InterfaceProfile for PtyInterface {
    fn interface_id(&self) -> &'static str {
        "pty"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::Interrupt,
            Operation::ResizeTerminal,
            Operation::ScreenSnapshot,
        ]
    }
}
