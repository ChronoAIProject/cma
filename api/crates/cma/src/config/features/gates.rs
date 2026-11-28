pub(super) const RETIREMENT_CAPTURE: &str = "CHRONO_NATIVE_RETIREMENT_CAPTURE_ENABLED";

pub(super) const WORKSPACE_PREPARE_OBSERVATIONS: &str =
    "CHRONO_WORKSPACE_PREPARE_OBSERVATIONS_ENABLED";

/// Rollout gates graduated to default-on after their rollout completed in
/// every environment. Unset means enabled; an explicit `false` remains the
/// emergency off-switch, and any other value keeps the strict-boolean error.
/// No startup validation ties these gates to another setting.
pub(in crate::config) const GRADUATED: &[&str] = &[
    "CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED",
    "CHRONO_COMMAND_RECONCILE_PACING_ENABLED",
    "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED",
    "CHRONO_SANDBOX_NATIVE_CLAIM_GUARD_ENABLED",
    "CHRONO_V2_UI_TRANSPORT_ENABLED",
];

/// Read one strict boolean rollout gate. Unset is off unless it graduated.
pub(in crate::config) fn flag(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
) -> Result<bool, String> {
    match lookup(key)?.as_deref() {
        None => Ok(GRADUATED.contains(&key)),
        Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(format!("{key} must be true or false")),
    }
}
pub(super) const CREDENTIAL_RETIREMENT_RECEIPTS: &str = "CHRONO_CREDENTIAL_RETIREMENT_RECEIPTS";
pub(super) const RETIREMENT_REAUTHORIZATION: &str = "CHRONO_RETIREMENT_REAUTHORIZATION_ENABLED";

pub(super) fn validate_startup(flags: &super::FeatureFlags) -> Result<(), String> {
    if flags.am_startup_health_serving && flags.am_failure_containment_enabled {
        return Err("CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED cannot be combined with CHRONO_AGENT_MANAGER_FAILURE_CONTAINMENT_ENABLED".into());
    }
    if flags.am_empty_warm_pool_startup && !flags.am_startup_health_serving {
        return Err("CHRONO_AM_EMPTY_WARM_POOL_STARTUP_ENABLED requires CHRONO_AM_STARTUP_HEALTH_SERVING_ENABLED".into());
    }
    Ok(())
}
