//! REST interface (priority 2) — for backend server clients.
//!
//! A projection of the Layer 2 core session model into REST idiom.

use crate::{InterfaceProfile, Operation};

/// The REST projection of the core session model.
#[derive(Debug, Default)]
pub struct RestInterface;

impl InterfaceProfile for RestInterface {
    fn interface_id(&self) -> &'static str {
        "rest"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::Steer,
            Operation::ResizeTerminal,
            Operation::ScreenSnapshot,
            Operation::GetUsage,
        ]
    }
}
