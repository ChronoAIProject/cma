use layer3_adapter::wire::WireOperation;

pub(crate) fn for_tag(tag: &str) -> Option<WireOperation> {
    Some(match tag {
        "browser" => WireOperation::Browser,
        "browser_control" => WireOperation::BrowserControl,
        "list_agent_files" => WireOperation::ListAgentFiles,
        "read_agent_file" => WireOperation::ReadAgentFile,
        "write_agent_file" => WireOperation::WriteAgentFile,
        "delete_agent_file" => WireOperation::DeleteAgentFile,
        "upload_agent_attachment" => WireOperation::UploadAgentAttachment,
        "cache_skill_archive" => WireOperation::CacheSkillArchive,
        "runtime_status" => WireOperation::RuntimeStatus,
        "tool_proxy_status" => WireOperation::ToolProxyStatus,
        "workspace_removal_support" => WireOperation::WorkspaceRemovalSupport,
        "reconcile_workspace_removal" => WireOperation::ReconcileWorkspaceRemoval,
        "install" => WireOperation::Install,
        "bootstrap_restore" => WireOperation::BootstrapRestore,
        "prepare_workspace" => WireOperation::PrepareWorkspace,
        "recover_workspace_prepare" => WireOperation::RecoverWorkspacePrepare,
        "workspace_setup" => WireOperation::WorkspaceSetup,
        "remove_workspace" => WireOperation::RemoveWorkspace,
        "prepare_agent_resources" => WireOperation::PrepareAgentResources,
        "add_worktree" => WireOperation::AddWorktree,
        "remove_worktree" => WireOperation::RemoveWorktree,
        "materialize_agent_home" => WireOperation::MaterializeAgentHome,
        "install_agent_skill" => WireOperation::InstallAgentSkill,
        "remove_agent_home" => WireOperation::RemoveAgentHome,
        "open_agent" => WireOperation::OpenAgent,
        "close_agent" => WireOperation::CloseAgent,
        "prove_agent_process_closed" => WireOperation::ProveAgentProcessClosed,
        "fence_agent_restart" => WireOperation::FenceAgentRestart,
        "capture_agent" => WireOperation::CaptureAgent,
        "checkpoint" => WireOperation::Checkpoint,
        "checkpoint_for_teardown" => WireOperation::CheckpointForTeardown,
        "resume_checkpoint_writers" => WireOperation::ResumeCheckpointWriters,
        "restore" => WireOperation::Restore,
        "credential" | "credential_for_receiver" => WireOperation::Credential,
        "resources" => WireOperation::Resources,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer3_adapter::wire::RequestFrame;
    use serde::de::{value::MapDeserializer, Error};
    use serde::Deserialize;

    #[derive(Debug)]
    struct Tags(Option<&'static [&'static str]>);

    impl std::fmt::Display for Tags {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("collect the shared request tags")
        }
    }

    impl std::error::Error for Tags {}

    impl Error for Tags {
        fn custom<T: std::fmt::Display>(_message: T) -> Self {
            Self(None)
        }

        fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
            Self(Some(expected))
        }
    }

    #[test]
    fn every_shared_request_tag_has_the_exact_http_operation() {
        // Ask the shared deserializer for its actual variants, so a new frame
        // cannot silently bypass this guard by being omitted from a test list.
        let unknown = MapDeserializer::<_, Tags>::new([("t", "unknown_control_tag")].into_iter());
        let tags = RequestFrame::deserialize(unknown).unwrap_err().0.unwrap();
        assert!(!tags.is_empty());
        for tag in tags {
            let operation = match *tag {
                "credential_for_receiver" => WireOperation::Credential,
                tag => serde_json::from_value::<WireOperation>(tag.into()).unwrap(),
            };
            assert_eq!(
                for_tag(tag),
                Some(operation),
                "unrecognized shared tag {tag}"
            );
        }
        assert_eq!(for_tag("unknown_control_tag"), None);
    }
}
