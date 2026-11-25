//! Captured setup instructions and first-message projection.
use super::*;

pub(in crate::agent_lifecycle) fn setup_message(request: &AgentLifecycleCreateRequest) -> String {
    setup_message_for(request, false)
}

pub(in crate::agent_lifecycle) fn public_repository_setup_message(
    request: &AgentLifecycleCreateRequest,
) -> String {
    setup_message_for(request, true)
}

fn setup_message_for(request: &AgentLifecycleCreateRequest, public_repository: bool) -> String {
    let mut message = if public_repository {
        "Your CMA Workspace contains the selected public GitHub repository at the verified source commit. Preserve the prepared source and existing Git metadata. Briefly confirm local Workspace readiness and wait for the owner's next task.".into()
    } else if request.repository.is_some() {
        first_message(request)
    } else {
        "You are running in a CMA workspace. Preserve existing files and changes. This is local workspace readiness only; no repository or external-service task has been requested. Do not probe GitHub or other optional services, or request access merely for readiness. Briefly report readiness and wait for the user's next task. For a later user task, request access only when that task requires it.".into()
    };
    if request.machine_included {
        message.push_str(" This Sandbox also includes a shared Linux desktop on DISPLAY=:99 with Chromium already open. The owner sees and controls that same display in CMA. For requested desktop tasks, use your existing shell tools with xdotool for pointer/keyboard input and scrot plus your image-viewing tool to inspect screenshots. Preserve the existing browser profile and use the visible Chromium window. X11 window searches may not find native Wayland browser windows. Run /usr/local/bin/cma-desktop-session --browser to reopen or focus it; this command waits for a visible browser window and returns while the desktop keeps the browser open. Check its exit status before continuing; a nonzero status means startup or readiness failed. Report only observed desktop results; wait for a user task before interacting with the desktop.");
    }
    message
}

pub(in crate::agent_lifecycle) fn admitted_first_message(
    input: &crate::store::LifecycleInput,
) -> Option<String> {
    if let Some(message) = &input.request.initial_message {
        return Some(message.clone());
    }
    if matches!(
        input.behavior,
        LifecycleBehavior::RepositoryChoice | LifecycleBehavior::PublicRepository
    ) {
        return input.setup_message.clone();
    }
    input.behavior.uses_repository_profile().then(|| {
        input
            .setup_message
            .clone()
            .unwrap_or_else(|| first_message(&input.request))
    })
}

pub(in crate::agent_lifecycle) fn first_message_purpose(
    request: &AgentLifecycleCreateRequest,
) -> layer2_core::sandbox::FirstMessagePurpose {
    if request.initial_message.is_some() {
        layer2_core::sandbox::FirstMessagePurpose::Task
    } else {
        layer2_core::sandbox::FirstMessagePurpose::Setup
    }
}

// Freeze these bytes for operations admitted before setup messages were captured.
pub(in crate::agent_lifecycle) fn first_message(request: &AgentLifecycleCreateRequest) -> String {
    let mut message =
        "You are running in a CMA workspace. Preserve existing files and changes. ".to_string();
    message.push_str("For GitHub tasks, use gh api through CMA's api-github (GitHub OAuth) service. Use API source checkout instead of gh repo clone. ");
    if let Some(repository) = &request.repository {
        // JSON quotation keeps user supplied URL/ref data distinct from instructions.
        let repository = serde_json::to_string(repository).expect("repository is serializable");
        message.push_str(&format!("The user selected this GitHub repository: {repository}. Prepare its source in your current workspace. "));
        message.push_str("Honor the requested ref; when absent, read default_branch from GET /repos/{owner}/{repo}. Resolve the ref through the commits API and pin the returned commit SHA. Download /repos/{owner}/{repo}/tarball/{sha} with gh api and safely stage its source; preserve existing files, local changes and .git metadata. Initialize a local Git repository only if the destination is not already in one. Report the source commit and readiness. ");
    } else {
        message.push_str("Keep this workspace ready for the user's next task. ");
    }
    message.push_str("When the user asks to create a repository, use POST /user/repos or /orgs/{org}/repos with the intended owner and visibility; do not create one during this readiness step. Use Contents or Git Data APIs for requested publication, updating refs without force. ");
    message.push_str("If GitHub access is missing, request api-github through agent_key_scope_revision; use the same MCP tool for other missing NyxID services. Do not ask the user to paste a secret. Wait for CMA's confirmed update before retrying; no Sandbox restart is needed. Briefly report readiness or the required access.");
    message
}
