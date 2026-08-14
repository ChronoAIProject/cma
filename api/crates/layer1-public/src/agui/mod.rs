//! AG-UI interface (priority 1) — for frontend clients.
//!
//! A projection of the Layer 2 core session model into AG-UI idiom.

use crate::{InterfaceProfile, Operation};

/// The AG-UI projection of the core session model.
#[derive(Debug, Default)]
pub struct AguiInterface;

impl InterfaceProfile for AguiInterface {
    fn interface_id(&self) -> &'static str {
        "agui"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::Steer,
            Operation::GetUsage,
        ]
    }
}
