//! Identity-platform authorization projected into core capabilities.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{CoreError, SubjectId};

/// A distinct action the core can be asked to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Provision a new Sandbox directly.
    CreateSandbox,
    /// Read and operate Sandboxes owned by the caller.
    UseOwnSandboxes,
    /// Delete a Sandbox owned by the caller.
    DeleteOwnSandbox,
    /// Request irreversible erasure of the caller's personal account.
    DeleteOwnAccount,
    /// Author and revise Profiles owned by the caller.
    ManageOwnProfiles,
    /// Create and manage external Bots owned by the caller.
    ManageOwnBots,
    /// Publish Triggers owned by the caller.
    PublishTriggers,
    /// Launch a published Trigger into the caller's own resources.
    LaunchTriggers,
    /// Read every Sandbox regardless of owner.
    ReadAllSandboxes,
    /// Retire or otherwise control a Sandbox regardless of owner.
    ControlAnySandbox,
    /// Read the provider runtime inventory.
    ReadRuntimeInventory,
    /// Read durable resource event logs.
    ReadEventLog,
    /// Read deployment-wide activity records.
    ReadActivityLog,
    /// Read the deployment user directory.
    ReadUserDirectory,
    /// Read and change deployment-owned settings.
    ConfigurePlatform,
    /// Manage opaque CMA event-gateway channel records.
    ManageGatewayRecords,
    /// Manage externally provisioned environments and conversations.
    ManageProviderBindings,
    /// Operate only conversations selected by verified binding-key authority.
    UseBindingConversations,
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
            Capability::UseOwnSandboxes,
            Capability::ConfigurePlatform,
            Capability::UseOwnSandboxes,
        ]);
        assert!(authority.allows(Capability::ConfigurePlatform));
        assert_eq!(authority.iter().count(), 2);
    }
}
