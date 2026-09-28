use super::{BotSetup, SetupStep};
use crate::{bots::BotError, profiles::WorkspaceLocation};

/// Small text edits remain provider-neutral and receive the same authoritative
/// validation as complete configurations. A separate `done` confirms the edits.
pub(super) fn apply(setup: &mut BotSetup, text: &str) -> Result<bool, BotError> {
    let Some((field, value)) = text.trim().split_once(':') else {
        return Ok(false);
    };
    let value = value.trim();
    match (setup.step, field.trim().to_ascii_lowercase().as_str()) {
        (SetupStep::AgentCustom, "instructions") => {
            setup.configuration.agent.system_prompt = value.into();
        }
        (SetupStep::AgentCustom, "model") if !value.is_empty() => {
            setup.configuration.agent.model =
                (!value.eq_ignore_ascii_case("default")).then(|| value.into());
        }
        (SetupStep::WorkspaceCustom, "repository") => {
            let values: Vec<_> = value.split_whitespace().collect();
            if values.is_empty() || values.len() > 2 {
                return Err(BotError::Invalid(
                    "Use repository: https://github.com/owner/repo [branch-or-tag].".into(),
                ));
            }
            setup.configuration.workspace.location = WorkspaceLocation::GitRepository {
                url: values[0].into(),
                reference: values.get(1).map(|value| (*value).into()),
            };
        }
        (SetupStep::WorkspaceCustom, "directory") => {
            setup.configuration.workspace.location =
                WorkspaceLocation::Directory { name: value.into() };
            setup.configuration.workspace.name = value.into();
        }
        (SetupStep::SandboxCustom, "resources") => {
            let values: Vec<_> = value.split_whitespace().collect();
            let invalid = || {
                BotError::Invalid(
                    "Use resources: 2000 4096 (CPU in millicores, memory in MiB).".into(),
                )
            };
            if values.len() != 2 {
                return Err(invalid());
            }
            setup.configuration.sandbox.cpu_millis = values[0].parse().map_err(|_| invalid())?;
            setup.configuration.sandbox.memory_mib = values[1].parse().map_err(|_| invalid())?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
