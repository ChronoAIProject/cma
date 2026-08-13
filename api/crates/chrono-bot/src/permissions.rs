//! Translating the identity provider's permissions into core capabilities.
//!
//! This is the only place that knows the provider's vocabulary. The core deals
//! in capabilities, so renaming a role, restructuring the hierarchy, or moving
//! to a different provider changes this file and nothing else.
//!
//! Translation is deny-by-default and additive: an unrecognised permission
//! grants nothing rather than being guessed at, and a caller ends up with
//! exactly the union of what their permissions name.

use layer2_core::{Authorization, Capability};

/// Permission strings this service recognises.
///
/// The provider's convention is `<product>:<resource>:<action>`, matching the
/// neighbouring products on the same platform.
pub mod permission {
    /// Provision a session.
    pub const SESSION_CREATE: &str = "chrono-bot:session:create";
    /// Read and attach to your own sessions.
    pub const SESSION_USE: &str = "chrono-bot:session:use";
    /// End your own session.
    pub const SESSION_TERMINATE: &str = "chrono-bot:session:terminate";
    /// Read every session, whoever owns it.
    pub const ADMIN_SESSIONS: &str = "chrono-bot:admin:sessions";
    /// Read the sandbox inventory and its status.
    pub const ADMIN_SANDBOXES: &str = "chrono-bot:admin:sandboxes";
    /// Read the durable event log of any session.
    pub const ADMIN_EVENTS: &str = "chrono-bot:admin:events";
}

/// Wildcard the provider issues to a full administrator.
const WILDCARD: &str = "*";

/// Prefix a product-wide wildcard would carry.
const PRODUCT_WILDCARD: &str = "chrono-bot:*";

/// Translate provider permissions into the capabilities the core enforces.
///
/// Unknown entries are ignored rather than rejected: the provider is shared
/// with other products, so a caller legitimately carries permissions that mean
/// nothing here.
pub fn capabilities_from_permissions<S: AsRef<str>>(
    permissions: impl IntoIterator<Item = S>,
) -> Authorization {
    let mut granted: Vec<Capability> = Vec::new();
    let mut wildcard = false;

    for raw in permissions {
        let p = raw.as_ref().trim();
        if p == WILDCARD || p == PRODUCT_WILDCARD {
            wildcard = true;
            continue;
        }
        match p {
            permission::SESSION_CREATE => granted.push(Capability::CreateSession),
            permission::SESSION_USE => granted.push(Capability::UseOwnSessions),
            permission::SESSION_TERMINATE => granted.push(Capability::TerminateOwnSession),
            permission::ADMIN_SESSIONS => granted.push(Capability::ReadAllSessions),
            permission::ADMIN_SANDBOXES => granted.push(Capability::ReadRuntimeInventory),
            permission::ADMIN_EVENTS => granted.push(Capability::ReadEventLog),
            _ => {}
        }
    }

    if wildcard {
        granted.extend([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
        ]);
    }

    Authorization::from_capabilities(granted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_permissions_grants_nothing() {
        // Authenticating is not authorization. A user of the wider platform who
        // has not been given access here gets none.
        let a = capabilities_from_permissions(Vec::<String>::new());
        assert!(!a.allows(Capability::CreateSession));
        assert!(!a.allows(Capability::UseOwnSessions));
        assert!(!a.is_privileged());
    }

    #[test]
    fn the_daily_user_set_grants_use_but_nothing_privileged() {
        let a = capabilities_from_permissions([
            permission::SESSION_CREATE,
            permission::SESSION_USE,
            permission::SESSION_TERMINATE,
        ]);
        assert!(a.allows(Capability::CreateSession));
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(a.allows(Capability::TerminateOwnSession));
        assert!(
            !a.is_privileged(),
            "a daily user must not see other people's sessions"
        );
        assert!(!a.allows(Capability::ReadAllSessions));
        assert!(!a.allows(Capability::ReadRuntimeInventory));
    }

    #[test]
    fn the_admin_set_grants_reads_across_subjects() {
        let a = capabilities_from_permissions([
            permission::ADMIN_SESSIONS,
            permission::ADMIN_SANDBOXES,
            permission::ADMIN_EVENTS,
        ]);
        assert!(a.allows(Capability::ReadAllSessions));
        assert!(a.allows(Capability::ReadRuntimeInventory));
        assert!(a.allows(Capability::ReadEventLog));
        assert!(a.is_privileged());
    }

    #[test]
    fn admin_permissions_alone_do_not_grant_session_use() {
        // Monitoring and operating are separate. A pure admin can watch without
        // being able to start or drive a session, which keeps the dashboard
        // role narrow enough to hand out freely.
        let a = capabilities_from_permissions([
            permission::ADMIN_SESSIONS,
            permission::ADMIN_SANDBOXES,
        ]);
        assert!(!a.allows(Capability::CreateSession));
        assert!(!a.allows(Capability::UseOwnSessions));
        assert!(!a.allows(Capability::TerminateOwnSession));
    }

    #[test]
    fn a_platform_wildcard_grants_everything() {
        let a = capabilities_from_permissions(["*"]);
        for c in [
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
        ] {
            assert!(a.allows(c), "wildcard must grant {c:?}");
        }
    }

    #[test]
    fn a_product_wildcard_grants_everything_here() {
        assert!(capabilities_from_permissions(["chrono-bot:*"]).allows(Capability::ReadAllSessions));
    }

    #[test]
    fn permissions_for_other_products_are_ignored_not_rejected() {
        // The provider is shared. Carrying another product's permissions is
        // normal and must neither grant anything here nor fail the request.
        let a = capabilities_from_permissions([
            "ornn:skill:read",
            "bucket:write",
            "nyxid:admin:read",
            permission::SESSION_USE,
        ]);
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(!a.is_privileged(), "another product's admin is not ours");
        assert!(!a.allows(Capability::ReadAllSessions));
    }

    #[test]
    fn surrounding_whitespace_does_not_defeat_a_grant() {
        let a = capabilities_from_permissions(["  chrono-bot:session:use  "]);
        assert!(a.allows(Capability::UseOwnSessions));
    }

    #[test]
    fn a_near_miss_grants_nothing() {
        // Deny-by-default: anything not matched exactly is ignored, so a typo
        // in a role definition fails closed.
        let a = capabilities_from_permissions([
            "chrono-bot:session:read",
            "chronobot:session:use",
            "chrono-bot:admin",
        ]);
        assert_eq!(a.capabilities().count(), 0);
    }
}
