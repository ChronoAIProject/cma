use super::state::ChannelReuseSnapshot;
use layer2_core::sandbox::{Workspace, WorkspaceSource};
use layer3_adapter::{WorkspaceRequest, WorkspaceSetupRequest};

pub(crate) fn prepare(snapshot: &ChannelReuseSnapshot, workspace: &Workspace) -> WorkspaceRequest {
    let source = match &workspace.source {
        WorkspaceSource::GitRepository { url, reference } => {
            layer3_adapter::WorkspaceSource::Clone {
                url: url.clone(),
                reference: reference.clone(),
            }
        }
        WorkspaceSource::Empty => layer3_adapter::WorkspaceSource::Init,
    };
    WorkspaceRequest::with_clone_depth(
        format!("{}:workspace-prepare", snapshot.allocation.operation_id),
        workspace.id.to_string(),
        workspace.name.clone(),
        source,
        snapshot.state.settings.execution.clone_depth,
    )
}

pub(crate) fn setup(
    snapshot: &ChannelReuseSnapshot,
    workspace: &Workspace,
) -> WorkspaceSetupRequest {
    let execution = snapshot.state.settings.execution;
    WorkspaceSetupRequest::with_timeouts(
        format!("{}:workspace-setup", snapshot.allocation.operation_id),
        workspace.id.to_string(),
        workspace.name.clone(),
        workspace.profile.content.setup_commands.clone(),
        execution.command_timeout_seconds,
        execution.total_timeout_seconds,
        layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
    )
}
