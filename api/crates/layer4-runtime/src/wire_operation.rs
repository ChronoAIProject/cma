use layer3_adapter::wire::WireOperation;

pub(crate) fn for_tag(tag: &str) -> Option<WireOperation> {
    Some(match tag {
        "cache_skill_archive" => WireOperation::CacheSkillArchive,
        "runtime_status" => WireOperation::RuntimeStatus,
        "tool_proxy_status" => WireOperation::ToolProxyStatus,
        "workspace_removal_support" => WireOperation::WorkspaceRemovalSupport,
        "reconcile_workspace_removal" => WireOperation::ReconcileWorkspaceRemoval,
        "install" => WireOperation::Install,
        "bootstrap_restore" => WireOperation::BootstrapRestore,
        "prepare_workspace" => WireOperation::PrepareWorkspace,
        "workspace_setup" => WireOperation::WorkspaceSetup,
        "remove_workspace" => WireOperation::RemoveWorkspace,
        "add_worktree" => WireOperation::AddWorktree,
        "remove_worktree" => WireOperation::RemoveWorktree,
        "materialize_agent_home" => WireOperation::MaterializeAgentHome,
        "install_agent_skill" => WireOperation::InstallAgentSkill,
        "remove_agent_home" => WireOperation::RemoveAgentHome,
        "open_agent" => WireOperation::OpenAgent,
        "close_agent" => WireOperation::CloseAgent,
        "capture_agent" => WireOperation::CaptureAgent,
        "checkpoint" => WireOperation::Checkpoint,
        "checkpoint_for_teardown" => WireOperation::CheckpointForTeardown,
        "resume_checkpoint_writers" => WireOperation::ResumeCheckpointWriters,
        "restore" => WireOperation::Restore,
        "credential" => WireOperation::Credential,
        "resources" => WireOperation::Resources,
        _ => return None,
    })
}
