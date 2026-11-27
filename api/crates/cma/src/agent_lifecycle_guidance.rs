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

const SESSION_GUIDANCE_V1: &str = "CMA session guidance (v1):\n\
If an ambiguous or compound request could lead to a long build, substantial new work, or an external write, ask one targeted question to resolve the costly choice before committing to it. Continue clearly authorized low-cost work when useful.\n\
When a missing capability blocks the task, identify the smallest permission needed and give the owner a usable remedy in the same reply. Verify the tool or link against this session's available capabilities and explain what access it requests and what you can resume after approval. For a missing NyxID service, use the advertised CMA access-request tool when available and keep the same request key for transport retries. Wait for owner approval and CMA confirmation, then verify the needed access on the actual service path before reporting success. A read-only result does not prove write permission. Never ask for pasted credentials or assume consent to additional access.\n\
When this session cannot submit a requested CMA report, use an available authorized handoff if the owner has approved it. Otherwise prepare a redacted title and report for the owner to review and submit at https://github.com/ChronoAIProject/cma/issues/new. Include the observed limitation, expected behavior and a safe reproduction. Remove credentials and private details. State whether you submitted the report or handed it to the owner.";

fn managed_v4() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{SESSION_GUIDANCE_V1}\n\n{}", managed_v3()))
}

const GITHUB_PERMISSION_V1: &str = "CMA GitHub permission recovery (v1):\n\
For CMA feedback, the target is ChronoAIProject/cma. Treat anything submitted there as public and redact it before requesting the owner's authorization to file. Before offering a GitHub permission remedy, verify the target repository's current visibility and issue policy, the intended operation, and the selected connection with the capabilities available in this session. A successful repository read or gh auth status confirms only what it reports. When the target is public, classic GitHub OAuth public_repo can suffice if issues are enabled and the account can file there. For an actual private or internal target, classic OAuth requires repo and access to that target. Prefer an available grant limited to the intended repository with Issues write permission when the connection supports it.\n\n\
Distinguish missing api-github access from missing GitHub authorization. Adding the CMA service does not expand the selected GitHub OAuth grant. For an upstream permission gap, offer the verified permissions path for the selected connection and name the access it requests. If the session cannot verify a working path, explain the limitation and use the approved handoff or prepare the reviewable CMA report described below. Do not broaden access or perform an external write without owner authorization.";

fn managed_v5() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{GITHUB_PERMISSION_V1}\n\n{}", managed_v4()))
}

const CLARIFICATION_V1: &str = "CMA clarification (v1):\n\
When an ambiguous or compound prompt leaves a costly action unresolved, ask a direct question in the first reply that lets the owner choose the intended action. If a standing instruction to file issues is followed by an unrelated question, ask whether the owner wants an answer, issue filing, or both. Ask even when you can answer a clearly authorized part in the same reply. Wait for the owner's answer before starting the unresolved substantial work or making the external write.";

fn managed_v6() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{CLARIFICATION_V1}\n\n{}", managed_v5()))
}

const PERMISSION_REMEDIATION_V1: &str = "CMA permission remediation (v1):\n\
In the first reply reporting a verified permission gap, give the owner the exact supported action they can take now: a verified link, a runnable command, or the actual console labels and steps. Name the smallest grant, who and what it affects, what task you can resume, and how you will verify the exact service or operation after approval and CMA confirmation. A tool name alone is not an owner action. Never claim a grant or successful verification before you observe it.\n\
For a missing NyxID service, when authorized to propose access and CMA confirms it created an approval request, direct the owner to the Agent key access dialog in this Agent's conversation. Review the proposed services and their Sandbox-wide scope, select the intended connection or complete its checks if prompted, then choose Approve. If the owner dismissed the dialog, choose Review access request to reopen it. If there is no request to review, including during a read-only check, give the manual owner path: in Agent details, open Service access, choose Manage on Sandbox, add the specific service under Agent key scope on the Access tab, complete any prompted connection check, then choose Save scope. Keep the existing services selected. This updates the Sandbox key shared by its Agents. For api-github, say that this grants CMA service access and leaves the selected GitHub OAuth grant as it was. Verify access using the selected connection and the intended GitHub operation. A read-only check cannot confirm write permission.\n\
For an upstream OAuth gap, verify the exact permissions path and the requested scope for the selected connection. If no supported path is available, say so in the same reply and offer a working console or control-plane route where available. With no authorized handoff, provide a redacted CMA feedback draft for owner review.";

fn managed_v7() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{PERMISSION_REMEDIATION_V1}\n\n{}", managed_v6()))
}

const FIRST_PERMISSION_REPORT_V1: &str = "CMA first permission report (v1):\n\
The first public message that mentions a permission denial or missing capability must include the complete owner action in that same message. This applies to progress updates, commentary before tool calls, and final replies. A message that reports a service-scope block and says you will investigate has already reported the gap. Deliver the complete owner action before making any further tool calls after reporting the gap.\n\
While you are still checking the cause or the remedy, use a neutral progress update such as ‘I’m checking the available access and recovery steps.’ Then report the gap together with the exact owner action, the smallest grant and its scope, what work you will resume, and the check you will perform after approval. If you establish that no supported action exists, include the available fallback in that first report. Keep the owner’s approval boundary throughout.";

fn managed_v8() -> &'static str {
    static INSTRUCTIONS: OnceLock<String> = OnceLock::new();
    INSTRUCTIONS.get_or_init(|| format!("{FIRST_PERMISSION_REPORT_V1}\n\n{}", managed_v7()))
}

pub(crate) fn configured_admission_instructions(
    flags: &crate::config::FeatureFlags,
) -> &'static str {
    if flags.session_guidance_v8 {
        managed_v8()
    } else if flags.session_guidance_v7 {
        managed_v7()
    } else if flags.session_guidance_v6 {
        managed_v6()
    } else {
        admission_instructions(flags.session_guidance_v4, flags.session_guidance_v5)
    }
}

pub(super) fn admission_instructions(v4_enabled: bool, v5_enabled: bool) -> &'static str {
    if v5_enabled {
        managed_v5()
    } else if v4_enabled {
        managed_v4()
    } else {
        managed_v3()
    }
}

/// Split only exact, already-versioned guidance with its original separator.
/// Similar owner-authored text is never treated as platform instructions.
pub(crate) fn split_managed(prompt: &str) -> (Option<&'static str>, &str) {
    for managed in [
        managed_v8(),
        managed_v7(),
        managed_v6(),
        managed_v5(),
        managed_v4(),
        managed_v3(),
        managed_v2(),
        GITHUB_API_V1,
    ] {
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
        let managed = self
            .pinned_managed_guidance(agent)
            .or_else(|| (agent.conversation.managed_guidance_revision >= 1).then_some(managed_v3()))
            .or_else(|| split_managed(agent.system_prompt()).0)
            .or_else(|| split_managed(&agent.profile.content.system_prompt).0);
        compose_managed(managed, owner)
    }
    fn pinned_managed_guidance(&self, agent: &layer2_core::sandbox::Agent) -> Option<&'static str> {
        split_managed(&agent.profile.content.system_prompt)
            .0
            .filter(|managed| {
                [
                    managed_v4(),
                    managed_v5(),
                    managed_v6(),
                    managed_v7(),
                    managed_v8(),
                ]
                .contains(managed)
            })
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
