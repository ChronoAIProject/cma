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
    /// Author and reuse your own environment profiles.
    pub const ENVIRONMENT_PROFILE_MANAGE: &str = "chrono-bot:environment-profile:manage";
    /// Read every session, whoever owns it.
    pub const ADMIN_SESSIONS: &str = "chrono-bot:admin:sessions";
    /// Read the sandbox inventory and its status.
    pub const ADMIN_SANDBOXES: &str = "chrono-bot:admin:sandboxes";
    /// Read the durable event log of any session.
    pub const ADMIN_EVENTS: &str = "chrono-bot:admin:events";
    /// Read the record of what the service was asked to do.
    pub const ADMIN_ACTIVITY: &str = "chrono-bot:admin:activity";
    /// Read the directory of subjects that have used the service.
    pub const ADMIN_USERS: &str = "chrono-bot:admin:users";
    /// End or pause a session whatever subject owns it.
    ///
    /// Its own permission rather than part of the monitoring set, because it
    /// is the only one of these that changes anything. A role handed out so
    /// somebody can watch the service should not also let them stop it.
    pub const ADMIN_CONTROL: &str = "chrono-bot:admin:control";
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
            permission::SESSION_CREATE => {
                granted.push(Capability::CreateSession);
                // An environment profile is an input to creating a session:
                // it describes what the session a caller may already start
                // will carry, and reaches nothing but that caller's own
                // profiles. Granting it alongside keeps a permission set
                // issued before profiles existed from producing a caller who
                // can start sessions but not describe them.
                granted.push(Capability::ManageOwnEnvironmentProfiles);
            }
            permission::ENVIRONMENT_PROFILE_MANAGE => {
                granted.push(Capability::ManageOwnEnvironmentProfiles)
            }
            permission::SESSION_USE => granted.push(Capability::UseOwnSessions),
            permission::SESSION_TERMINATE => granted.push(Capability::TerminateOwnSession),
            permission::ADMIN_SESSIONS => granted.push(Capability::ReadAllSessions),
            permission::ADMIN_SANDBOXES => granted.push(Capability::ReadRuntimeInventory),
            permission::ADMIN_EVENTS => granted.push(Capability::ReadEventLog),
            permission::ADMIN_ACTIVITY => granted.push(Capability::ReadActivityLog),
            permission::ADMIN_USERS => granted.push(Capability::ReadUserDirectory),
            permission::ADMIN_CONTROL => granted.push(Capability::ControlAnySession),
            _ => {}
        }
    }

    if wildcard {
        granted.extend([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
            Capability::ReadActivityLog,
            Capability::ReadUserDirectory,
            Capability::ControlAnySession,
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
            a.allows(Capability::ManageOwnEnvironmentProfiles),
            "describing what your own session carries is part of starting one"
        );
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
            permission::ADMIN_ACTIVITY,
            permission::ADMIN_USERS,
        ]);
        assert!(a.allows(Capability::ReadAllSessions));
        assert!(a.allows(Capability::ReadRuntimeInventory));
        assert!(a.allows(Capability::ReadEventLog));
        assert!(a.allows(Capability::ReadActivityLog));
        assert!(a.allows(Capability::ReadUserDirectory));
        assert!(a.is_privileged());
    }

    #[test]
    fn watching_the_service_does_not_confer_stopping_anybody_elses_work() {
        // The whole monitoring set, and still not one capability that changes
        // anything. Ending somebody else's session is its own permission, so a
        // role handed out to let somebody watch cannot quietly also stop work.
        let a = capabilities_from_permissions([
            permission::ADMIN_SESSIONS,
            permission::ADMIN_SANDBOXES,
            permission::ADMIN_EVENTS,
            permission::ADMIN_ACTIVITY,
            permission::ADMIN_USERS,
        ]);
        assert!(!a.allows(Capability::ControlAnySession));
    }

    #[test]
    fn the_control_permission_grants_control_and_no_reads() {
        let a = capabilities_from_permissions([permission::ADMIN_CONTROL]);
        assert!(a.allows(Capability::ControlAnySession));
        assert!(
            a.is_privileged(),
            "acting on other people's work is privileged"
        );
        assert!(!a.allows(Capability::ReadAllSessions));
        assert!(!a.allows(Capability::ReadActivityLog));
        assert!(!a.allows(Capability::ReadUserDirectory));
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
        assert!(!a.allows(Capability::ManageOwnEnvironmentProfiles));
    }

    #[test]
    fn the_profile_permission_grants_profiles_and_nothing_else() {
        // Its own permission so a deployment can hand out profile authoring
        // without handing out the ability to start a session with one.
        let a = capabilities_from_permissions([permission::ENVIRONMENT_PROFILE_MANAGE]);
        assert!(a.allows(Capability::ManageOwnEnvironmentProfiles));
        assert!(!a.allows(Capability::CreateSession));
        assert!(!a.allows(Capability::UseOwnSessions));
        assert!(!a.is_privileged());
    }

    #[test]
    fn a_platform_wildcard_grants_everything() {
        let a = capabilities_from_permissions(["*"]);
        for c in [
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
            Capability::ReadActivityLog,
            Capability::ReadUserDirectory,
            Capability::ControlAnySession,
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
