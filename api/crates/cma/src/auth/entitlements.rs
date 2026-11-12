//! Stable wire names for verified CMA authorization capabilities.

use layer2_core::{Authorization, Capability};

pub(crate) fn wire_name(capability: Capability) -> &'static str {
    match capability {
        Capability::DeleteOwnAccount => "delete_own_account",
        Capability::CreateSandbox => "create_sandbox",
        Capability::UseOwnSandboxes => "use_own_sandboxes",
        Capability::DeleteOwnSandbox => "delete_own_sandbox",
        Capability::ManageOwnBots => "manage_own_bots",
        Capability::ManageOwnProfiles => "manage_own_profiles",
        Capability::PublishTriggers => "publish_triggers",
        Capability::LaunchTriggers => "launch_triggers",
        Capability::ReadAllSandboxes => "read_all_sandboxes",
        Capability::ControlAnySandbox => "control_any_sandbox",
        Capability::ReadRuntimeInventory => "read_runtime_inventory",
        Capability::ReadEventLog => "read_event_log",
        Capability::ReadActivityLog => "read_activity_log",
        Capability::ReadUserDirectory => "read_user_directory",
        Capability::ConfigurePlatform => "configure_platform",
        Capability::UseBindingConversations => "use_binding_conversations",
        Capability::ManageProviderBindings => "manage_provider_bindings",
        Capability::ManageGatewayRecords => "manage_gateway_records",
    }
}

pub(crate) fn reported(authorization: &Authorization) -> Vec<&'static str> {
    authorization
        .iter()
        .filter(|capability| *capability != Capability::DeleteOwnAccount)
        .map(wire_name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Capability; 18] = [
        Capability::DeleteOwnAccount,
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
        Capability::UseBindingConversations,
    ];

    #[test]
    fn every_wire_name_is_distinct() {
        let mut names = ALL.into_iter().map(wire_name).collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn reported_authority_is_stable_and_exact() {
        let authority = Authorization::new([
            Capability::ReadUserDirectory,
            Capability::UseOwnSandboxes,
            Capability::LaunchTriggers,
        ]);
        assert_eq!(
            reported(&authority),
            [
                "use_own_sandboxes",
                "launch_triggers",
                "read_user_directory"
            ]
        );
    }
}
