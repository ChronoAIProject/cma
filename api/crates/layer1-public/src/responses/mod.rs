//! Responses interface profile retained for the agent repoint in issue #817.

use crate::{InterfaceProfile, Operation};

pub struct ResponsesInterface;

impl InterfaceProfile for ResponsesInterface {
    fn interface_id(&self) -> &'static str {
        "responses"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}
