//! Identity-platform authorization projected into core capabilities.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{CoreError, SubjectId};

/// Generic authority retained while resource-specific capabilities move to #802.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Read resources owned by the caller.
    ReadOwnResources,
    /// Change resources owned by the caller.
    ManageOwnResources,
    /// Read deployment-wide activity records.
    ReadActivity,
    /// Read the deployment user directory.
    ReadDirectory,
    /// Read resources belonging to any subject.
    ReadAllResources,
    /// Change deployment-owned settings.
    ConfigurePlatform,
}

/// A closed set of capabilities established by the authentication boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Authorization(BTreeSet<Capability>);

impl Authorization {
    /// An authorization that grants nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// Build an authorization from verified capabilities.
    pub fn new(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    /// Whether this authority contains a capability.
    pub fn allows(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }

    /// Refuse unless a capability is present.
    pub fn require(&self, capability: Capability) -> Result<(), CoreError> {
        self.allows(capability)
            .then_some(())
            .ok_or(CoreError::Forbidden)
    }

    /// Iterate over the verified capabilities in stable order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }
}

/// An authenticated subject and the authority verified for this request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// Stable identity asserted by the identity provider.
    pub subject: SubjectId,
    /// Capabilities resolved for the request.
    pub authorization: Authorization,
}

impl Caller {
    /// Bind an identity to its verified authority.
    pub fn new(subject: SubjectId, authorization: Authorization) -> Self {
        Self {
            subject,
            authorization,
        }
    }

    /// Refuse unless the caller holds a capability.
    pub fn require(&self, capability: Capability) -> Result<(), CoreError> {
        self.authorization.require(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_is_closed_and_defaults_to_deny() {
        assert_eq!(
            Authorization::none().require(Capability::ConfigurePlatform),
            Err(CoreError::Forbidden)
        );

        let authority = Authorization::new([
            Capability::ReadOwnResources,
            Capability::ConfigurePlatform,
            Capability::ReadOwnResources,
        ]);
        assert!(authority.allows(Capability::ConfigurePlatform));
        assert_eq!(authority.iter().count(), 2);
    }
}
