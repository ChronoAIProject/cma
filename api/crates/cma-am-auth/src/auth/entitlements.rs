//! Stable wire names for verified CMA authorization capabilities.

use layer2_core::{account_deletion::OwnerSessionAuthority, Caller, Capability};

pub fn wire_name(capability: Capability) -> &'static str {
    match capability {
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

pub fn reported(
    caller: &Caller,
    authority: Option<&OwnerSessionAuthority>,
    enabled: bool,
) -> Vec<&'static str> {
    let mut capabilities = caller
        .authorization
        .iter()
        .map(wire_name)
        .collect::<Vec<_>>();
    if enabled && authority.is_some_and(|authority| authority.subject() == &caller.subject) {
        capabilities.push("delete_own_account");
    }
    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::{Authorization, SubjectId};

    const ALL: [Capability; 17] = [
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
    fn deletion_entitlement_requires_enabled_subject_bound_session_authority() {
        let caller = Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let owner = OwnerSessionAuthority::verified_personal(caller.subject.clone());
        let other = OwnerSessionAuthority::verified_personal(SubjectId::new("other"));
        assert_eq!(
            reported(&caller, Some(&owner), true),
            ["use_own_sandboxes", "delete_own_account"]
        );
        for (authority, enabled) in [(None, true), (Some(&other), true), (Some(&owner), false)] {
            assert_eq!(reported(&caller, authority, enabled), ["use_own_sandboxes"]);
        }
        assert_eq!(
            serde_json::to_value(&caller.authorization).unwrap(),
            serde_json::json!(["use_own_sandboxes"])
        );
    }

    #[test]
    fn reported_authority_is_stable_and_exact() {
        let authority = Authorization::new([
            Capability::ReadUserDirectory,
            Capability::UseOwnSandboxes,
            Capability::LaunchTriggers,
        ]);
        assert_eq!(
            reported(&Caller::new(SubjectId::new("owner"), authority), None, true),
            [
                "use_own_sandboxes",
                "launch_triggers",
                "read_user_directory"
            ]
        );
    }
}
