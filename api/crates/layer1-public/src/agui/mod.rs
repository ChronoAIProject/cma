//! AG-UI interface profile retained for the agent repoint in issue #817.

use crate::{InterfaceProfile, Operation};

mod launch_companion;

pub use launch_companion::{launch_companion_router, LaunchCompanionState};

#[cfg(test)]
mod launch_companion_tests;

pub struct AguiInterface;

impl InterfaceProfile for AguiInterface {
    fn interface_id(&self) -> &'static str {
        "ag-ui"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}
