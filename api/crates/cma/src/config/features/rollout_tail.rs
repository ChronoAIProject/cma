//! Rollout gates for external service and repository integration.

use super::flag;

pub(super) struct TailFlags {
    pub(super) segment_adoption_enabled: bool,
    pub(super) paged_response_events: bool,
    pub(super) incremental_text: bool,
    pub(super) v2_ui_transport: bool,
    pub(super) trigger_command_admission: bool,
    pub(super) github_repository_preflight: bool,
    pub(super) github_connection_approval: bool,
    pub(super) github_connection_recovery: bool,
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
        incremental_text: flag(lookup, "CHRONO_AGENT_INCREMENTAL_TEXT_ENABLED")?,
        v2_ui_transport: flag(lookup, "CHRONO_V2_UI_TRANSPORT_ENABLED")?,
        trigger_command_admission: flag(lookup, "CHRONO_TRIGGER_COMMAND_ADMISSION_ENABLED")?,
        github_repository_preflight: flag(lookup, "CHRONO_GITHUB_REPOSITORY_PREFLIGHT_ENABLED")?,
        github_connection_approval: flag(lookup, "CHRONO_GITHUB_CONNECTION_APPROVAL_ENABLED")?,
        github_connection_recovery: flag(lookup, "CHRONO_GITHUB_CONNECTION_RECOVERY_ENABLED")?,
        nyxid_resource_request: flag(lookup, "CHRONO_NYXID_RESOURCE_REQUEST_ENABLED")?,
        nyxid_proxy: flag(lookup, "CHRONO_NYXID_PROXY_ENABLED")?,
        nyxid_proxy_replay_principal: flag(lookup, "CHRONO_NYXID_PROXY_REPLAY_PRINCIPAL_ENABLED")?,
        service_authorization: flag(lookup, "CHRONO_SERVICE_AUTHORIZATION_ENABLED")?,
        silent_sign_in: flag(lookup, "CHRONO_SILENT_SIGN_IN_ENABLED")?,
        document_revocation_reserve: flag(lookup, "CHRONO_DOCUMENT_REVOCATION_RESERVE_ENABLED")?,
    })
}
