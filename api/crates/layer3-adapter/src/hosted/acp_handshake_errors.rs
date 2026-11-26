use super::{AdapterError, HostedAgentOpenSettings};
use crate::ingress_budget::diagnostic::text;

/// MCP setup carries a limited CMA bearer. Runtimes may echo that descriptor in
/// error JSON; preserve only the category before worker logs retain the failure.
pub(super) fn protected_setup_failure(error: AdapterError, phase: &'static str) -> AdapterError {
    let category = match error {
        AdapterError::AccessRevoked => return AdapterError::AccessRevoked,
        AdapterError::InputAuthorityUnavailable(_) => AdapterError::InputAuthorityUnavailable,
        AdapterError::PromptNotWritten(_) => AdapterError::PromptNotWritten,
        AdapterError::Unavailable(_)
        | AdapterError::RuntimeStartupHold(_)
        | AdapterError::NativePoolDefinitionAbsent { .. } => AdapterError::Unavailable,
        guarded @ AdapterError::NativeClaimNotReady(_) => return guarded,
        AdapterError::Rejected(_)
        | AdapterError::SkillUpgradeRefused(_)
        | AdapterError::AdmittedSandboxSpecDrift
        | AdapterError::NativePoolConfigurationDrift
        | AdapterError::DesktopFailed => AdapterError::Rejected,
        AdapterError::OperationIdentityConflict(_) => AdapterError::OperationIdentityConflict,
        AdapterError::RuntimeLedgerHold(_) => AdapterError::RuntimeLedgerHold,
        capacity @ AdapterError::RuntimeCapacity(_) => return capacity,
        AdapterError::AtCapacity(_) => AdapterError::AtCapacity,
        AdapterError::Unsupported(_) => AdapterError::Unsupported,
        AdapterError::RuntimeContractMismatch(_) => AdapterError::Rejected,
    };
    crate::ingress_budget::diagnostic::make_preserving_category(
        category,
        format_args!("channel tool session setup did not complete (phase: {phase})"),
    )
}

pub(super) fn unconfirmed_setup() -> AdapterError {
    // An earlier initialize/open can have succeeded after its reply was lost.
    // Missing confirmation is not evidence that this runtime lacks an ability.
    // Without a known identity and a supported exact completion proof there is
    // no safe automatic recovery. Never create a substitute session or loop
    // indefinitely while the undispatched creation input remains queued.
    AdapterError::Rejected(text("existing conversation tool setup cannot be confirmed"))
}

pub(super) fn validate_settings(
    cwd: &str,
    settings: &HostedAgentOpenSettings,
) -> crate::Result<()> {
    let valid_mode = settings.permission_mode.as_deref().is_none_or(|mode| {
        !mode.is_empty()
            && mode.len() <= 128
            && mode.trim() == mode
            && !mode.chars().any(char::is_control)
    });
    if cwd.is_empty()
        || cwd.len() > 4_096
        || !cwd.starts_with('/')
        || cwd.chars().any(char::is_control)
        || !valid_mode
        || settings
            .recovered_context_sequence
            .is_some_and(|sequence| sequence == 0)
        || settings.recovered_context_sequence.is_some()
            && (settings.context_refresh.is_none() || settings.recovered_prompt.is_some())
    {
        return Err(AdapterError::Rejected(text(
            "ACP working directory or permission mode is invalid",
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "acp_handshake_error_tests.rs"]
mod error_tests;
