//! Runtime abilities expressed without provider-specific vocabulary.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One ability a runtime may advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCapability {
    Conversation,
    Snapshot,
    Steer,
    Interrupt,
    Usage,
    Resources,
}

/// A closed set of runtime abilities, used for advertisements and frozen grants.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeCapabilities(BTreeSet<RuntimeCapability>);

impl RuntimeCapabilities {
    /// Build a capability declaration.
    pub fn new(values: impl IntoIterator<Item = RuntimeCapability>) -> Self {
        Self(values.into_iter().collect())
    }

    /// Whether the runtime advertises an ability.
    pub fn supports(&self, capability: RuntimeCapability) -> bool {
        self.0.contains(&capability)
    }

    /// Iterate over the frozen grants in stable order.
    pub fn iter(&self) -> impl Iterator<Item = RuntimeCapability> + '_ {
        self.0.iter().copied()
    }

    /// Refuse a request whose required abilities are absent.
    pub fn require(
        &self,
        required: impl IntoIterator<Item = RuntimeCapability>,
    ) -> Result<(), RuntimeCapability> {
        required
            .into_iter()
            .find(|capability| !self.supports(*capability))
            .map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_reports_the_first_missing_ability() {
        let offered =
            RuntimeCapabilities::new([RuntimeCapability::Conversation, RuntimeCapability::Usage]);
        assert_eq!(
            offered.require([
                RuntimeCapability::Conversation,
                RuntimeCapability::Snapshot,
                RuntimeCapability::Usage,
            ]),
            Err(RuntimeCapability::Snapshot)
        );
    }
}
