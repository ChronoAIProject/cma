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

    let mut ids: BTreeSet<&str> = BASELINE_SKILLS.iter().map(|(id, _)| *id).collect();
    for skill in &request.skills {
        if !valid_component(&skill.id)
            || !ids.insert(&skill.id)
            || !valid_sha256(&skill.archive_sha256)
            || skill.source_url.trim().is_empty()
        {
            return Err(AdapterError::Rejected(format!(
                "invalid or duplicate agent skill {}",
                skill.id
            )));
        }
    }

    let mut symlinks = vec![HomeSymlink {
        path: "auth.json".into(),
        target: SHARED_AUTH_PATH.into(),
    }];
    symlinks.extend(BASELINE_SKILLS.iter().map(|(id, target)| HomeSymlink {
        path: format!("skills/{id}"),
        target: (*target).to_string(),
    }));

    Ok(AgentHomePlan {
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
        skills: request.skills.clone(),
    })
}

/// Render the unchanged bridge launch. Conversation resume stays on ACP.
pub fn render_launch(request: &AgentOpenRequest) -> crate::Result<AgentLaunch> {
    validate_slot(&request.options.slot)?;
    if request.options.home_directory != expected_home(&request.options.slot)
        || !valid_worktree_path(&request.options.working_directory)
    {
        return Err(AdapterError::Rejected(
            "Codex agent requires its own home and a worktree cwd".into(),
        ));
    }

    let mut env = std::collections::BTreeMap::from([(
        "CODEX_HOME".to_string(),
        request.options.home_directory.clone(),
    )]);
    if !request.options.own_credential {
        env.insert(
            "CHRONO_UPSTREAM_PLACEHOLDER_KEY".into(),
            crate::hosted::PLACEHOLDER_CREDENTIAL.into(),
        );
    }
    Ok(AgentLaunch {
        agent_id: request.options.slot.clone(),
        cwd: request.options.working_directory.clone(),
        home: request.options.home_directory.clone(),
        program: BRIDGE_PROGRAM.into(),
        // codex-acp has no resume CLI argument. The existing ACP load call
        // receives resume_conversation after the bridge starts.
        args: Vec::new(),
        env,
    })
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
