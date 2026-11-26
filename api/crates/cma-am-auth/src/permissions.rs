//! Translate exact identity-provider permissions into core capabilities.

use layer2_core::{Authorization, Capability};

/// Canonical permission strings understood by CMA.
pub mod permission {
    pub const SANDBOX_CREATE: &str = "cma:sandbox:create";
    pub const SANDBOX_USE: &str = "cma:sandbox:use";
    pub const SANDBOX_DELETE: &str = "cma:sandbox:delete";
    pub const BOT_MANAGE: &str = "cma:bot:manage";
    pub const PROFILE_MANAGE: &str = "cma:profile:manage";
    pub const TRIGGER_PUBLISH: &str = "cma:trigger:publish";
    pub const TRIGGER_LAUNCH: &str = "cma:trigger:launch";
    pub const ADMIN_SANDBOXES: &str = "cma:admin:sandboxes";
    pub const ADMIN_EVENTS: &str = "cma:admin:events";
    pub const ADMIN_ACTIVITY: &str = "cma:admin:activity";
    pub const ADMIN_USERS: &str = "cma:admin:users";
    pub const ADMIN_CONTROL: &str = "cma:admin:control";
    pub const ADMIN_SETTINGS: &str = "cma:admin:settings";
    pub const PROVIDER_BINDINGS: &str = "cma:provider:bindings";
    pub const GATEWAY_RECORDS: &str = "cma:gateway:records";
}

const PROVIDER_WILDCARD: &str = "*";
const PRODUCT_WILDCARD: &str = "cma:*";

const OWN_WORK: [Capability; 7] = [
    Capability::CreateSandbox,
    Capability::UseOwnSandboxes,
    Capability::DeleteOwnSandbox,
    Capability::ManageOwnProfiles,
    Capability::ManageOwnBots,
    Capability::PublishTriggers,
    Capability::LaunchTriggers,
];

// Binding conversation authority is established from custody, never provider RBAC.
const ALL_CAPABILITIES: [Capability; 16] = [
    Capability::CreateSandbox,
    Capability::UseOwnSandboxes,
    Capability::DeleteOwnSandbox,
    Capability::ManageOwnProfiles,
    Capability::ManageOwnBots,
    Capability::PublishTriggers,
    Capability::LaunchTriggers,
    Capability::ReadAllSandboxes,
    Capability::ControlAnySandbox,
    Capability::ReadRuntimeInventory,
    Capability::ReadEventLog,
    Capability::ReadActivityLog,
    Capability::ReadUserDirectory,
    Capability::ConfigurePlatform,
    Capability::ManageGatewayRecords,
    Capability::ManageProviderBindings,
];

/// Translate exact provider grants. Unknown permissions grant nothing.
pub fn capabilities_from_permissions<S: AsRef<str>>(
    permissions: impl IntoIterator<Item = S>,
) -> Authorization {
    let mut granted = Vec::new();
    for permission in permissions {
        match permission.as_ref().trim() {
            PROVIDER_WILDCARD => granted.extend(OWN_WORK),
            PRODUCT_WILDCARD => granted.extend(ALL_CAPABILITIES),
            permission::SANDBOX_CREATE => granted.push(Capability::CreateSandbox),
            permission::SANDBOX_USE => granted.push(Capability::UseOwnSandboxes),
            permission::SANDBOX_DELETE => granted.push(Capability::DeleteOwnSandbox),
            permission::BOT_MANAGE => granted.push(Capability::ManageOwnBots),
            permission::PROFILE_MANAGE => granted.push(Capability::ManageOwnProfiles),
            permission::TRIGGER_PUBLISH => granted.push(Capability::PublishTriggers),
            permission::TRIGGER_LAUNCH => granted.push(Capability::LaunchTriggers),
            permission::ADMIN_SANDBOXES => granted.extend([
                Capability::ReadAllSandboxes,
                Capability::ReadRuntimeInventory,
            ]),
            permission::ADMIN_EVENTS => granted.push(Capability::ReadEventLog),
            permission::ADMIN_ACTIVITY => granted.push(Capability::ReadActivityLog),
            permission::ADMIN_USERS => granted.push(Capability::ReadUserDirectory),
            permission::ADMIN_CONTROL => granted.push(Capability::ControlAnySandbox),
            permission::ADMIN_SETTINGS => granted.push(Capability::ConfigurePlatform),
            permission::PROVIDER_BINDINGS => granted.push(Capability::ManageProviderBindings),
            permission::GATEWAY_RECORDS => granted.push(Capability::ManageGatewayRecords),
            _ => {}
        }
    }
    Authorization::new(granted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_permissions_are_additive_and_unknown_names_deny() {
        let authority = capabilities_from_permissions([
            permission::SANDBOX_USE,
            permission::TRIGGER_LAUNCH,
            permission::ADMIN_ACTIVITY,
            "other-product:admin",
            "cma:sandbox:*",
        ]);
        assert!(authority.allows(Capability::UseOwnSandboxes));
        assert!(authority.allows(Capability::LaunchTriggers));
        assert!(authority.allows(Capability::ReadActivityLog));
        assert_eq!(authority.iter().count(), 3);
    }

    #[test]
    fn bot_management_grants_only_owner_bot_authority() {
        let authority = capabilities_from_permissions([permission::BOT_MANAGE]);
        assert_eq!(
            authority.iter().collect::<Vec<_>>(),
            [Capability::ManageOwnBots]
        );
        assert!(
            !capabilities_from_permissions(["cma:bots:manage", "cma:bot:*"])
                .allows(Capability::ManageOwnBots)
        );
    }

    #[test]
    fn provider_wildcard_stops_at_own_work() {
        let authority = capabilities_from_permissions([PROVIDER_WILDCARD]);
        for capability in OWN_WORK {
            assert!(authority.allows(capability), "missing {capability:?}");
        }
        assert!(!authority.allows(Capability::ReadAllSandboxes));
        assert!(!authority.allows(Capability::ControlAnySandbox));
        assert!(!authority.allows(Capability::ConfigurePlatform));
    }

    #[test]
    fn product_wildcard_reaches_every_cma_capability() {
        let authority = capabilities_from_permissions([PRODUCT_WILDCARD]);
        for capability in ALL_CAPABILITIES {
            assert!(authority.allows(capability), "missing {capability:?}");
        }
        assert_eq!(authority.iter().count(), ALL_CAPABILITIES.len());
        assert!(!authority.allows(Capability::UseBindingConversations));
    }

    #[test]
    fn visitor_role_permissions_are_exactly_non_publishing_own_use() {
        let authority = capabilities_from_permissions([
            permission::TRIGGER_LAUNCH,
            permission::SANDBOX_USE,
            permission::SANDBOX_DELETE,
        ]);
        assert!(authority.allows(Capability::LaunchTriggers));
        assert!(authority.allows(Capability::UseOwnSandboxes));
        assert!(authority.allows(Capability::DeleteOwnSandbox));
        assert!(!authority.allows(Capability::CreateSandbox));
        assert!(!authority.allows(Capability::ManageOwnProfiles));
        assert!(!authority.allows(Capability::PublishTriggers));
    }

    #[test]
    fn sandbox_operator_read_also_reaches_runtime_inventory() {
        let authority = capabilities_from_permissions([permission::ADMIN_SANDBOXES]);
        assert!(authority.allows(Capability::ReadAllSandboxes));
        assert!(authority.allows(Capability::ReadRuntimeInventory));
        assert_eq!(authority.iter().count(), 2);
    }

    #[test]
    fn gateway_record_permission_maps_to_its_dedicated_capability() {
        let authority = capabilities_from_permissions([permission::GATEWAY_RECORDS]);
        assert!(authority.allows(Capability::ManageGatewayRecords));
        assert_eq!(authority.iter().count(), 1);
    }
}
