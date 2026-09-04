//! AG-UI interface profile retained for the agent repoint in issue #817.

use crate::{InterfaceProfile, Operation};

pub struct AguiInterface;

impl InterfaceProfile for AguiInterface {
    fn interface_id(&self) -> &'static str {
        "ag-ui"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}
