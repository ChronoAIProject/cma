//! Runtime allocation custody shared by adapters, consumers and public sinks.
//! Persisted assemblies and source receipts retain their existing schemas.

pub use layer3_adapter::ingress_budget::{
    failure::{AdmissionRefusal, AdmittedError},
    ownership::{
        AdmittedFrame, AdmittedSocket, ReservationBundle, RuntimeOutputDelivery, SharedOwned,
    },
    scope::{IngressWorkScope, OwnedWork},
    sizing::{self, AllocationPlan, ClassBounds, IngressClass},
    IngressBudget, IngressOwnershipPolicy,
};

use super::AgentMessageAssembly;

/// One consumer owns its assembly, including retained quiet receipt identities.
/// This owner is deliberately not Clone. P2 supplies the recording algorithm.
pub struct RecordingSession {
    assembly: AssemblyStorage,
    policy: Option<IngressOwnershipPolicy>,
}

enum AssemblyStorage {
    Empty(AgentMessageAssembly),
    Admitted(OwnedWork<AgentMessageAssembly>),
}

impl RecordingSession {
    pub fn new(policy: Option<IngressOwnershipPolicy>) -> Self {
        Self {
            assembly: AssemblyStorage::Empty(AgentMessageAssembly::default()),
            policy,
        }
    }

    pub fn policy(&self) -> Option<&IngressOwnershipPolicy> {
        self.policy.as_ref()
    }

    pub fn assembly(&self) -> &AgentMessageAssembly {
        match &self.assembly {
            AssemblyStorage::Empty(assembly) => assembly,
            AssemblyStorage::Admitted(assembly) => assembly,
        }
    }

    /// Transfer the completed fold and its working custody before old storage drops.
    pub fn replace(
        &mut self,
        next: OwnedWork<AgentMessageAssembly>,
    ) -> Result<(), super::AgentConversationError> {
        if self
            .policy
            .as_ref()
            .is_some_and(|policy| !next.belongs_to(policy))
        {
            return Err(super::AgentConversationError::CapacityExceeded);
        }
        self.assembly = AssemblyStorage::Admitted(next);
        Ok(())
    }
}

#[cfg(test)]
#[path = "ingress_ownership_tests.rs"]
mod tests;
