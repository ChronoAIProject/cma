use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use super::CodexConfig;
use crate::{
    AdapterError, AgentHomePlan, AgentHomeRequest, AgentLaunch, AgentOpenRequest, HomeFile,
    HomeSymlink,
};

pub const BRIDGE_PROGRAM: &str = "codex-acp";
pub const SHARED_AUTH_PATH: &str = "/home/cma/.cma/provider/codex/auth.json";
pub const AGENT_HOME_ROOT: &str = "/home/cma/.cma/agents";
const HOME_ENVIRONMENT_NAME: &str = "CODEX_HOME";
pub(super) const MANAGED_PROXY_ENVIRONMENT_NAME: &str = "CMA_MODEL_PROXY_PLACEHOLDER";
pub(crate) const RESERVED_ENVIRONMENT_NAMES: &[&str] =
    &[HOME_ENVIRONMENT_NAME, MANAGED_PROXY_ENVIRONMENT_NAME];
const BASELINE_SKILLS: [(&str, &str); 2] = [
    ("nyxid", "/home/cma/.codex/skills/nyxid"),
    (
        "ornn-agent-manual-cli",
        "/home/cma/.codex/skills/ornn-agent-manual-cli",
    ),
];

pub(crate) fn expected_home(slot: &crate::AgentSlot) -> String {
    format!("{AGENT_HOME_ROOT}/{}/home", slot.0)
}

pub fn checkpoint_exclusions(agents: &[crate::AgentSlot]) -> crate::Result<Vec<String>> {
    // Preserve the wire paths across runtime upgrades so admitted holds replay
    // exactly. New runtimes expand these generated roots during archive capture.
    let mut paths = BTreeSet::from([".codex/tmp/arg0".to_string()]);
    for agent in agents {
        validate_slot(agent)?;
        // The 0.147 root identifies the provider-owned executable alias directory.
        paths.insert(format!(".cma/agents/{}/home/tmp/arg0", agent.0));
    }
    Ok(paths.into_iter().collect())
}

/// Resolve the generated Codex helper roots at archive capture, without changing
/// the persisted or in-flight checkpoint request. Codex 0.165 uses a sibling
/// `arg0-linux` directory. Retain support for the exact 0.147 paths as well.
pub fn checkpoint_archive_exclusions(exclusions: &[String]) -> Vec<String> {
    let mut paths: BTreeSet<String> = exclusions.iter().cloned().collect();
    for path in exclusions {
        let agent_root = path
            .strip_prefix(".cma/agents/")
            .and_then(|path| path.strip_suffix("/home/tmp/arg0"))
            .is_some_and(valid_component);
        if path == ".codex/tmp/arg0" || agent_root {
            paths.insert(format!("{path}-linux"));
        }
    }
    paths.into_iter().collect()
}

/// Render files, links, and requested archives for one Codex home.
pub fn render_agent_home(
    request: &AgentHomeRequest,
    config: &CodexConfig,
) -> crate::Result<AgentHomePlan> {
    validate_slot(&request.options.slot)?;
    if request.options.home_directory != expected_home(&request.options.slot) {
        return Err(AdapterError::Rejected(
            "Codex home does not match its agent slot".into(),
        ));
    }

    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut names: BTreeSet<&str> = BTreeSet::new();
    for skill in &request.skills {
        if !valid_component(&skill.id)
            || !ids.insert(&skill.id)
            || !valid_sha256(&skill.archive_sha256)
            || skill.source_url.trim().is_empty()
            || skill.name.as_deref().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 256
                    || name.trim() != name
                    || name.chars().any(char::is_control)
                    || !names.insert(name)
            })
        {
            return Err(AdapterError::Rejected(format!(
                "invalid or duplicate agent skill {}",
                skill.id
            )));
        }
    }

    validate_isolation(&request.options)?;
    let mut symlinks = if request.options.isolated_agent {
        Vec::new()
    } else {
        vec![HomeSymlink {
            path: "auth.json".into(),
            target: SHARED_AUTH_PATH.into(),
        }]
    };
    symlinks.extend(
        BASELINE_SKILLS
            .iter()
            .filter(|(id, _)| !ids.contains(id) && !names.contains(id))
            .map(|(id, target)| HomeSymlink {
                path: format!("skills/{id}"),
                target: (*target).to_string(),
            }),
    );

    Ok(AgentHomePlan {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        agent_id: request.options.slot.clone(),
        files: vec![
            HomeFile {
                path: "config.toml".into(),
                bytes: super::render_config(&request.options, config)?.into_bytes(),
            },
            HomeFile {
                path: "AGENTS.md".into(),
                bytes: request.system_prompt.as_bytes().to_vec(),
            },
        ],
        symlinks,
        // Names select provider fallbacks; the runtime installs immutable archives by ID.
        skills: request
            .skills
            .iter()
            .cloned()
            .map(|mut skill| {
                skill.name = None;
                skill
            })
            .collect(),
    })
}

/// Render the unchanged bridge launch. Conversation resume stays on ACP.
pub fn render_launch(request: &AgentOpenRequest) -> crate::Result<AgentLaunch> {
    validate_slot(&request.options.slot)?;
    validate_isolation(&request.options)?;
    if request.options.home_directory != expected_home(&request.options.slot)
        || !valid_worktree_path(&request.options.working_directory)
    {
        return Err(AdapterError::Rejected(
            "Codex agent requires its own home and a worktree cwd".into(),
        ));
    }

    let mut env = std::collections::BTreeMap::from([(
        HOME_ENVIRONMENT_NAME.to_string(),
        request.options.home_directory.clone(),
    )]);
    if !request.options.own_credential {
        env.insert(
            MANAGED_PROXY_ENVIRONMENT_NAME.into(),
            crate::hosted::PLACEHOLDER_CREDENTIAL.into(),
        );
    }
    Ok(AgentLaunch {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        agent_id: request.options.slot.clone(),
        cwd: request.options.working_directory.clone(),
        home: request.options.home_directory.clone(),
        program: if request.options.isolated_agent {
            "/usr/bin/python3".into()
        } else {
            BRIDGE_PROGRAM.into()
        },
        // codex-acp has no resume CLI argument. The existing ACP load call
        // receives resume_conversation after the bridge starts.
        args: if request.options.isolated_agent {
            vec![
                "-I".into(),
                "-c".into(),
                include_str!("isolate_agent.py").into(),
                "outer".into(),
                request.options.home_directory.clone(),
                request.options.working_directory.clone(),
            ]
        } else {
            Vec::new()
        },
        env,
    })
}

fn validate_isolation(options: &crate::RuntimeOptions) -> crate::Result<()> {
    if options.isolated_agent && options.own_credential {
        return Err(AdapterError::Rejected(
            "isolated agents require managed model access; shared provider credentials cannot be mounted".into(),
        ));
    }
    Ok(())
}

fn validate_slot(slot: &crate::AgentSlot) -> crate::Result<()> {
    if valid_component(&slot.0) {
        Ok(())
    } else {
        Err(AdapterError::Rejected("invalid Codex agent slot".into()))
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_worktree_path(value: &str) -> bool {
    let path = Path::new(value);
    let normalized: PathBuf = path.components().collect();
    if normalized.to_str() != Some(value) {
        return false;
    }

    let mut components = path.components();
    matches!(components.next(), Some(Component::RootDir))
        && matches!(components.next(), Some(Component::Normal(part)) if part.to_str() == Some("home"))
        && matches!(components.next(), Some(Component::Normal(part)) if part.to_str() == Some("cma"))
        && matches!(components.next(), Some(Component::Normal(_)))
        && matches!(components.next(), Some(Component::Normal(part)) if part.to_str() == Some(".worktrees"))
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}
