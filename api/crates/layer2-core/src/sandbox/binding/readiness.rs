//! Read-only validation of the resources retained by a ready profile binding.
use super::{BindingError, BindingState, ProviderBinding};
use crate::sandbox::store::{SandboxStore, WorkspaceStore};
use crate::sandbox::{SandboxStatus, WorkspaceStatus};

pub(super) async fn validate<S>(store: &S, binding: &ProviderBinding) -> Result<(), BindingError>
where
    S: SandboxStore + WorkspaceStore + ?Sized,
{
    if binding.state != BindingState::Ready
        || binding.existing_agent.is_some()
        || binding.profile.environment.is_none()
    {
        return Ok(());
    }
    // Both reads must succeed before absence is classified. A storage outage is
    // not evidence that an owner deleted a resource.
    let (sandbox, workspace) = tokio::try_join!(
        store.get_sandbox(&binding.sandbox_id),
        store.get_workspace(&binding.workspace_id),
    )?;
    let (Some(sandbox), Some(workspace)) = (sandbox, workspace) else {
        return Err(BindingError::ResourcesUnavailable);
    };
    if sandbox.id != binding.sandbox_id
        || sandbox.owner != binding.owner
        || workspace.id != binding.workspace_id
        || workspace.owner != binding.owner
        || workspace.sandbox_id != binding.sandbox_id
    {
        return Err(BindingError::Corrupt(
            "binding backing resource ownership is inconsistent".into(),
        ));
    }
    // Paused, recovering and failed resources retain their original state and
    // recovery policy. Only explicit removal is a permanent readiness refusal.
    if matches!(
        sandbox.status,
        SandboxStatus::Retiring | SandboxStatus::Retired
    ) || matches!(
        workspace.status,
        WorkspaceStatus::Removing | WorkspaceStatus::Removed
    ) {
        return Err(BindingError::ResourcesUnavailable);
    }
    Ok(())
}
