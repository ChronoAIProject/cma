//! Translate identity-provider permissions into retained core capabilities.

use layer2_core::{Authorization, Capability};

pub mod permission {
    pub const ADMIN_SANDBOXES: &str = "cma:admin:sandboxes";
    pub const ADMIN_ACTIVITY: &str = "cma:admin:activity";
    pub const ADMIN_USERS: &str = "cma:admin:users";
    pub const ADMIN_SETTINGS: &str = "cma:admin:settings";
}

const PROVIDER_WILDCARD: &str = "*";
const PRODUCT_WILDCARD: &str = "cma:*";

/// Translate exact provider grants. Unknown permissions grant nothing.
pub fn capabilities_from_permissions<S: AsRef<str>>(
    permissions: impl IntoIterator<Item = S>,
) -> Authorization {
    let mut granted = Vec::new();
    for permission in permissions {
        match permission.as_ref().trim() {
            PROVIDER_WILDCARD => {
                granted.extend([Capability::ReadOwnResources, Capability::ManageOwnResources])
            }
            PRODUCT_WILDCARD => granted.extend([
                Capability::ReadOwnResources,
                Capability::ManageOwnResources,
                Capability::ReadActivity,
                Capability::ReadDirectory,
                Capability::ReadAllResources,
                Capability::ConfigurePlatform,
            ]),
            permission::ADMIN_SANDBOXES => granted.push(Capability::ReadAllResources),
            permission::ADMIN_ACTIVITY => granted.push(Capability::ReadActivity),
            permission::ADMIN_USERS => granted.push(Capability::ReadDirectory),
            permission::ADMIN_SETTINGS => granted.push(Capability::ConfigurePlatform),
            _ => {}
        }
    }
    Authorization::new(granted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_is_additive_and_deny_by_default() {
        let authority = capabilities_from_permissions([
            "other-product:admin",
            permission::ADMIN_ACTIVITY,
            permission::ADMIN_USERS,
        ]);
        assert!(authority.allows(Capability::ReadActivity));
        assert!(authority.allows(Capability::ReadDirectory));
        assert!(!authority.allows(Capability::ConfigurePlatform));
    }

    #[test]
    fn a_provider_wildcard_stops_at_own_work() {
        let authority = capabilities_from_permissions([PROVIDER_WILDCARD]);
        assert!(authority.allows(Capability::ManageOwnResources));
        assert!(!authority.allows(Capability::ReadAllResources));
    }
}
