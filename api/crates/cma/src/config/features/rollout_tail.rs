//! Rollout gates for external service and repository integration.

/// Rollout gates graduated to default-on after their rollout completed in
/// every environment. Unset means enabled; an explicit `false` remains the
/// emergency off-switch, and any other value keeps the strict-boolean error.
/// No startup validation ties these gates to another setting.
pub(crate) const GRADUATED: &[&str] = &[
    "CHRONO_AGENT_CONTROL_WRITE_COORDINATION_ENABLED",
    "CHRONO_COMMAND_RECONCILE_PACING_ENABLED",
    "CHRONO_OWNERSHIP_CLEANUP_PACING_ENABLED",
    "CHRONO_SANDBOX_NATIVE_CLAIM_GUARD_ENABLED",
    "CHRONO_V2_UI_TRANSPORT_ENABLED",
];

/// Read one strict boolean rollout gate. Unset is off unless it graduated.
pub(crate) fn flag(
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

pub(super) struct TailFlags {
    pub(super) segment_adoption_enabled: bool,
    pub(super) paged_response_events: bool,
    pub(super) v2_ui_transport: bool,
    pub(super) trigger_command_admission: bool,
    pub(super) github_repository_preflight: bool,
    pub(super) nyxid_resource_request: bool,
    pub(super) nyxid_proxy: bool,
    pub(super) nyxid_proxy_replay_principal: bool,
    pub(super) service_authorization: bool,
    pub(super) silent_sign_in: bool,
    pub(super) document_revocation_reserve: bool,
}

pub(super) fn read(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<TailFlags, String> {
    Ok(TailFlags {
        segment_adoption_enabled: flag(lookup, "CHRONO_AGENT_LOG_SEGMENT_ADOPTION_ENABLED")?,
        paged_response_events: flag(lookup, "CHRONO_RESPONSE_PAGED_EVENTS_ENABLED")?,
        v2_ui_transport: flag(lookup, "CHRONO_V2_UI_TRANSPORT_ENABLED")?,
        trigger_command_admission: flag(lookup, "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED")?,
        github_repository_preflight: flag(lookup, "CHRONO_GITHUB_REPOSITORY_PREFLIGHT_ENABLED")?,
        nyxid_resource_request: flag(lookup, "CHRONO_NYXID_RESOURCE_REQUEST_ENABLED")?,
        nyxid_proxy: flag(lookup, "CHRONO_NYXID_PROXY_ENABLED")?,
        nyxid_proxy_replay_principal: flag(lookup, "CHRONO_NYXID_PROXY_REPLAY_PRINCIPAL_ENABLED")?,
        service_authorization: flag(lookup, "CHRONO_SERVICE_AUTHORIZATION_ENABLED")?,
        silent_sign_in: flag(lookup, "CHRONO_SILENT_SIGN_IN_ENABLED")?,
        document_revocation_reserve: flag(lookup, "CHRONO_DOCUMENT_REVOCATION_RESERVE_ENABLED")?,
    })
}
