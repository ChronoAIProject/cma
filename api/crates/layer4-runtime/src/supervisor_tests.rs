use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use layer3_adapter::wire::{AgentRef, ResponseFrame, WireOperation};
use layer3_adapter::{
    AgentHomePlan, AgentLaunch, AgentSlot, CheckpointRequest, CredentialFrame, HomeFile,
    InstallRequest, RestoreRequest, RuntimeCredential, WorkspaceRemoveRequest, WorkspaceRequest,
    WorkspaceSetupRequest, WorkspaceSource, WorktreeRef, WorktreeRequest,
};

use super::*;

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

fn credential(operation_id: &str) -> RequestFrame {
    RequestFrame::Credential(CredentialFrame {
        operation_id: operation_id.into(),
        value: RuntimeCredential::new("test-credential"),
        custody_generation: Some(1),
    })
}

fn failure(response: ResponseFrame) -> layer3_adapter::wire::WireFailure {
    match response {
        ResponseFrame::Failure(failure) => failure,
        other => panic!("expected failure response, got {other:?}"),
    }
}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent produced its fixture file");
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
        .dispatch(RequestFrame::AddWorktree(WorktreeRequest {
            workspace: "project".into(),
            name: "agent-1".into(),
            base: workspace.resolved_commit,
        }))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt,
        other => panic!("worktree addition failed: {other:?}"),
    };
    let materialized = supervisor
        .dispatch(RequestFrame::MaterializeAgentHome(AgentHomePlan {
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
        agent_id: AgentSlot::new("agent-1"),
        cwd: worktree.path.clone(),
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
    let opened = supervisor.dispatch(RequestFrame::OpenAgent(launch)).await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    wait_for_file(&environment_file).await;
    assert_eq!(
        std::fs::read_to_string(&environment_file).unwrap(),
        format!(
            "chrono-visible|nyxid-visible|agent-visible|{}",
            agent_home.display()
        )
    );
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
        .dispatch(RequestFrame::CloseAgent(AgentRef {
            agent_id: AgentSlot::new("agent-1"),
        }))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)), "{closed:?}");
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
        .dispatch(RequestFrame::RemoveWorktree(WorktreeRef {
            workspace: "project".into(),
            name: "agent-1".into(),
        }))
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
