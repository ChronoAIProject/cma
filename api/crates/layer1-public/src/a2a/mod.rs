//! A2A interface (priority 5) — for agent clients.
//!
//! A projection of the Layer 2 core session model into A2A idiom.

use crate::{InterfaceProfile, Operation};

/// The A2A projection of the core session model.
#[derive(Debug, Default)]
pub struct A2aInterface;

impl InterfaceProfile for A2aInterface {
    fn interface_id(&self) -> &'static str {
        "a2a"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
        ]
    }
}
