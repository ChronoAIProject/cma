use crate::{AdapterError, PermissionLevel, RuntimeOptions};

/// Inputs to the Codex configuration renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexConfig {
    pub default_model: String,
    pub managed_base_url: String,
    pub managed_wire_api: String,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            default_model: "gpt-5.6-sol".into(),
            managed_base_url: crate::hosted::MODEL_PROXY_BASE_URL.into(),
            managed_wire_api: "responses".into(),
        }
    }
}

/// Wait for stream activity before Codex treats the connection as lost. This
/// is Codex's own default, kept explicit so a Codex upgrade cannot change it
/// silently. Long reasoning can be quiet for minutes, so it is not shortened.
pub const MANAGED_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
/// Re-run a sampling request at most once after its stream fails or goes
/// idle. Codex's default of five re-runs kept a turn whose upstream had
/// stalled in `Working` for about thirty minutes before it reported a failure.
pub const MANAGED_STREAM_MAX_RETRIES: u64 = 1;
/// Retry a failed HTTP request (5xx or transport) twice per sampling attempt.
/// Fast refusals are still covered, but a slow gateway timeout is not repeated
/// five times inside every sampling attempt.
pub const MANAGED_REQUEST_MAX_RETRIES: u64 = 2;

/// Render the exact config.toml for one agent home.
///
/// Managed access through the loopback provider states bounded stream and
/// request limits. A stalled model call then ends the turn with a reported
/// failure in about ten minutes in the worst case, and an accepted turn is
/// never resubmitted. Changing these bytes changes the rendered home, so a
/// caller that replays a prepared home must give it a new preparation
/// identity.
pub fn render_config(options: &RuntimeOptions, config: &CodexConfig) -> crate::Result<String> {
    let model = options.model.as_deref().unwrap_or(&config.default_model);
    if model.trim().is_empty() {
        return Err(AdapterError::Rejected("Codex model is empty".into()));
    }
    let model = toml_string(model)?;
    let effort = options.reasoning_effort.as_str();
    let (approval, sandbox) = permission_settings(options.permission);

    if options.own_credential {
        return Ok(format!(
            "model = {model}\n\
             model_reasoning_effort = \"{effort}\"\n\
             approval_policy = \"{approval}\"\n\
             sandbox_mode = \"{sandbox}\"\n"
        ));
    }

    if config.managed_base_url.trim().is_empty() || config.managed_wire_api.trim().is_empty() {
        return Err(AdapterError::Rejected(
            "managed model access requires a loopback provider configuration".into(),
        ));
    }
    let base_url = toml_string(config.managed_base_url.trim_end_matches('/'))?;
    let wire_api = toml_string(&config.managed_wire_api);
    let wire_api = wire_api?;
    let environment_name = super::home::MANAGED_PROXY_ENVIRONMENT_NAME;
    Ok(format!(
        "model = {model}\n\
         model_provider = \"cma\"\n\
         model_reasoning_effort = \"{effort}\"\n\
         approval_policy = \"{approval}\"\n\
         sandbox_mode = \"{sandbox}\"\n\
         \n\
         [model_providers.cma]\n\
         name = \"CMA Loopback\"\n\
         base_url = {base_url}\n\
         env_key = \"{environment_name}\"\n\
         wire_api = {wire_api}\n\
         request_max_retries = {MANAGED_REQUEST_MAX_RETRIES}\n\
         stream_max_retries = {MANAGED_STREAM_MAX_RETRIES}\n\
         stream_idle_timeout_ms = {MANAGED_STREAM_IDLE_TIMEOUT_MS}\n"
    ))
}

fn permission_settings(permission: PermissionLevel) -> (&'static str, &'static str) {
    match permission {
        PermissionLevel::Guarded => ("on-request", "read-only"),
        PermissionLevel::Autonomous => ("never", "danger-full-access"),
    }
}

fn toml_string(value: &str) -> crate::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| AdapterError::Rejected(format!("could not quote Codex setting: {error}")))
}
