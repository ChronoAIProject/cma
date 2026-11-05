pub(super) fn guidance(class: &str) -> &'static str {
    match class {
        "agent_log_capacity_exceeded" =>
            "This Agent's retained history cannot accept more work. Waiting or restarting does not free history capacity. Keep its history and contact the deployment operator about recovery, or explicitly create a replacement Agent.",
        "resource_quota" | "workspace_resource_quota" | "agent_resource_quota" =>
            "Free unused capacity or choose a smaller profile before creating a replacement.",
        "service_not_granted" =>
            "Connect the services required by the Sandbox Profile before creating a replacement.",
        "terminal_security" | "workspace_runtime_contract_mismatch" | "workspace_runtime_receipt_invalid"
        | "agent_runtime_contract_mismatch" | "agent_conversation_identity_mismatch" | "agent_parent_corrupt" =>
            "Contact the deployment operator to check runtime compatibility and access before retrying.",
        "agent_skill_not_found" | "agent_skill_invalid" =>
            "Check the skills in the Agent Profile, publish a corrected revision, and create a replacement.",
        "terminal_validation" | "workspace_runtime_rejected" | "agent_runtime_rejected" | "agent_runtime_unsupported" =>
            "Check the selected profile and its commands before creating a replacement. Contact the deployment operator if the problem persists.",
        "retryable_transient" | "workspace_dependency_unavailable" | "agent_runtime_unavailable"
        | "agent_store_unavailable" | "agent_skill_source_unavailable" =>
            "Check again later. Contact the deployment operator if the dependency remains unavailable.",
        _ => "Review the selected profile and contact the deployment operator if the problem persists.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remediation_is_safe_and_specific_without_interpolating_failure_details() {
        for class in [
            "resource_quota",
            "workspace_resource_quota",
            "agent_resource_quota",
        ] {
            assert!(guidance(class).contains("capacity"));
        }
        assert!(guidance("service_not_granted").contains("Connect"));
        assert!(guidance("terminal_security").contains("operator"));
        assert!(guidance("agent_skill_invalid").contains("skills"));
        assert!(guidance("workspace_runtime_rejected").contains("commands"));
        assert!(guidance("retryable_transient").contains("later"));
        let capacity = guidance("agent_log_capacity_exceeded");
        assert!(capacity.contains("Waiting or restarting does not free"));
        assert!(capacity.contains("Keep its history"));
        assert!(capacity.contains("explicitly create a replacement"));
        assert!(!guidance("private upstream error").contains("private upstream error"));
    }
}
