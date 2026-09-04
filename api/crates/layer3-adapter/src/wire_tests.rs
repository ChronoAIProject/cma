use std::collections::BTreeMap;

use super::*;
use crate::{
    AgentSkill, CommandReceipt, HomeFile, HomeSymlink, PermissionLevel, ReasoningEffort,
    RuntimeCredential, RuntimeOptions, WorkspaceSource, WriterState,
};

fn options() -> RuntimeOptions {
    RuntimeOptions {
        model: Some("gpt-test".into()),
        reasoning_effort: ReasoningEffort::High,
        permission: PermissionLevel::Autonomous,
        own_credential: false,
        working_directory: "/home/cma/repo/.worktrees/wt-1".into(),
        home_directory: "/home/cma/.cma/agents/agent-1/home".into(),
        slot: AgentSlot("agent-1".into()),
    }
}

fn sha() -> String {
    "a".repeat(64)
}

#[test]
fn every_contract_2_request_round_trips_with_its_exact_tag() {
    let frames = vec![
        RequestFrame::Install(InstallRequest::bounded(
            "install-1",
            vec!["make tools".into()],
            600,
            32_768,
        )),
        RequestFrame::PrepareWorkspace(WorkspaceRequest {
            name: "repo".into(),
            source: WorkspaceSource::Clone {
                url: "https://github.com/example/repo".into(),
                reference: Some("main".into()),
            },
        }),
        RequestFrame::WorkspaceSetup(WorkspaceSetupRequest {
            name: "repo".into(),
            commands: vec!["make setup".into()],
        }),
        RequestFrame::RemoveWorkspace(WorkspaceRef {
            name: "repo".into(),
        }),
        RequestFrame::AddWorktree(WorktreeRequest {
            workspace: "repo".into(),
            name: "wt-1".into(),
            base: "abc123".into(),
        }),
        RequestFrame::RemoveWorktree(WorktreeRef {
            workspace: "repo".into(),
            name: "wt-1".into(),
        }),
        RequestFrame::MaterializeAgentHome(AgentHomePlan {
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
                version: Some("1".into()),
                archive_sha256: sha(),
                source_url: "https://ornn.example/review.tgz".into(),
            }],
        }),
        RequestFrame::OpenAgent(AgentLaunch {
            agent_id: AgentSlot("agent-1".into()),
            cwd: options().working_directory,
            home: options().home_directory,
            program: "bridge".into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }),
        RequestFrame::CloseAgent(AgentRef {
            agent_id: AgentSlot("agent-1".into()),
        }),
        RequestFrame::CaptureAgent(AgentRef {
            agent_id: AgentSlot("agent-1".into()),
        }),
        RequestFrame::Checkpoint(CheckpointRequest {
            epoch: 9,
            exclusions: vec![".cache".into()],
        }),
        RequestFrame::Restore(RestoreRequest {
            object_reference: "object://checkpoint".into(),
            sha256: sha(),
        }),
        RequestFrame::Credential(CredentialFrame {
            operation_id: "credential-1".into(),
            value: RuntimeCredential::new("secret"),
            custody_generation: Some(7),
        }),
        RequestFrame::Resources,
    ];
    let tags = [
        "install",
        "prepare_workspace",
        "workspace_setup",
        "remove_workspace",
        "add_worktree",
        "remove_worktree",
        "materialize_agent_home",
        "open_agent",
        "close_agent",
        "capture_agent",
        "checkpoint",
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
        let decoded: RequestFrame = serde_json::from_value(json).expect("request deserializes");
        assert_eq!(decoded, frame);
    }
    assert!(serde_json::from_str::<RequestFrame>(r#"{"t":"unknown"}"#).is_err());
}

#[test]
fn every_contract_2_reply_round_trips_and_failures_stay_typed() {
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
        ResponseFrame::PrepareWorkspace(WorkspaceReceipt {
            name: "repo".into(),
            path: "/home/cma/repo".into(),
            resolved_commit: sha(),
        }),
        ResponseFrame::WorkspaceSetup(SetupReceipt {
            commands: vec![command],
        }),
        ResponseFrame::RemoveWorkspace(Acknowledgement::default()),
        ResponseFrame::AddWorktree(WorktreeReceipt {
            workspace: "repo".into(),
            name: "wt-1".into(),
            path: "/home/cma/repo/.worktrees/wt-1".into(),
            branch: "cma/wt-1".into(),
            base_commit: sha(),
        }),
        ResponseFrame::RemoveWorktree(Acknowledgement::default()),
        ResponseFrame::MaterializeAgentHome(Acknowledgement::default()),
        ResponseFrame::OpenAgent(AgentOpenReceipt {
            agent_id: AgentSlot("agent-1".into()),
            conversation_id: Some("conversation-1".into()),
        }),
        ResponseFrame::CloseAgent(Acknowledgement::default()),
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
