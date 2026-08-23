//! What a signed-in caller is told they may do.
//!
//! A client cannot offer a control intelligently without knowing whether the
//! request behind it will be permitted, and the alternatives are both bad: it
//! either offers everything and lets people discover refusals by hitting them,
//! or it guesses from a role name, which is the provider's vocabulary and none
//! of a browser's business. So the identity surface answers the question
//! directly, in the core's own vocabulary.
//!
//! **This is advisory and nothing else.** Every capability named here is
//! checked again by Layer 2 on the request that uses it. Hiding a control is a
//! courtesy to whoever is holding it; it is not access control, and a client
//! that fabricated this list would gain nothing.
//!
//! Nothing here is a secret. It is the caller's own entitlement, told to the
//! caller.

use layer2_core::{Authorization, Capability};

/// The name a client sees for a capability.
///
/// An exhaustive match rather than a serde rename on the core type, for two
/// reasons. A capability added to the core has to be named here deliberately —
/// the compiler is what asks — and the core stays free of any opinion about
/// what a client is told, which is Layer 1's and this binary's concern.
pub fn wire_name(capability: Capability) -> &'static str {
    match capability {
        Capability::CreateSession => "create_session",
        Capability::UseOwnSessions => "use_own_sessions",
        Capability::TerminateOwnSession => "terminate_own_session",
        Capability::ManageOwnEnvironmentProfiles => "manage_own_environment_profiles",
        Capability::ReadAllSessions => "read_all_sessions",
        Capability::ReadRuntimeInventory => "read_runtime_inventory",
        Capability::ReadEventLog => "read_event_log",
        Capability::ReadActivityLog => "read_activity_log",
        Capability::ReadUserDirectory => "read_user_directory",
        Capability::ControlAnySession => "control_any_session",
        Capability::ConfigurePlatform => "configure_platform",
        Capability::ExceedSessionLimit => "exceed_session_limit",
    }
}

/// Everything this caller holds, as a client is told it.
///
/// Ordered by the core's own ordering rather than by grant order, so two reads
/// of an unchanged authorization render identically.
pub fn reported(authorization: &Authorization) -> Vec<&'static str> {
    authorization.capabilities().map(wire_name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_authorization_reports_nothing() {
        // The shape that matters most: a caller with no entitlement is told so
        // plainly, and a client reading this fails closed on every control.
        assert!(reported(&Authorization::none()).is_empty());
    }

    #[test]
    fn every_capability_has_a_distinct_name() {
        // Two capabilities sharing a name would silently grant one when the
        // other was meant, and the failure would show up in a UI rather than
        // in a test.
        let all = [
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
            Capability::ConfigurePlatform,
            Capability::ExceedSessionLimit,
        ];
        let mut names: Vec<&str> = all.iter().copied().map(wire_name).collect();
        names.sort_unstable();
        let distinct = names.len();
        names.dedup();
        assert_eq!(names.len(), distinct, "a name is used twice: {names:?}");
    }

    #[test]
    fn an_operator_set_reports_exactly_what_was_granted() {
        let reported = reported(&Authorization::from_capabilities([
            Capability::ReadAllSessions,
            Capability::ReadUserDirectory,
            Capability::ControlAnySession,
        ]));
        assert!(reported.contains(&"read_all_sessions"));
        assert!(reported.contains(&"read_user_directory"));
        assert!(reported.contains(&"control_any_session"));
        assert_eq!(
            reported.len(),
            3,
            "nothing may be reported that was not granted"
        );
    }

    #[test]
    fn a_daily_user_is_not_told_they_may_operate() {
        // The regression this guards: an ordinary caller offered an operator
        // surface would spend their time discovering refusals.
        let reported = reported(&Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
        ]));
        for operator in [
            "read_all_sessions",
            "read_runtime_inventory",
            "read_activity_log",
            "read_user_directory",
            "control_any_session",
        ] {
            assert!(
                !reported.contains(&operator),
                "{operator} must not be reported to a daily user"
            );
        }
    }

    #[test]
    fn the_report_is_stable_whatever_order_capabilities_were_granted_in() {
        let one = Authorization::from_capabilities([
            Capability::ControlAnySession,
            Capability::CreateSession,
        ]);
        let other = Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::ControlAnySession,
        ]);
        assert_eq!(reported(&one), reported(&other));
    }
}
