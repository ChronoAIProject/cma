use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use layer3_adapter::wire::{AgentRef, FencedRequest, ResponseFrame, WireOperation};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot,
    CheckpointHoldRequest, CheckpointRequest, CheckpointWriterResumeRequest, CredentialFrame,
    HomeFile, InstallRequest, RestoreRequest, RuntimeCredential, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource, WorktreeRef, WorktreeRequest,
    WriterState,
};

use super::*;

fn add_worktree_request(request: WorktreeRequest) -> RequestFrame {
    RequestFrame::AddWorktree(FencedRequest::new(1, request))
}

fn remove_worktree_request(request: WorktreeRef) -> RequestFrame {
    RequestFrame::RemoveWorktree(FencedRequest::new(1, request))
}

fn materialize_home_request(request: AgentHomePlan) -> RequestFrame {
    RequestFrame::MaterializeAgentHome(FencedRequest::new(1, request))
}

fn remove_home_request(request: AgentHomeRemoveRequest) -> RequestFrame {
    RequestFrame::RemoveAgentHome(FencedRequest::new(1, request))
}

fn open_agent_request(request: AgentLaunch) -> RequestFrame {
    RequestFrame::OpenAgent(FencedRequest::new(1, request))
}

fn close_agent_request(request: AgentCloseRequest) -> RequestFrame {
    RequestFrame::CloseAgent(FencedRequest::new(1, request))
}

fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cma-supervisor-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

fn supervisor(config: RuntimeConfig) -> Arc<Supervisor> {
    Supervisor::new(config, SupervisorBoundary::default()).unwrap()
}

fn cleanup(home: &Path) {
    std::fs::remove_dir_all(home).unwrap();
    let _ = std::fs::remove_dir_all(home.with_extension("checkpoints"));
    let _ = std::fs::remove_dir_all(home.with_extension("runtime-state"));
}

fn corrupt_workspace_journal(config: &RuntimeConfig) {
    let directory = config.state_root.join("workspace-operations");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{}.json", "0".repeat(64))),
        b"{broken",
    )
    .unwrap();
}

fn corrupt_install_journal(config: &RuntimeConfig) {
    std::fs::write(
        config
            .checkpoint_root
            .join("runtime-state")
            .join("install-v2.json"),
        b"{broken",
    )
    .unwrap();
}

fn credential(operation_id: &str) -> RequestFrame {
    RequestFrame::Credential(CredentialFrame {
        operation_id: operation_id.into(),
        value: RuntimeCredential::new("test-credential"),
        custody_generation: Some(1),
        github_route: None,
    })
}

fn failure(response: ResponseFrame) -> layer3_adapter::wire::WireFailure {
    match response {
        ResponseFrame::Failure(failure) => failure,
        other => panic!("expected failure response, got {other:?}"),
    }
}

async fn wait_for_contents(path: &Path, expected: &[u8]) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !std::fs::read(path).is_ok_and(|contents| contents == expected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent produced its complete fixture file");
}

#[tokio::test]
async fn credential_is_refused_until_install_completes() {
    let home = root("credential-order");
    let supervisor = supervisor(RuntimeConfig::for_test(home.clone()));
    let rejected = failure(supervisor.dispatch(credential("too-early")).await);
    assert_eq!(rejected.operation, WireOperation::Credential);
    assert_eq!(rejected.code, "install_not_completed");
    assert_eq!(supervisor.phase().await, Phase::Provisioned);
    cleanup(&home);
}

#[tokio::test]
async fn writer_resume_is_an_idempotent_noop_without_an_active_hold() {
    let home = root("writer-resume-noop");
    let supervisor = supervisor(RuntimeConfig::for_test(home.clone()));
    let token = "a".repeat(64);
    let response = supervisor
        .dispatch(RequestFrame::ResumeCheckpointWriters(
            CheckpointWriterResumeRequest {
                writer_resume_token: token.clone(),
            },
        ))
        .await;
    match response {
        ResponseFrame::ResumeCheckpointWriters(receipt) => {
            assert_eq!(receipt.writer_resume_token, token);
            assert!(receipt.writers_resumed);
        }
        other => panic!("writer resume no-op failed: {other:?}"),
    }
    assert_eq!(supervisor.phase().await, Phase::Provisioned);
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "delayed-hold",
                Vec::new()
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    assert!(matches!(
        supervisor.dispatch(credential("delayed-hold")).await,
        ResponseFrame::Credential(_)
    ));
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::CheckpointForTeardown(CheckpointHoldRequest {
                    checkpoint: CheckpointRequest {
                        epoch: 1,
                        exclusions: Vec::new()
                    },
                    writer_resume_token: token,
                }))
                .await
        )
        .code,
        "checkpoint_hold_retired"
    );
    assert!(supervisor.held_checkpoint.lock().await.is_none());
    cleanup(&home);
}

#[tokio::test]
async fn checkpoint_writer_tokens_reject_nonempty_and_symlink_records() {
    let home = root("invalid-writer-tokens");
    let config = RuntimeConfig::for_test(home.clone());
    let directory = config.checkpoint_root.join("checkpoint-writer-state");
    let supervisor = supervisor(config);
    std::fs::create_dir_all(&directory).unwrap();
    let token = "f".repeat(64);
    let marker = directory.join(&token);
    std::fs::write(&marker, b"not-an-empty-token-record").unwrap();
    let request = RequestFrame::ResumeCheckpointWriters(CheckpointWriterResumeRequest {
        writer_resume_token: token,
    });
    assert_eq!(
        failure(supervisor.dispatch(request.clone()).await).code,
        "checkpoint_writer_state_invalid"
    );
    std::fs::remove_file(&marker).unwrap();
    let target = directory.join("empty-target");
    std::fs::write(&target, b"").unwrap();
    std::os::unix::fs::symlink(&target, &marker).unwrap();
    assert_eq!(
        failure(supervisor.dispatch(request.clone()).await).code,
        "checkpoint_writer_state_invalid"
    );
    std::fs::remove_file(&marker).unwrap();
    assert!(matches!(
        supervisor.dispatch(request).await,
        ResponseFrame::ResumeCheckpointWriters(receipt) if receipt.writers_resumed
    ));
    cleanup(&home);
}

#[tokio::test]
async fn held_checkpoint_fences_mutations_and_replays_until_exact_resume() {
    let home = root("held-checkpoint");
    let config = RuntimeConfig::for_test(home.clone());
    let agent_home = config.agent_home("agent-held");
    let supervisor = supervisor(config);
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install-held-checkpoint",
                Vec::new(),
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    assert!(matches!(
        supervisor.dispatch(credential("held-checkpoint")).await,
        ResponseFrame::Credential(_)
    ));
    std::fs::create_dir_all(agent_home.join("sessions")).unwrap();
    std::fs::write(
        agent_home.join("sessions/rollout.jsonl"),
        b"{\"payload\":{\"id\":\"conversation-held\"}}\n",
    )
    .unwrap();

    let hold = CheckpointHoldRequest {
        checkpoint: CheckpointRequest {
            epoch: 31,
            exclusions: vec![".cache".into()],
        },
        writer_resume_token: "b".repeat(64),
    };
    let first = supervisor
        .dispatch(RequestFrame::CheckpointForTeardown(hold.clone()))
        .await;
    let receipt = match &first {
        ResponseFrame::CheckpointForTeardown(receipt) => {
            assert_eq!(receipt.checkpoint.writers, WriterState::Held);
            receipt.clone()
        }
        other => panic!("held checkpoint failed: {other:?}"),
    };
    assert!(supervisor.held_checkpoint.lock().await.is_some());
    assert_eq!(
        supervisor
            .dispatch(RequestFrame::CheckpointForTeardown(hold.clone()))
            .await,
        first
    );

    let mut conflict = hold.clone();
    conflict.checkpoint.epoch += 1;
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::CheckpointForTeardown(conflict))
                .await
        )
        .code,
        "checkpoint_hold_conflict"
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::Checkpoint(CheckpointRequest {
                    epoch: 32,
                    exclusions: Vec::new(),
                }))
                .await
        )
        .code,
        "checkpoint_writers_held"
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::Restore(RestoreRequest {
                    object_reference: receipt.checkpoint.object_reference.clone(),
                    sha256: receipt.checkpoint.sha256.clone(),
                }))
                .await
        )
        .code,
        "checkpoint_writers_held"
    );
    assert_eq!(
        failure(supervisor.dispatch(credential("blocked-rotation")).await).code,
        "checkpoint_writers_held"
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
                    "workspace:wks_held:prepare:v1",
                    "wks_held",
                    "held",
                    WorkspaceSource::Init,
                )))
                .await
        )
        .code,
        "checkpoint_writers_held"
    );
    assert!(matches!(
        supervisor.dispatch(RequestFrame::Resources).await,
        ResponseFrame::Resources(_)
    ));
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::CaptureAgent(AgentRef {
                agent_id: AgentSlot::new("agent-held"),
            }))
            .await,
        ResponseFrame::CaptureAgent(_)
    ));

    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::ResumeCheckpointWriters(
                    CheckpointWriterResumeRequest {
                        writer_resume_token: "c".repeat(64),
                    },
                ))
                .await
        )
        .code,
        "checkpoint_hold_conflict"
    );
    assert!(supervisor.held_checkpoint.lock().await.is_some());
    let resumed = supervisor
        .dispatch(RequestFrame::ResumeCheckpointWriters(
            CheckpointWriterResumeRequest {
                writer_resume_token: hold.writer_resume_token.clone(),
            },
        ))
        .await;
    assert!(matches!(
        resumed,
        ResponseFrame::ResumeCheckpointWriters(receipt) if receipt.writers_resumed
    ));
    assert!(supervisor.held_checkpoint.lock().await.is_none());
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::CheckpointForTeardown(hold.clone()))
                .await
        )
        .code,
        "checkpoint_hold_retired"
    );
    let restarted = Supervisor::new(
        RuntimeConfig::for_test(home.clone()),
        SupervisorBoundary::default(),
    )
    .unwrap();
    assert!(matches!(
        restarted.dispatch(credential("held-restarted")).await,
        ResponseFrame::Credential(_)
    ));
    assert_eq!(
        failure(
            restarted
                .dispatch(RequestFrame::CheckpointForTeardown(hold))
                .await
        )
        .code,
        "checkpoint_hold_retired"
    );
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Checkpoint(CheckpointRequest {
                epoch: 32,
                exclusions: Vec::new(),
            }))
            .await,
        ResponseFrame::Checkpoint(_)
    ));
    cleanup(&home);
}

#[tokio::test]
async fn startup_refuses_to_serve_with_any_corrupt_workspace_journal() {
    let home = root("startup-journal-corrupt");
    let config = RuntimeConfig::for_test(home.clone());
    corrupt_workspace_journal(&config);
    let result = Supervisor::start(config, SupervisorBoundary::default()).await;
    let error = match result {
        Ok(_) => panic!("startup accepted a corrupt workspace journal"),
        Err(error) => error,
    };
    assert_eq!(error.code, "workspace_journal_corrupt");
    cleanup(&home);
}

#[tokio::test]
async fn install_and_credential_validate_workspace_journals_before_mutating() {
    let home = root("unrelated-journal-corrupt");
    let config = RuntimeConfig::for_test(home.clone());
    let supervisor = supervisor(config.clone());
    corrupt_workspace_journal(&config);

    let install = failure(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install-blocked-by-journal",
                vec!["touch must-not-run".into()],
            )))
            .await,
    );
    assert_eq!(install.operation, WireOperation::Install);
    assert_eq!(install.code, "workspace_journal_corrupt");
    assert!(!home.join("must-not-run").exists());

    let credential = failure(supervisor.dispatch(credential("also-blocked")).await);
    assert_eq!(credential.operation, WireOperation::Credential);
    assert_eq!(credential.code, "workspace_journal_corrupt");
    cleanup(&home);
}

#[tokio::test]
async fn an_identical_install_replay_returns_the_receipt_without_rerunning_commands() {
    let home = root("install-replay");
    let supervisor = supervisor(RuntimeConfig::for_test(home.clone()));
    let request = InstallRequest::bounded(
        "install-replay",
        vec!["printf 'ran\\n' >> install-runs".into()],
        5,
        32 * 1024,
    );

    let first = supervisor
        .dispatch(RequestFrame::Install(request.clone()))
        .await;
    let replay = supervisor
        .dispatch(RequestFrame::Install(request.clone()))
        .await;
    assert_eq!(replay, first);
    assert_eq!(
        std::fs::read_to_string(home.join("install-runs")).unwrap(),
        "ran\n"
    );

    assert!(matches!(
        supervisor.dispatch(credential("credential-1")).await,
        ResponseFrame::Credential(_)
    ));
    let credentialed_replay = supervisor
        .dispatch(RequestFrame::Install(request.clone()))
        .await;
    assert_eq!(credentialed_replay, first);

    let mismatch = failure(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::bounded(
                "install-replay",
                vec!["touch must-not-run".into()],
                5,
                32 * 1024,
            )))
            .await,
    );
    assert_eq!(mismatch.code, "install_replay_mismatch");
    assert!(!home.join("must-not-run").exists());
    cleanup(&home);
}

#[tokio::test]
async fn recreated_supervisor_recovers_completed_install_before_credential_delivery() {
    let home = root("recreated-install");
    let config = RuntimeConfig::for_test(home.clone());
    let first = supervisor(config.clone());
    let installed = first
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install-before-recreation",
            vec!["printf 'ran\\n' >> install-runs".into()],
        )))
        .await;
    assert!(
        matches!(installed, ResponseFrame::Install(_)),
        "{installed:?}"
    );
    drop(first);

    let recreated = supervisor(config);
    assert_eq!(recreated.phase().await, Phase::Installed);
    let delivered = recreated.dispatch(credential("after-recreation")).await;
    assert!(
        matches!(delivered, ResponseFrame::Credential(_)),
        "{delivered:?}"
    );
    assert_eq!(recreated.phase().await, Phase::Credentialed);
    assert_eq!(
        std::fs::read_to_string(home.join("install-runs")).unwrap(),
        "ran\n"
    );
    cleanup(&home);
}

#[tokio::test]
async fn post_start_install_corruption_fences_every_mutation() {
    let home = root("install-corrupt-after-start");
    let config = RuntimeConfig::for_test(home.clone());
    let supervisor = supervisor(config.clone());
    let installed = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install-before-corruption",
            Vec::new(),
        )))
        .await;
    assert!(
        matches!(installed, ResponseFrame::Install(_)),
        "{installed:?}"
    );
    assert!(matches!(
        supervisor.dispatch(credential("before-corruption")).await,
        ResponseFrame::Credential(_)
    ));
    corrupt_install_journal(&config);

    let blocked_install = failure(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "install-after-corruption",
                vec!["touch must-not-install".into()],
            )))
            .await,
    );
    assert_eq!(blocked_install.code, "install_state_invalid");
    assert!(!home.join("must-not-install").exists());

    let blocked_credential = failure(supervisor.dispatch(credential("after-corruption")).await);
    assert_eq!(blocked_credential.code, "install_state_invalid");

    let blocked_workspace = failure(
        supervisor
            .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
                "workspace:wks_blocked:prepare:v1",
                "wks_blocked",
                "must-not-prepare",
                WorkspaceSource::Init,
            )))
            .await,
    );
    assert_eq!(blocked_workspace.code, "install_state_invalid");
    assert!(!home.join("must-not-prepare").exists());
    cleanup(&home);
}

#[tokio::test]
async fn proxy_is_absent_during_install_and_binds_after_credential() {
    let home = root("proxy-order");
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_bind = reserved.local_addr().unwrap();
    drop(reserved);
    let mut config = RuntimeConfig::for_test(home.clone());
    config
        .process_env
        .insert("CHRONO_INSTALL_SECRET".into(), "hidden".into());
    config
        .process_env
        .insert("NYXID_INSTALL_SECRET".into(), "hidden".into());
    config.model_proxy = Some(crate::config::ProxyConfig {
        bind: proxy_bind,
        upstream: "http://127.0.0.1:1".into(),
        role: "model",
    });
    let supervisor = supervisor(config);
    let command = format!(
        r#"if [ "${{CHRONO_INSTALL_SECRET+set}}" = set ] || [ "${{NYXID_INSTALL_SECRET+set}}" = set ]; then exit 11; fi
if (exec 3<>/dev/tcp/127.0.0.1/{}) 2>/dev/null; then exit 12; fi
touch install-complete"#,
        proxy_bind.port()
    );
    let response = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::bounded(
            "install-proxy",
            vec![command],
            5,
            32 * 1024,
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::Install(_)),
        "{response:?}"
    );
    assert!(home.join("install-complete").is_file());
    assert!(tokio::net::TcpStream::connect(proxy_bind).await.is_err());

    let response = supervisor.dispatch(credential("initial")).await;
    assert!(
        matches!(response, ResponseFrame::Credential(_)),
        "{response:?}"
    );
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(proxy_bind),
    )
    .await
    .expect("proxy connect completed")
    .expect("proxy listener is bound after credential delivery");
    cleanup(&home);
}

#[tokio::test]
async fn complete_frame_sequence_uses_real_git_home_process_and_checkpoint() {
    let home = root("frame-sequence");
    let mut config = RuntimeConfig::for_test(home.clone());
    config
        .process_env
        .insert("CHRONO_POST_CREDENTIAL".into(), "chrono-visible".into());
    config
        .process_env
        .insert("NYXID_POST_CREDENTIAL".into(), "nyxid-visible".into());
    let agent_home = config.agent_home("agent-1");
    let supervisor = supervisor(config);

    let response = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::bounded(
            "install-sequence",
            vec!["touch installed".into()],
            5,
            32 * 1024,
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::Install(_)),
        "{response:?}"
    );
    assert!(matches!(
        supervisor.dispatch(credential("initial")).await,
        ResponseFrame::Credential(_)
    ));

    let workspace = match supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_project:prepare:v1",
            "wks_project",
            "project",
            WorkspaceSource::Init,
        )))
        .await
    {
        ResponseFrame::PrepareWorkspace(receipt) => receipt,
        other => panic!("workspace preparation failed: {other:?}"),
    };
    let setup = supervisor
        .dispatch(RequestFrame::WorkspaceSetup(WorkspaceSetupRequest::new(
            "workspace:wks_project:setup:v1",
            "wks_project",
            "project",
            vec![
                r#"printf '%s|%s|%s' "$CHRONO_POST_CREDENTIAL" "$NYXID_POST_CREDENTIAL" "$CMA_TEST_ENV" > setup-env"#
                    .into(),
            ],
        )))
        .await;
    assert!(
        matches!(setup, ResponseFrame::WorkspaceSetup(_)),
        "{setup:?}"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("project/setup-env")).unwrap(),
        "chrono-visible|nyxid-visible|visible"
    );

    let worktree = match supervisor
        .dispatch(add_worktree_request(WorktreeRequest::new(
            "worktree:project:agent-1:add:v1",
            "project",
            "agent-1",
            workspace.resolved_commit,
        )))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt,
        other => panic!("worktree addition failed: {other:?}"),
    };
    let materialized = supervisor
        .dispatch(materialize_home_request(AgentHomePlan {
            operation_id: "agent:agent-1:home:create:v1".into(),
            request_sha256: "a".repeat(64),
            agent_id: AgentSlot::new("agent-1"),
            files: vec![HomeFile {
                path: "config.toml".into(),
                bytes: b"model = \"test\"\n".to_vec(),
            }],
            symlinks: Vec::new(),
            skills: Vec::new(),
        }))
        .await;
    assert!(
        matches!(materialized, ResponseFrame::MaterializeAgentHome(_)),
        "{materialized:?}"
    );

    let environment_file = agent_home.join("environment");
    let launch = AgentLaunch {
        operation_id: "agent:agent-1:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent-1"),
        cwd: worktree.receipt.path.clone(),
        home: agent_home.to_string_lossy().into_owned(),
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            r#"mkdir -p "$HOME/sessions"
printf '%s|%s|%s|%s' "$CHRONO_POST_CREDENTIAL" "$NYXID_POST_CREDENTIAL" "$AGENT_ONLY" "$CODEX_HOME" > "$HOME/environment"
printf '{"payload":{"id":"conversation-1"}}\n' > "$HOME/sessions/rollout.jsonl"
while IFS= read -r line; do printf '%s\n' "$line"; done"#
                .into(),
        ],
        env: BTreeMap::from([
            ("AGENT_ONLY".into(), "agent-visible".into()),
            (
                "CODEX_HOME".into(),
                agent_home.to_string_lossy().into_owned(),
            ),
        ]),
    };
    let opened = supervisor.dispatch(open_agent_request(launch)).await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    let expected_environment = format!(
        "chrono-visible|nyxid-visible|agent-visible|{}",
        agent_home.display()
    );
    wait_for_contents(&environment_file, expected_environment.as_bytes()).await;
    assert_eq!(
        std::fs::read_to_string(&environment_file).unwrap(),
        expected_environment
    );
    wait_for_contents(
        &agent_home.join("sessions/rollout.jsonl"),
        b"{\"payload\":{\"id\":\"conversation-1\"}}\n",
    )
    .await;
    let captured = supervisor
        .dispatch(RequestFrame::CaptureAgent(AgentRef {
            agent_id: AgentSlot::new("agent-1"),
        }))
        .await;
    match captured {
        ResponseFrame::CaptureAgent(snapshot) => {
            assert_eq!(snapshot.conversation_id, "conversation-1");
            assert_eq!(snapshot.archive_sha256.len(), 64);
        }
        other => panic!("conversation capture failed: {other:?}"),
    }

    let restore_while_open = failure(
        supervisor
            .dispatch(RequestFrame::Restore(RestoreRequest {
                object_reference: format!("home-{}.cma", "0".repeat(64)),
                sha256: "0".repeat(64),
            }))
            .await,
    );
    assert_eq!(restore_while_open.code, "restore_agents_open");

    #[cfg(target_os = "linux")]
    {
        let checkpoint = supervisor
            .dispatch(RequestFrame::Checkpoint(CheckpointRequest {
                epoch: 6,
                exclusions: vec![".cache".into()],
            }))
            .await;
        assert!(
            matches!(checkpoint, ResponseFrame::Checkpoint(_)),
            "{checkpoint:?}"
        );
    }

    let closed = supervisor
        .dispatch(close_agent_request(AgentCloseRequest::new(
            "agent:agent-1:close:v1",
            AgentSlot::new("agent-1"),
        )))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)), "{closed:?}");
    assert!(agent_home.exists());
    let removed_home = supervisor
        .dispatch(remove_home_request(AgentHomeRemoveRequest::new(
            "agent:agent-1:home:remove:v1",
            AgentSlot::new("agent-1"),
        )))
        .await;
    assert!(
        matches!(removed_home, ResponseFrame::RemoveAgentHome(_)),
        "{removed_home:?}"
    );
    assert!(!agent_home.exists());

    std::fs::write(home.join("durable"), "before checkpoint").unwrap();
    let checkpoint = match supervisor
        .dispatch(RequestFrame::Checkpoint(CheckpointRequest {
            epoch: 7,
            exclusions: vec![".cache".into()],
        }))
        .await
    {
        ResponseFrame::Checkpoint(receipt) => receipt,
        other => panic!("checkpoint failed: {other:?}"),
    };
    std::fs::write(home.join("durable"), "after checkpoint").unwrap();
    let restored = supervisor
        .dispatch(RequestFrame::Restore(RestoreRequest {
            object_reference: checkpoint.object_reference,
            sha256: checkpoint.sha256,
        }))
        .await;
    assert!(
        matches!(restored, ResponseFrame::Restore(_)),
        "{restored:?}"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("durable")).unwrap(),
        "before checkpoint"
    );

    let removed = supervisor
        .dispatch(remove_worktree_request(WorktreeRef::new(
            "worktree:project:agent-1:remove:v1",
            "project",
            "agent-1",
        )))
        .await;
    assert!(
        matches!(removed, ResponseFrame::RemoveWorktree(_)),
        "{removed:?}"
    );
    let removed = supervisor
        .dispatch(RequestFrame::RemoveWorkspace(WorkspaceRemoveRequest::new(
            "workspace:wks_project:remove:v1",
            "wks_project",
            "project",
        )))
        .await;
    assert!(
        matches!(removed, ResponseFrame::RemoveWorkspace(_)),
        "{removed:?}"
    );
    cleanup(&home);
}
