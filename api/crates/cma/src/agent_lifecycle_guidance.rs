//! Versioned instructions captured at admission, never recomputed for old profiles.
use layer1_public::rest::{AgentLifecycleCreateRequest, AgentLifecycleProfile};
use std::sync::OnceLock;

// Retain these exact bytes while any admitted operation or prepared home uses them.
pub(crate) const GITHUB_API_V1: &str = "CMA GitHub workflow:\n\
For a GitHub clone or source-setup request, use gh api through the existing api-github OAuth service to download source and prepare a local Git workspace. Use the requested ref when supplied; otherwise GET /repos/{owner}/{repo} to read default_branch. Resolve that ref with GET /repos/{owner}/{repo}/commits/{ref} and pin its commit SHA. Download GET /repos/{owner}/{repo}/tarball/{sha} with gh api into a temporary archive; NyxID can follow the archive redirect server-side. Validate the successful response and archive before extracting. Use Git Trees/Blobs or Contents at the pinned SHA only if the archive is unavailable; handle truncated trees, base64 binary data and URL-encoded refs/paths.\n\
Inspect every archive member before extraction, strip only its common top-level directory, bound expanded size and reject absolute paths, traversal, .git entries, special files and link escapes. Stage source first, preserve executable modes, and handle safe symlinks without following them during file writes. Preserve existing files, local changes and .git metadata; reconcile conflicts before replacing files. Report unavailable submodule or LFS content without inventing it. If the destination is not already inside a Git repository, initialize a local repository there; never reinitialize, reset or replace an existing worktree. An empty remote needs only an empty local workspace. Record the source commit separately; describe this as a source checkout, without claiming imported history or working native pull/push. Local source setup does not require remote history.\n\
For user-requested repository creation use gh api --method POST /user/repos or /orgs/{org}/repos with the intended owner and visibility. For requested publication use Contents or Git Data APIs: retain the observed parent/base tree, create blobs/tree/commit and update the intended ref without force. Re-read and reconcile concurrent changes; never silently replace another commit. Do not publish or create repositories merely to prepare local source.\n\
Use gh api instead of gh repo clone or native git clone/fetch/push for this workflow. Keep gh's existing configuration; never extract credentials, request pasted secrets, or run gh auth login/setup-git. If api-github access is missing, call agent_key_scope_revision and wait for CMA's confirmed update before retrying; no Sandbox restart is needed. Respect repository permissions and API size/rate limits; report incomplete downloads accurately.";

// Versioned bytes also become part of persisted profile and Agent home identities.
pub(crate) const MEMORY_V1: &str = "CMA memory management (v1):\n\
During active work, manage memory within the Sandbox's actual effective limit. Before memory-intensive installs, builds, tests, model loading or data processing, use inexpensive local observations to check effective memory limits, total usage and available headroom. Inspect applicable cgroup limits/usage when available and machine memory availability (for example /proc/meminfo); ignore unlimited sentinels and respect the tightest applicable bound rather than assuming the host's total RAM is your allowance. If the effective limit cannot be established, use conservative settings and state the uncertainty when it affects the task. The Sandbox's memory is shared by its runtime, other Agents and processes; your process RSS alone is not the total budget.\n\
Reserve headroom for the runtime, concurrent work and temporary peaks. Prefer bounded worker/job concurrency, smaller batches, streaming or out-of-core algorithms and incremental processing. Before launching parallel tasks, account for their combined peak memory. During long memory-intensive work, make low-cost checks at sensible intervals or between stages, without constant high-frequency polling or a new background monitoring service. If headroom becomes low, reduce concurrency or stop only your own unnecessary jobs before reaching the limit. Release your own idle resources and large in-memory objects; remove only clearly disposable caches you created when useful. Deleting disk caches does not by itself prove that RAM has been freed.\n\
Never kill another Agent, the Sandbox runtime or an unowned process, delete user files or workspace state, or use privileged kernel cache-drop operations to reclaim memory. If a workload cannot safely fit, use a smaller or more memory-efficient method, or explain the constraint and ask for the necessary capacity or task adjustment. Do not blindly retry a workload that ran out of memory with the same settings. These checks apply while you are actively working; do not claim continuous monitoring outside active turns or guarantee that memory exhaustion is impossible.";

/// New admission pins this composition; older operations retain their captured bytes.
pub(crate) fn managed_v2() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{GITHUB_API_V1}\n\n{MEMORY_V1}"))
}

pub(crate) const DOCUMENTS_V1: &str = "CMA document artifacts (v1):\n\
When producing a specification for review, a reference document, a substantial explanatory document, or information the user needs to preserve, automatically publish a Markdown document to the user's personal CMA Artifact registry using publish_document from the CMA tools. Include the confirmed artifact URL in your response. Prefer a concise chat reply pointing to the durable document instead of repeating its full body. Use descriptive titles and meaningful headings; tables, checklists, code fences, images and mermaid diagrams are supported.\n\
Use update_document with the returned document ID and latest expected_revision when revising your own artifact. Keep the exact request_key and content on transport retries so one intended save never creates duplicates; use a new key for a new revision. If the owner has edited the document, preserve their changes and resolve the conflict instead of overwriting an old revision. Documents start private; the owner controls sharing and commenting in CMA. Never put credentials, private keys or tokens in a document. Publish only material authorized by the user's task. Do not claim successful persistence until the tool confirms it. If these tools are unavailable in a retained runtime, keep the source in the user's workspace and explain that publication is pending rather than inventing an artifact URL.";

/// A new immutable revision; retained v1/v2 homes keep their original bytes.
pub(crate) fn managed_v3() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{}\n\n{DOCUMENTS_V1}", managed_v2()))
}

/// Split only exact, already-versioned guidance with its original separator.
/// Similar owner-authored text is never treated as platform instructions.
pub(crate) fn split_managed(prompt: &str) -> (Option<&'static str>, &str) {
    for managed in [managed_v3(), managed_v2(), GITHUB_API_V1] {
        if prompt == managed {
            return (Some(managed), "");
        }
        if let Some(owner) = prompt
            .strip_prefix(managed)
            .and_then(|tail| tail.strip_prefix("\n\n"))
        {
            return (Some(managed), owner);
        }
    }
    (None, prompt)
}

pub(crate) fn compose_managed(managed: Option<&str>, owner: &str) -> String {
    match managed {
        Some(managed) if owner.is_empty() => managed.to_owned(),
        Some(managed) => format!("{managed}\n\n{owner}"),
        None => owner.to_owned(),
    }
}

/// Owner edits cannot remove CMA's deployment-managed instructions. The exact
/// desired composition is persisted before dispatch, never recomputed on replay.
pub(crate) struct ManagedInstructionPolicy;
impl layer2_core::sandbox::AgentInstructionPolicy for ManagedInstructionPolicy {
    fn owner_prompt(&self, agent: &layer2_core::sandbox::Agent) -> String {
        split_managed(agent.system_prompt()).1.to_owned()
    }
    fn compose(&self, agent: &layer2_core::sandbox::Agent, owner: &str) -> String {
        let managed = (agent.conversation.managed_guidance_revision >= 1)
            .then_some(managed_v3())
            .or_else(|| split_managed(agent.system_prompt()).0)
            .or_else(|| split_managed(&agent.profile.content.system_prompt).0);
        compose_managed(managed, owner)
    }
}

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
    prompt.is_empty() || prompt == GITHUB_API_V1 || prompt == managed_v2() || prompt == managed_v3()
}

#[cfg(test)]
#[path = "agent_lifecycle_guidance_tests.rs"]
mod tests;
