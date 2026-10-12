use layer3_adapter::agent_sandbox::AgentSandboxConfig;

pub(super) fn from_lookup(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    namespace: &str,
    allocate: bool,
) -> Result<Option<AgentSandboxConfig>, String> {
    let enabled = match lookup("CHRONO_AGENT_SANDBOX_ENABLED")?.as_deref() {
        Some("true") => true,
        Some("false") | None => allocate,
        _ => return Err("CHRONO_AGENT_SANDBOX_ENABLED must be true or false".into()),
    };
    if !enabled {
        return Ok(None);
    }
    let key_file = super::required(lookup, "CHRONO_AGENT_SANDBOX_CONTROL_KEY_FILE")?;
    let control_key = std::fs::read_to_string(&key_file)
        .map_err(|_| "agent-sandbox control key file is unavailable".to_string())?
        .trim()
        .to_owned();
    let value = AgentSandboxConfig {
        namespace: namespace.into(),
        deployment: super::runtime::deployment_id_from_lookup(lookup)?,
        runtime_class: lookup("CHRONO_AGENT_SANDBOX_RUNTIME_CLASS")?.filter(|s| !s.is_empty()),
        service_account: lookup("CHRONO_AGENT_SANDBOX_SERVICE_ACCOUNT")?
            .unwrap_or_else(|| "cma-runtime".into()),
        image_pull_secrets: lookup("CHRONO_AGENT_SANDBOX_IMAGE_PULL_SECRETS")?
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        control_key,
    };
    value.validate().map_err(|e| e.to_string())?;
    Ok(Some(value))
}
