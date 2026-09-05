use layer3_adapter::wire::WireOperation;

pub(crate) fn for_tag(tag: &str) -> Option<WireOperation> {
    Some(match tag {
        "install" => WireOperation::Install,
        "prepare_workspace" => WireOperation::PrepareWorkspace,
        "workspace_setup" => WireOperation::WorkspaceSetup,
        "remove_workspace" => WireOperation::RemoveWorkspace,
        "add_worktree" => WireOperation::AddWorktree,
        "remove_worktree" => WireOperation::RemoveWorktree,
        "materialize_agent_home" => WireOperation::MaterializeAgentHome,
        "remove_agent_home" => WireOperation::RemoveAgentHome,
        "open_agent" => WireOperation::OpenAgent,
        "close_agent" => WireOperation::CloseAgent,
        "capture_agent" => WireOperation::CaptureAgent,
        "checkpoint" => WireOperation::Checkpoint,
        "restore" => WireOperation::Restore,
        "credential" => WireOperation::Credential,
        "resources" => WireOperation::Resources,
        _ => return None,
    })
}
