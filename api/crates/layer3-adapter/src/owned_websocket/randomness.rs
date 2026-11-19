//! Owned OS entropy for the explicit client nonce and mask ports.

use ring::rand::{SecureRandom, SystemRandom};

use crate::ingress_budget::{
    sizing::shared_owner_bytes, AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};

/// The production source has no selectable algorithm or configurable seed.
/// The protocol adapters map fill failure to their typed entropy refusal.
pub struct SystemRandomSource {
    source: SharedOwned<SystemRandom>,
}

impl SystemRandomSource {
    /// Include the source's shared owners and its eventual trait-object box.
    /// HTTP requests, encoded nonces and codec storage have separate plans.
    pub fn allocation_plan() -> crate::Result<AllocationPlan> {
        AllocationPlan::new(
            "transport_entropy_source",
            &[
                shared_owner_bytes::<SystemRandom>(),
                size_of::<Self>(),
                align_of::<Self>(),
            ],
        )
    }

    /// Admit the source before constructing its ownership metadata.
    pub fn new(policy: &IngressOwnershipPolicy) -> crate::Result<Self> {
        let storage = policy.reserve(IngressClass::Control, Self::allocation_plan()?)?;
        Ok(Self {
            source: storage.initialize(SystemRandom::new),
        })
    }

    /// Fill from the selected OS backend without a rand thread-local fallback.
    ///
    /// Ring permits the first fill to block. A caller timeout does not cancel
    /// this synchronous call; supported-platform progress needs qualification.
    pub fn fill(&mut self, output: &mut [u8]) -> Result<(), ring::error::Unspecified> {
        self.source.fill(output)
    }

    pub fn requested_bytes(&self) -> usize {
        self.source.requested_bytes()
    }
}

impl std::fmt::Debug for SystemRandomSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SystemRandomSource")
    }
}

#[cfg(test)]
#[path = "randomness_tests.rs"]
mod tests;
