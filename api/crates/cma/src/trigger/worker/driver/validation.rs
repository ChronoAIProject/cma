use super::{resource_corrupt, LaunchResourceError};
use layer2_core::profiles::{
    content_digest, AgentProfileRevision, SandboxProfileRevision, WorkspaceProfileRevision,
};

pub(super) fn validate_profile_digests(
    sandbox: &SandboxProfileRevision,
    nested: &[WorkspaceProfileRevision],
    workspace: &WorkspaceProfileRevision,
    agent: &AgentProfileRevision,
) -> Result<(), LaunchResourceError> {
    let sandbox_valid = content_digest(&sandbox.content).ok().as_ref() == Some(&sandbox.digest);
    let nested_valid = nested
        .iter()
        .all(|revision| content_digest(&revision.content).ok().as_ref() == Some(&revision.digest));
    let workspace_valid =
        content_digest(&workspace.content).ok().as_ref() == Some(&workspace.digest);
    let agent_valid = content_digest(&agent.content).ok().as_ref() == Some(&agent.digest);
    if sandbox_valid && nested_valid && workspace_valid && agent_valid {
        Ok(())
    } else {
        Err(resource_corrupt())
    }
}
