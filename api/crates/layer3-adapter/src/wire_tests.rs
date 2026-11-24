use std::collections::BTreeMap;

use super::*;
use crate::{
    AgentCloseRequest, AgentHomeRemoveRequest, AgentSkill, CommandReceipt, HomeFile, HomeSymlink,
    PermissionLevel, ReasoningEffort, RuntimeCredential, RuntimeOptions, WorkspaceRemoveReceipt,
    WorkspaceRemoveRequest, WorkspaceSource, WorktreeRemoveReceipt, WriterState,
};

fn options() -> RuntimeOptions {
    RuntimeOptions {
        model: Some("gpt-test".into()),
        reasoning_effort: ReasoningEffort::High,
        permission: PermissionLevel::Autonomous,
        own_credential: false,
        isolated_agent: false,
        working_directory: "/home/cma/repo/.worktrees/wt-1".into(),
        home_directory: "/home/cma/.cma/agents/agent-1/home".into(),
        slot: AgentSlot("agent-1".into()),
    }
}

fn sha() -> String {
    "a".repeat(64)
}

fn fenced<T>(request: T) -> FencedRequest<T> {
    FencedRequest::new(7, request)
}

fn fenced_receipt<T>(receipt: T) -> FencedReceipt<T> {
    FencedReceipt::new(7, receipt)
}

#[test]
fn every_contract_3_request_round_trips_with_its_exact_tag() {
    let frames = vec![
        RequestFrame::Install(InstallRequest::bounded(
            "install-1",
            vec!["make tools".into()],
            600,
            32_768,
        )),
        RequestFrame::BootstrapRestore(RestoreBootstrapRequest::new(
            "restore-bootstrap-1",
            "sandbox-1",
            RestoreRequest {
                object_reference: format!("home-{}.cma", sha()),
                sha256: sha(),
            },
        )),
        RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks-repo:prepare:v1",
            "wks-repo",
            "repo",
            WorkspaceSource::Clone {
                url: "https://github.com/example/repo".into(),
                reference: Some("main".into()),
            },
        )),
        RequestFrame::WorkspaceSetup(WorkspaceSetupRequest::new(
            "workspace:wks-repo:setup:v1",
            "wks-repo",
            "repo",
            vec!["make setup".into()],
        )),
        RequestFrame::RemoveWorkspace(WorkspaceRemoveRequest::new(
            "workspace:wks-repo:remove:v1",
            "wks-repo",
            "repo",
        )),
        RequestFrame::AddWorktree(fenced(WorktreeRequest::new(
            "worktree:repo:wt-1:add:v1",
            "repo",
            "wt-1",
            "abc123",
        ))),
        RequestFrame::RemoveWorktree(fenced(WorktreeRef::new(
            "worktree:repo:wt-1:remove:v1",
            "repo",
            "wt-1",
        ))),
        RequestFrame::MaterializeAgentHome(fenced(AgentHomePlan {
            operation_id: "agent:agent-1:home:create:v1".into(),
            request_sha256: sha(),
            agent_id: AgentSlot("agent-1".into()),
            files: vec![HomeFile {
                path: "AGENTS.md".into(),
                bytes: b"prompt".to_vec(),
            }],
            symlinks: vec![HomeSymlink {
                path: "auth.json".into(),
                target: "/shared/auth.json".into(),
            }],
            skills: vec![AgentSkill {
                id: "review".into(),
                name: None,
                version: Some("1".into()),
                archive_sha256: sha(),
                source_url: "https://ornn.example/review.tgz".into(),
            }],
        })),
        RequestFrame::RemoveAgentHome(fenced(AgentHomeRemoveRequest::new(
            "agent:agent-1:home:remove:v1",
            AgentSlot("agent-1".into()),
        ))),
        RequestFrame::OpenAgent(fenced(AgentLaunch {
            operation_id: "agent:agent-1:open:v1".into(),
            request_sha256: sha(),
            agent_id: AgentSlot("agent-1".into()),
            cwd: options().working_directory,
            home: options().home_directory,
            program: "bridge".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        })),
        RequestFrame::CloseAgent(fenced(AgentCloseRequest::new(
            "agent:agent-1:close:v1",
            AgentSlot("agent-1".into()),
        ))),
        RequestFrame::CaptureAgent(AgentRef {
            agent_id: AgentSlot("agent-1".into()),
        }),
        RequestFrame::Checkpoint(CheckpointRequest {
            epoch: 9,
            exclusions: vec![".cache".into()],
        }),
        RequestFrame::CheckpointForTeardown(CheckpointHoldRequest {
            checkpoint: CheckpointRequest {
                epoch: 10,
                exclusions: vec![".cache".into()],
            },
            writer_resume_token: sha(),
        }),
        RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
            writer_resume_token: sha(),
        }),
        RequestFrame::Restore(RestoreRequest {
            object_reference: "object://checkpoint".into(),
            sha256: sha(),
        }),
        RequestFrame::Credential(CredentialFrame {
            operation_id: "credential-1".into(),
            value: RuntimeCredential::new("secret"),
            custody_generation: Some(7),
            github_route: None,
        }),
        RequestFrame::Resources,
    ];
    let tags = [
        "install",
        "bootstrap_restore",
        "prepare_workspace",
        "workspace_setup",
        "remove_workspace",
        "add_worktree",
        "remove_worktree",
        "materialize_agent_home",
        "remove_agent_home",
        "open_agent",
        "close_agent",
        "capture_agent",
        "checkpoint",
        "checkpoint_for_teardown",
        "resume_checkpoint_writers",
        "restore",
        "credential",
        "resources",
    ];

    for (frame, tag) in frames.into_iter().zip(tags) {
        let json = serde_json::to_value(&frame).expect("request serializes");
        assert_eq!(json["t"], tag);
        if tag == "install" {
            assert_eq!(json["operation_id"], "install-1");
            assert_eq!(json["request_sha256"].as_str().unwrap().len(), 64);
            assert_eq!(json["timeout_s"], 600);
            assert_eq!(json["output_limit"], 32_768);
            assert!(json.get("timeout_seconds").is_none());
            assert!(json.get("output_limit_bytes").is_none());
        }
        if tag == "add_worktree" {
            assert_eq!(json["authority_fence"], 7);
            assert_eq!(json["request"]["workspace"], "repo");
        }
        let decoded: RequestFrame = serde_json::from_value(json).expect("request deserializes");
        assert_eq!(decoded, frame);
    }
    assert!(serde_json::from_str::<RequestFrame>(r#"{"t":"unknown"}"#).is_err());
}

#[test]
fn every_contract_3_reply_round_trips_and_failures_stay_typed() {
    let command = CommandReceipt {
        index: 0,
        exit_code: 0,
        stdout_sha256: sha(),
        stderr_sha256: sha(),
    };
    let replies = vec![
        ResponseFrame::Install(InstallReceipt {
            operation_id: "install-1".into(),
            request_sha256: sha(),
            commands: vec![command.clone()],
        }),
        ResponseFrame::BootstrapRestore(RestoreBootstrapReceipt {
            operation_id: "restore-bootstrap-1".into(),
            request_sha256: sha(),
            restore_request_sha256: sha(),
            installation_inherited: true,
        }),
        ResponseFrame::PrepareWorkspace(WorkspaceReceipt {
            operation_id: "workspace:wks-repo:prepare:v1".into(),
            request_sha256: sha(),
            workspace_id: "wks-repo".into(),
            name: "repo".into(),
            path: "/home/cma/repo".into(),
            resolved_commit: sha(),
            source_commit: None,
        }),
        ResponseFrame::WorkspaceSetup(SetupReceipt {
            operation_id: "workspace:wks-repo:setup:v1".into(),
            request_sha256: sha(),
            workspace_id: "wks-repo".into(),
            commands: vec![command],
        }),
        ResponseFrame::RemoveWorkspace(WorkspaceRemoveReceipt {
            operation_id: "workspace:wks-repo:remove:v1".into(),
            request_sha256: sha(),
            workspace_id: "wks-repo".into(),
        }),
        ResponseFrame::AddWorktree(fenced_receipt(WorktreeReceipt {
            operation_id: "worktree:repo:wt-1:add:v1".into(),
            request_sha256: sha(),
            workspace: "repo".into(),
            name: "wt-1".into(),
            path: "/home/cma/repo/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: sha(),
        })),
        ResponseFrame::RemoveWorktree(fenced_receipt(WorktreeRemoveReceipt {
            operation_id: "worktree:repo:wt-1:remove:v1".into(),
            request_sha256: sha(),
            workspace: "repo".into(),
            name: "wt-1".into(),
        })),
        ResponseFrame::MaterializeAgentHome(fenced_receipt(AgentHomeReceipt {
            operation_id: "agent:agent-1:home:create:v1".into(),
            request_sha256: sha(),
            agent_id: AgentSlot("agent-1".into()),
            path: "/home/cma/.cma/agents/agent-1/home".into(),
        })),
        ResponseFrame::RemoveAgentHome(fenced_receipt(AgentHomeRemoveReceipt {
            operation_id: "agent:agent-1:home:remove:v1".into(),
            request_sha256: sha(),
            agent_id: AgentSlot("agent-1".into()),
        })),
        ResponseFrame::OpenAgent(fenced_receipt(AgentOpenReceipt {
            operation_id: "agent:agent-1:open:v1".into(),
            request_sha256: sha(),
            agent_id: AgentSlot("agent-1".into()),
            cwd: "/home/cma/repo/.worktrees/wt-1".into(),
            home: "/home/cma/.cma/agents/agent-1/home".into(),
            conversation_id: Some("conversation-1".into()),
        })),
        ResponseFrame::CloseAgent(fenced_receipt(AgentCloseReceipt {
            operation_id: "agent:agent-1:close:v1".into(),
            request_sha256: sha(),
            agent_id: AgentSlot("agent-1".into()),
        })),
        ResponseFrame::CaptureAgent(ConversationSnapshot {
            conversation_id: "conversation-1".into(),
            archive_sha256: sha(),
            archive: vec![1, 2, 3],
        }),
        ResponseFrame::Checkpoint(CheckpointReceipt {
            epoch: 9,
            object_reference: "object://checkpoint".into(),
            sha256: sha(),
            writers: WriterState::Resumed,
        }),
        ResponseFrame::CheckpointForTeardown(CheckpointHoldReceipt {
            checkpoint: CheckpointReceipt {
                epoch: 10,
                object_reference: "object://held-checkpoint".into(),
                sha256: sha(),
                writers: WriterState::Held,
            },
            writer_resume_token: sha(),
        }),
        ResponseFrame::ResumeCheckpointWriters(CheckpointWriterResumeReceipt {
            writer_resume_token: sha(),
            writers_resumed: true,
        }),
        ResponseFrame::Restore(Acknowledgement::default()),
        ResponseFrame::Credential(CredentialReceipt {
            operation_id: "credential-1".into(),
            installed: true,
        }),
        ResponseFrame::Resources(ResourceReading::default()),
        ResponseFrame::Failure(WireFailure {
            operation: WireOperation::Install,
            code: "command_failed".into(),
            detail: "command 0 exited 1".into(),
            retryable: false,
        }),
    ];

    for reply in replies {
        let json = serde_json::to_vec(&reply).expect("reply serializes");
        let decoded: ResponseFrame = serde_json::from_slice(&json).expect("reply deserializes");
        assert_eq!(decoded, reply);
    }
}

#[test]
fn credential_debug_is_redacted_while_the_wire_retains_its_value() {
    let frame = CredentialFrame {
        operation_id: "credential-1".into(),
        value: RuntimeCredential::new("never-log-this"),
        custody_generation: Some(4),
        github_route: None,
    };
    assert!(!format!("{frame:?}").contains("never-log-this"));
    let json = serde_json::to_value(RequestFrame::Credential(frame)).expect("serializes");
    assert_eq!(json["value"], "never-log-this");
    assert_eq!(json["custody_generation"], 4);
}

#[test]
fn resources_keep_the_existing_supervisor_field_names_and_omit_absences() {
    let json = serde_json::to_value(ResponseFrame::Resources(ResourceReading {
        uptime_ms: 42,
        cpu_usage_usec: Some(7),
        ..ResourceReading::default()
    }))
    .expect("resource reply serializes");

    assert_eq!(json["t"], "resources");
    assert_eq!(json["uptimeMs"], 42);
    assert_eq!(json["cpuUsageUsec"], 7);
    assert!(json.get("uptime_ms").is_none());
    assert!(json.get("cpu_usage_usec").is_none());
    assert!(json.get("memoryBytesUsed").is_none());
}
