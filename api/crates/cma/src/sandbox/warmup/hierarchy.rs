//! Prepare stable workspace, worktree, home and idle conversation identities before claim.

use layer2_core::profiles::WorkspaceLocation;
use layer2_core::sandbox::{WarmSandbox, WarmSandboxState};
use layer2_core::SubjectId;
use layer3_adapter::{
    AgentHomeRequest, AgentOpenRequest, RuntimeHandle, SandboxAdapter, WorkspaceRequest,
    WorkspaceSetupRequest, WorkspaceSource, WorktreeRequest,
};

use crate::sandbox::agent_skills::AgentSkillResolver;
use crate::sandbox::runtime_mapping::profile_runtime_options;

pub(in crate::sandbox) async fn prepare_step(
    adapter: &dyn SandboxAdapter,
    skills: &dyn AgentSkillResolver,
    owner: &SubjectId,
    record: &mut WarmSandbox,
) -> layer3_adapter::Result<()> {
    let handle = RuntimeHandle::new(
        &record
            .runtime
            .as_ref()
            .ok_or_else(|| invalid("warm runtime is missing"))?
            .runtime_id,
    );
    let execution = record.plan.execution;
    for (workspace, plan) in record.workspaces.iter_mut().zip(&record.plan.workspaces) {
        let name = plan
            .profile
            .content
            .workspace_name()
            .map_err(|_| invalid("invalid warm workspace name"))?;
        if workspace.resolved_commit.is_none() {
            let source = match &plan.profile.content.location {
                WorkspaceLocation::RepositorySnapshot { .. }
                | WorkspaceLocation::PublicRepositorySnapshot { .. } => {
                    return Err(invalid(
                        "repository snapshots require owner credentials after admission",
                    ))
                }
                WorkspaceLocation::GitRepository { url, reference } => WorkspaceSource::Clone {
                    url: url.clone(),
                    reference: reference.clone(),
                },
                WorkspaceLocation::Directory { .. } => WorkspaceSource::Init,
            };
            let request = WorkspaceRequest::with_clone_depth(
                format!("workspace:{}:prepare:v1", workspace.id),
                workspace.id.to_string(),
                name,
                source,
                execution.clone_depth,
            );
            let receipt = adapter.prepare_workspace(&handle, &request).await?;
            workspace.resolved_commit = Some(receipt.resolved_commit);
            return Ok(());
        }
        if !workspace.setup_ready {
            let request = WorkspaceSetupRequest::with_timeouts(
                format!("workspace:{}:setup:v1", workspace.id),
                workspace.id.to_string(),
                name,
                plan.profile.content.preparation_commands(),
                execution.command_timeout_seconds,
                execution.total_timeout_seconds,
                layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
            );
            adapter.run_workspace_setup(&handle, &request).await?;
            workspace.setup_ready = true;
            return Ok(());
        }
        for (index, (agent, profile)) in workspace.agents.iter_mut().zip(&plan.agents).enumerate() {
            if agent.conversation_id.is_some() {
                continue;
            }
            if agent.skills.is_none() {
                agent.skills = Some(
                    skills
                        .resolve_warm(owner, &profile.content.skills)
                        .await
                        .map_err(|_| invalid("public skill preparation failed"))?,
                );
                return Ok(());
            }
            let worktree = format!("wt-{}", index + 1);
            let request = WorktreeRequest::new(
                format!("worktree:{name}:{worktree}:add:v1"),
                name.clone(),
                worktree.clone(),
                workspace
                    .resolved_commit
                    .clone()
                    .expect("prepared workspace"),
            );
            adapter.add_worktree(&handle, 1, &request).await?;
            let material = agent.skills.as_ref().expect("persisted skill selection");
            for archive in skills
                .warm_archives(owner, material)
                .await
                .map_err(|_| invalid("public skill archive preparation failed"))?
            {
                adapter.cache_skill_archive(&handle, &archive).await?;
            }
            let options = profile_runtime_options(
                &profile.content,
                &record.plan.profile.content,
                &agent.id,
                &format!("/home/cma/{name}/.worktrees/{worktree}"),
            );
            let request = AgentHomeRequest::new(
                format!("agent:{}:home:create:v1", agent.id),
                options.clone(),
                profile.content.system_prompt.clone(),
                skills
                    .runtime_skills(material)
                    .map_err(|_| invalid("public skill references are invalid"))?,
            );
            adapter.materialize_agent_home(&handle, 1, &request).await?;
            let request =
                AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None);
            let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
            let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
            let result = adapter.open_agent(&handle, 1, &request, sender).await;
            drain.abort();
            agent.conversation_id = Some(
                result?
                    .conversation_id
                    .ok_or_else(|| invalid("warm Agent has no conversation"))?,
            );
            return Ok(());
        }
    }
    if record
        .workspaces
        .iter()
        .all(|workspace| workspace.is_ready())
    {
        record.state = WarmSandboxState::Ready;
    }
    Ok(())
}

fn invalid(message: &str) -> layer3_adapter::AdapterError {
    layer3_adapter::AdapterError::Rejected(message.into())
}

/// Replaying the exact open and session identity proves the idle process still lives.
/// A missing process/session fails this check; it must never create a replacement.
pub(super) async fn verify_sessions(
    adapter: &dyn SandboxAdapter,
    record: &WarmSandbox,
) -> layer3_adapter::Result<()> {
    let handle = RuntimeHandle::new(
        &record
            .runtime
            .as_ref()
            .ok_or_else(|| invalid("warm runtime is missing"))?
            .runtime_id,
    );
    for (workspace, plan) in record.workspaces.iter().zip(&record.plan.workspaces) {
        let name = plan
            .profile
            .content
            .workspace_name()
            .map_err(|_| invalid("invalid warm workspace name"))?;
        for (index, (agent, profile)) in workspace.agents.iter().zip(&plan.agents).enumerate() {
            let conversation = agent
                .conversation_id
                .as_deref()
                .ok_or_else(|| invalid("warm Agent conversation is missing"))?;
            let options = profile_runtime_options(
                &profile.content,
                &record.plan.profile.content,
                &agent.id,
                &format!("/home/cma/{name}/.worktrees/wt-{}", index + 1),
            );
            let request =
                AgentOpenRequest::new(format!("agent:{}:open:v1", agent.id), options, None);
            let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
            let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
            let result = adapter
                .reconnect_agent(&handle, 1, &request, conversation, sender)
                .await;
            drain.abort();
            if result?.conversation_id.as_deref() != Some(conversation) {
                return Err(invalid("warm Agent conversation changed"));
            }
        }
    }
    Ok(())
}
