//! Versioned instructions captured at admission, never recomputed for old profiles.
use layer1_public::rest::{AgentLifecycleCreateRequest, AgentLifecycleProfile};

// Retain these exact bytes while any admitted operation or prepared home uses them.
pub(crate) const GITHUB_API_V1: &str = "CMA GitHub workflow:\n\
For a GitHub clone or source-setup request, use gh api through the existing api-github OAuth service to download source and prepare a local Git workspace. Use the requested ref when supplied; otherwise GET /repos/{owner}/{repo} to read default_branch. Resolve that ref with GET /repos/{owner}/{repo}/commits/{ref} and pin its commit SHA. Download GET /repos/{owner}/{repo}/tarball/{sha} with gh api into a temporary archive; NyxID can follow the archive redirect server-side. Validate the successful response and archive before extracting. Use Git Trees/Blobs or Contents at the pinned SHA only if the archive is unavailable; handle truncated trees, base64 binary data and URL-encoded refs/paths.\n\
Inspect every archive member before extraction, strip only its common top-level directory, bound expanded size and reject absolute paths, traversal, .git entries, special files and link escapes. Stage source first, preserve executable modes, and handle safe symlinks without following them during file writes. Preserve existing files, local changes and .git metadata; reconcile conflicts before replacing files. Report unavailable submodule or LFS content without inventing it. If the destination is not already inside a Git repository, initialize a local repository there; never reinitialize, reset or replace an existing worktree. An empty remote needs only an empty local workspace. Record the source commit separately; describe this as a source checkout, without claiming imported history or working native pull/push. Local source setup does not require remote history.\n\
For user-requested repository creation use gh api --method POST /user/repos or /orgs/{org}/repos with the intended owner and visibility. For requested publication use Contents or Git Data APIs: retain the observed parent/base tree, create blobs/tree/commit and update the intended ref without force. Re-read and reconcile concurrent changes; never silently replace another commit. Do not publish or create repositories merely to prepare local source.\n\
Use gh api instead of gh repo clone or native git clone/fetch/push for this workflow. Keep gh's existing configuration; never extract credentials, request pasted secrets, or run gh auth login/setup-git. If api-github access is missing, call agent_key_scope_revision and wait for CMA's confirmed update before retrying; no Sandbox restart is needed. Respect repository permissions and API size/rate limits; report incomplete downloads accurately.";

pub(crate) fn profile_with_instructions(
    mut profile: AgentLifecycleProfile,
    instructions: Option<&str>,
) -> AgentLifecycleProfile {
    if let Some(instructions) = instructions {
        profile.system_prompt = Some(match profile.system_prompt {
            Some(prompt) if !prompt.is_empty() => format!("{instructions}\n\n{prompt}"),
            _ => instructions.to_owned(),
        });
    }
    profile
}

pub(super) fn provisioning_request(
    request: &AgentLifecycleCreateRequest,
    instructions: Option<&str>,
) -> AgentLifecycleCreateRequest {
    let mut request = request.clone();
    if instructions.is_some() {
        request.profile = Some(profile_with_instructions(
            request.profile.unwrap_or_default(),
            instructions,
        ));
    }
    request
}

/// Historical preparation had no instructions; new preparation pins this revision.
pub(crate) fn is_prepared_prompt(prompt: &str) -> bool {
    prompt.is_empty() || prompt == GITHUB_API_V1
}

#[cfg(test)]
#[path = "agent_lifecycle_guidance_tests.rs"]
mod tests;
