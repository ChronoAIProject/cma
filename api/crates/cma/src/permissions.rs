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

/// Canonical permission strings this service recognises.
///
/// The provider's convention is `<product>:<resource>:<action>`, matching the
/// neighbouring products on the same platform. New roles use these spellings;
/// the exact pre-CMA spellings remain private compatibility aliases below.
pub mod permission {
    /// Provision a session.
    pub const SESSION_CREATE: &str = "cma:session:create";
    /// Read and attach to your own sessions.
    pub const SESSION_USE: &str = "cma:session:use";
    /// End your own session.
    pub const SESSION_TERMINATE: &str = "cma:session:terminate";
    /// Author and reuse your own environment profiles.
    pub const ENVIRONMENT_PROFILE_MANAGE: &str = "cma:environment-profile:manage";
    /// Read every session, whoever owns it.
    pub const ADMIN_SESSIONS: &str = "cma:admin:sessions";
    /// Read the sandbox inventory and its status.
    pub const ADMIN_SANDBOXES: &str = "cma:admin:sandboxes";
    /// Read the durable event log of any session.
    pub const ADMIN_EVENTS: &str = "cma:admin:events";
    /// Read the record of what the service was asked to do.
    pub const ADMIN_ACTIVITY: &str = "cma:admin:activity";
    /// Read the directory of subjects that have used the service.
    pub const ADMIN_USERS: &str = "cma:admin:users";
    /// End or pause a session whatever subject owns it.
    ///
    /// Its own permission rather than part of the monitoring set, because it
    /// is the only one of these that changes anything. A role handed out so
    /// somebody can watch the service should not also let them stop it.
    pub const ADMIN_CONTROL: &str = "cma:admin:control";
    /// Read and change settings that apply to the whole deployment.
    ///
    /// Separate from `ADMIN_CONTROL` for the same reason that one is separate
    /// from the monitoring set, and further still: stopping a session affects
    /// the session stopped, while changing a platform setting affects every
    /// session created afterwards, including ones belonging to people who
    /// will never meet whoever changed it.
    pub const ADMIN_SETTINGS: &str = "cma:admin:settings";
    /// Create and revive sessions past the deployment's live-session limit.
    ///
    /// Its own string rather than part of any admin bundle. Every other
    /// `admin:` permission here either observes across subjects or acts on
    /// somebody else's session; this one does neither, and reaches nothing but
    /// how many sessions its own holder may have at once. Bundling it with a
    /// monitoring role would hand unlimited session creation to somebody who
    /// asked to watch the service.
    pub const ADMIN_UNLIMITED_SESSIONS: &str = "cma:admin:unlimited-sessions";
}

/// Exact grants issued before the CMA permission vocabulary was introduced.
///
/// These are aliases, not a second public vocabulary. Keeping them explicit
/// preserves deny-by-default translation: similar prefixes, suffixes, and
/// future permissions do not acquire authority by inference.
mod legacy_permission {
    pub const SESSION_CREATE: &str = "chrono-bot:session:create";
    pub const SESSION_USE: &str = "chrono-bot:session:use";
    pub const SESSION_TERMINATE: &str = "chrono-bot:session:terminate";
    pub const ENVIRONMENT_PROFILE_MANAGE: &str = "chrono-bot:environment-profile:manage";
    pub const ADMIN_SESSIONS: &str = "chrono-bot:admin:sessions";
    pub const ADMIN_SANDBOXES: &str = "chrono-bot:admin:sandboxes";
    pub const ADMIN_EVENTS: &str = "chrono-bot:admin:events";
    pub const ADMIN_ACTIVITY: &str = "chrono-bot:admin:activity";
    pub const ADMIN_USERS: &str = "chrono-bot:admin:users";
    pub const ADMIN_CONTROL: &str = "chrono-bot:admin:control";
    pub const ADMIN_SETTINGS: &str = "chrono-bot:admin:settings";
    pub const ADMIN_UNLIMITED_SESSIONS: &str = "chrono-bot:admin:unlimited-sessions";
}

/// Wildcard the provider issues across everything it serves.
///
/// **Not an administrator here.** The provider is shared, and this is its
/// vocabulary rather than ours: it says the subject is fully entitled *on the
/// provider*, which is a statement about storage, sandboxes and every other
/// product behind it. Reading it as permission to act on another subject's
/// work in this service hands one product's grant to a different product's
/// data, and a session belongs to the person who started it.
///
/// So it grants what a caller may do to their own sessions, and nothing that
/// reaches anybody else's. Only a grant that names this product can do that.
const WILDCARD: &str = "*";

/// Wildcard naming this product, which does grant everything here.
///
/// Distinct from the one above because this one was written by somebody
/// deciding about CMA.
const PRODUCT_WILDCARD: &str = "cma:*";

/// Exact pre-CMA product wildcard, retained for already-issued grants.
const LEGACY_PRODUCT_WILDCARD: &str = "chrono-bot:*";

/// What a subject may do to their own work, and nothing beyond it.
const OWN_WORK: [Capability; 4] = [
    Capability::CreateSession,
    Capability::UseOwnSessions,
    Capability::TerminateOwnSession,
    Capability::ManageOwnEnvironmentProfiles,
];

/// Whether a grant naming this product reaches `capability`.
///
/// A match rather than a list, and exhaustive rather than defaulted: a
/// capability added to the core stops this file compiling until somebody
/// decides. The list this replaced could silently narrow the wildcard.
///
/// Every arm answers `true` today, and that is not a reason to delete the
/// question. Translation here is deny-by-default; granting whatever the enum
/// happens to hold would invert that and hand out an undecided capability by
/// inheritance. The value is the arm somebody has to type, not its answer.
const fn product_wildcard_reaches(capability: Capability) -> bool {
    match capability {
        // The caller's own work. Reached here too, because a wildcard naming
        // this product that granted less than the provider's plain wildcard
        // would be a narrower grant with a wider name.
        Capability::CreateSession
        | Capability::UseOwnSessions
        | Capability::TerminateOwnSession
        | Capability::ManageOwnEnvironmentProfiles => true,
        // Everything outside the ordinary own-work grant. Only a grant naming
        // this product reaches these; that is the distinction this file exists
        // to draw.
        Capability::ReadAllSessions
        | Capability::ReadRuntimeInventory
        | Capability::ReadEventLog
        | Capability::ReadActivityLog
        | Capability::ReadUserDirectory
        | Capability::ConfigurePlatform
        | Capability::ControlAnySession
        | Capability::ExceedSessionLimit => true,
    }
}

/// Translate provider permissions into the capabilities the core enforces.
///
/// Unknown entries are ignored rather than rejected: the provider is shared
/// with other products, so a caller legitimately carries permissions that mean
/// nothing here.
pub fn capabilities_from_permissions<S: AsRef<str>>(
    permissions: impl IntoIterator<Item = S>,
) -> Authorization {
    let mut granted: Vec<Capability> = Vec::new();
    let mut own_work = false;
    let mut everything = false;

    for raw in permissions {
        let p = raw.as_ref().trim();
        if p == WILDCARD {
            own_work = true;
            continue;
        }
        if p == PRODUCT_WILDCARD || p == LEGACY_PRODUCT_WILDCARD {
            everything = true;
            continue;
        }
        match p {
            permission::SESSION_CREATE | legacy_permission::SESSION_CREATE => {
                granted.push(Capability::CreateSession);
                // An environment profile is an input to creating a session:
                // it describes what the session a caller may already start
                // will carry, and reaches nothing but that caller's own
                // profiles. Granting it alongside keeps a permission set
                // issued before profiles existed from producing a caller who
                // can start sessions but not describe them.
                granted.push(Capability::ManageOwnEnvironmentProfiles);
            }
            permission::ENVIRONMENT_PROFILE_MANAGE
            | legacy_permission::ENVIRONMENT_PROFILE_MANAGE => {
                granted.push(Capability::ManageOwnEnvironmentProfiles)
            }
            permission::SESSION_USE | legacy_permission::SESSION_USE => {
                granted.push(Capability::UseOwnSessions)
            }
            permission::SESSION_TERMINATE | legacy_permission::SESSION_TERMINATE => {
                granted.push(Capability::TerminateOwnSession)
            }
            permission::ADMIN_SESSIONS | legacy_permission::ADMIN_SESSIONS => {
                granted.push(Capability::ReadAllSessions)
            }
            permission::ADMIN_SANDBOXES | legacy_permission::ADMIN_SANDBOXES => {
                granted.push(Capability::ReadRuntimeInventory)
            }
            permission::ADMIN_EVENTS | legacy_permission::ADMIN_EVENTS => {
                granted.push(Capability::ReadEventLog)
            }
            permission::ADMIN_ACTIVITY | legacy_permission::ADMIN_ACTIVITY => {
                granted.push(Capability::ReadActivityLog)
            }
            permission::ADMIN_USERS | legacy_permission::ADMIN_USERS => {
                granted.push(Capability::ReadUserDirectory)
            }
            permission::ADMIN_CONTROL | legacy_permission::ADMIN_CONTROL => {
                granted.push(Capability::ControlAnySession)
            }
            permission::ADMIN_SETTINGS | legacy_permission::ADMIN_SETTINGS => {
                granted.push(Capability::ConfigurePlatform)
            }
            permission::ADMIN_UNLIMITED_SESSIONS | legacy_permission::ADMIN_UNLIMITED_SESSIONS => {
                granted.push(Capability::ExceedSessionLimit)
            }
            _ => {}
        }
    }

    // The provider's own wildcard reaches the caller's work and stops there.
    if own_work {
        granted.extend(OWN_WORK);
    }

    // A wildcard naming this product is the only thing that crosses subjects
    // without saying which capability it means. What it reaches is decided
    // one capability at a time above, so a new one cannot join by accident or
    // be left out in silence.
    if everything {
        granted.extend(
            Capability::ALL
                .into_iter()
                .filter(|capability| product_wildcard_reaches(*capability)),
        );
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
    fn a_monitoring_role_does_not_gain_unlimited_sessions() {
        // Privileged monitoring is still only monitoring. Deriving this
        // exemption from the catch-all privilege predicate would silently hand
        // session-limit bypass to a role whose purpose is to watch the service.
        let monitoring = capabilities_from_permissions([
            permission::ADMIN_SESSIONS,
            permission::ADMIN_SANDBOXES,
            permission::ADMIN_EVENTS,
            permission::ADMIN_ACTIVITY,
            permission::ADMIN_USERS,
        ]);
        assert!(!monitoring.allows(Capability::ExceedSessionLimit));

        let control = capabilities_from_permissions([permission::ADMIN_CONTROL]);
        assert!(
            !control.allows(Capability::ExceedSessionLimit),
            "control over another subject's session is not a limit exemption"
        );
    }

    #[test]
    fn the_settings_role_does_not_gain_unlimited_sessions() {
        // Authoring the deployment's limit and being exempt from it are
        // separate decisions, so the settings grant must not imply the latter.
        let a = capabilities_from_permissions([permission::ADMIN_SETTINGS]);
        assert!(!a.allows(Capability::ExceedSessionLimit));
    }

    #[test]
    fn the_daily_user_set_does_not_gain_unlimited_sessions() {
        // Everyday session use reaches only the holder's own work; it must not
        // quietly opt that holder out of a deployment-wide policy.
        let a = capabilities_from_permissions([
            permission::SESSION_CREATE,
            permission::SESSION_USE,
            permission::SESSION_TERMINATE,
        ]);
        assert!(!a.allows(Capability::ExceedSessionLimit));
    }

    #[test]
    fn the_providers_own_wildcard_does_not_gain_unlimited_sessions() {
        // The provider-wide wildcard represents ordinary work across a shared
        // provider. Only a permission naming this product may grant an
        // exemption from this product's deployment policy.
        let a = capabilities_from_permissions([WILDCARD]);
        assert!(!a.allows(Capability::ExceedSessionLimit));
    }

    #[test]
    fn the_permission_grants_this_and_nothing_else() {
        // The exemption is deliberately unbundled. Walking the maintained set
        // makes any accidental second grant visible as the capability itself.
        let a = capabilities_from_permissions([permission::ADMIN_UNLIMITED_SESSIONS]);
        for capability in Capability::ALL {
            assert_eq!(
                a.allows(capability),
                capability == Capability::ExceedSessionLimit,
                "the unlimited-sessions permission grants {capability:?} incorrectly"
            );
        }
        assert_eq!(a.capabilities().count(), 1);
    }

    #[test]
    fn an_unrecognised_permission_still_grants_nothing() {
        // Translation is deny-by-default. A singular near miss must not be
        // guessed into the deployment-wide exemption.
        let a = capabilities_from_permissions(["cma:admin:unlimited-session"]);
        assert_eq!(a.capabilities().count(), 0);
    }

    #[test]
    fn the_platform_wildcard_grants_a_caller_their_own_work() {
        let a = capabilities_from_permissions(["*"]);
        for c in [
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
        ] {
            assert!(a.allows(c), "the provider's wildcard must grant {c:?}");
        }
    }

    #[test]
    fn the_platform_wildcard_does_not_reach_another_subjects_work() {
        // The provider is shared, and its wildcard is a statement about the
        // provider rather than about this service. Reading it as administrator
        // here showed every subject's sessions to every subject holding it.
        let a = capabilities_from_permissions(["*"]);
        for c in [
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
            Capability::ReadActivityLog,
            Capability::ReadUserDirectory,
            Capability::ControlAnySession,
        ] {
            assert!(!a.allows(c), "the provider's wildcard must not grant {c:?}");
        }
        assert!(
            !a.is_privileged(),
            "a caller entitled on the provider is not an operator here"
        );
        assert_eq!(
            a.capabilities().count(),
            OWN_WORK.len(),
            "the provider's wildcard reaches a caller's own work and stops there"
        );
    }

    #[test]
    fn a_product_wildcard_grants_everything_here() {
        // Named this product, so it is a decision about this product.
        let a = capabilities_from_permissions([PRODUCT_WILDCARD]);
        for c in Capability::ALL {
            assert!(a.allows(c), "the product wildcard must grant {c:?}");
        }
        // Against the enum, deliberately. Counting the grant against the list
        // that produced it compares a thing with itself and cannot fail from
        // an omission, which is the omission this test exists to catch.
        assert_eq!(
            a.capabilities().count(),
            Capability::ALL.len(),
            "the wildcard must grant every capability this product has and no more"
        );
    }

    #[test]
    fn the_product_wildcard_grants_exactly_what_it_says_it_reaches() {
        // The grant and the decision are two expressions of one fact, and a
        // test that reads only one cannot notice them parting.
        let a = capabilities_from_permissions([PRODUCT_WILDCARD]);
        for c in Capability::ALL {
            assert_eq!(
                a.allows(c),
                product_wildcard_reaches(c),
                "{c:?} is granted differently from how it is decided"
            );
        }
    }

    #[test]
    fn changing_the_platform_needs_a_permission_that_names_it() {
        // The blast radius is what makes this worth its own test: this is the
        // one capability that changes what every session created afterwards is
        // told, including sessions belonging to people who will never meet
        // whoever changed it.
        let named = capabilities_from_permissions([permission::ADMIN_SETTINGS]);
        assert!(named.allows(Capability::ConfigurePlatform));
        assert!(
            named.capabilities().count() == 1,
            "naming one permission grants one capability, not a bundle"
        );

        // Everything a caller could plausibly hold that is not it.
        for held in [
            vec![permission::ADMIN_SESSIONS],
            vec![permission::ADMIN_CONTROL],
            vec![permission::ADMIN_ACTIVITY],
            vec![permission::ADMIN_USERS],
            vec![permission::SESSION_CREATE, permission::SESSION_USE],
            // Entitled on the shared provider, which says nothing about here.
            vec!["*"],
            vec![],
        ] {
            let a = capabilities_from_permissions(held.clone());
            assert!(
                !a.allows(Capability::ConfigurePlatform),
                "{held:?} must not reach the platform's own settings"
            );
        }
    }

    #[test]
    fn configuring_the_platform_is_privileged() {
        // `is_privileged` has no production caller: it is the catch-all two
        // other tests lean on to notice a capability that reaches past its
        // caller's own work. A new one left out of it disarms them silently.
        assert!(Capability::ConfigurePlatform.is_privileged());
    }

    #[test]
    fn the_permissions_this_deployment_actually_issues_reach_no_other_subject() {
        // The exact set the provider returns for a subject here. Every one of
        // these belongs to another product except the wildcard, and none of
        // them is a decision that this subject may see somebody else's work.
        let a = capabilities_from_permissions([
            "object:delete",
            "fkst:session:stop",
            "sandbox:execute",
            "ornn:skill:read",
            "bucket:read",
            "*",
        ]);
        assert!(a.allows(Capability::UseOwnSessions));
        assert!(!a.allows(Capability::ReadAllSessions));
        assert!(!a.allows(Capability::ControlAnySession));
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
        let a = capabilities_from_permissions(["  cma:session:use  "]);
        assert!(a.allows(Capability::UseOwnSessions));
    }

    #[test]
    fn a_near_miss_grants_nothing() {
        // Deny-by-default: anything not matched exactly is ignored, so a typo
        // in a role definition fails closed.
        let a =
            capabilities_from_permissions(["cma:session:read", "cmabot:session:use", "cma:admin"]);
        assert_eq!(a.capabilities().count(), 0);
    }

    #[test]
    fn every_legacy_exact_grant_keeps_the_canonical_capability() {
        let aliases = [
            (
                permission::SESSION_CREATE,
                legacy_permission::SESSION_CREATE,
            ),
            (permission::SESSION_USE, legacy_permission::SESSION_USE),
            (
                permission::SESSION_TERMINATE,
                legacy_permission::SESSION_TERMINATE,
            ),
            (
                permission::ENVIRONMENT_PROFILE_MANAGE,
                legacy_permission::ENVIRONMENT_PROFILE_MANAGE,
            ),
            (
                permission::ADMIN_SESSIONS,
                legacy_permission::ADMIN_SESSIONS,
            ),
            (
                permission::ADMIN_SANDBOXES,
                legacy_permission::ADMIN_SANDBOXES,
            ),
            (permission::ADMIN_EVENTS, legacy_permission::ADMIN_EVENTS),
            (
                permission::ADMIN_ACTIVITY,
                legacy_permission::ADMIN_ACTIVITY,
            ),
            (permission::ADMIN_USERS, legacy_permission::ADMIN_USERS),
            (permission::ADMIN_CONTROL, legacy_permission::ADMIN_CONTROL),
            (
                permission::ADMIN_SETTINGS,
                legacy_permission::ADMIN_SETTINGS,
            ),
            (
                permission::ADMIN_UNLIMITED_SESSIONS,
                legacy_permission::ADMIN_UNLIMITED_SESSIONS,
            ),
        ];

        for (canonical, legacy) in aliases {
            let canonical_authorization = capabilities_from_permissions([canonical]);
            let legacy_authorization = capabilities_from_permissions([legacy]);
            assert!(
                canonical_authorization.capabilities().next().is_some(),
                "the canonical grant {canonical} must grant something"
            );
            assert_eq!(
                legacy_authorization, canonical_authorization,
                "the compatibility grant {legacy} drifted from {canonical}"
            );
        }
    }

    #[test]
    fn the_legacy_product_wildcard_keeps_every_capability() {
        assert_eq!(
            capabilities_from_permissions([LEGACY_PRODUCT_WILDCARD]),
            capabilities_from_permissions([PRODUCT_WILDCARD])
        );
        assert_eq!(
            capabilities_from_permissions([LEGACY_PRODUCT_WILDCARD])
                .capabilities()
                .count(),
            Capability::ALL.len()
        );
    }
}
