//! A2A interface profile retained for the agent repoint in issue #817.

use crate::{InterfaceProfile, Operation};

pub struct A2aInterface;

impl InterfaceProfile for A2aInterface {
    fn interface_id(&self) -> &'static str {
        "a2a"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}
