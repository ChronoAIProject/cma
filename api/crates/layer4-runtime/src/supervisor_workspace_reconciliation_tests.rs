use super::*;
use crate::workspace_journal::{self, Begin as WorkspaceBegin};
use layer3_adapter::{
    RuntimeStatusRequest, WorkspaceRemovalObservation, WorkspaceRemovalOutcome,
    WorkspaceRemovalRequest, WorkspaceRemoveReceipt,
};

fn persistent(name: &str) -> RuntimeConfig {
    let config = RuntimeConfig {
        home_identity: Some("a".repeat(43)),
        ..RuntimeConfig::for_test(root(name))
    };
    std::fs::create_dir_all(&config.home).unwrap();
    config
}

fn identity(config: &RuntimeConfig) -> RuntimeStatusRequest {
    RuntimeStatusRequest {
        sandbox_id: config.sandbox_id.clone(),
        generation: config.generation,
        home_identity: config.home_identity.clone().unwrap(),
    }
}

async fn current(config: &RuntimeConfig, supervisor: &Supervisor) -> layer3_adapter::RuntimeStatus {
    let response = dispatch_after_writer_contention(
        supervisor,
        RequestFrame::WorkspaceRemovalSupport(identity(config)),
    )
    .await;
    let ResponseFrame::WorkspaceRemovalSupport(support) = response else {
        panic!("{response:?}")
    };
    support.validate_for(&identity(config)).unwrap();
    support.runtime
}

fn remove_request() -> WorkspaceRemoveRequest {
    WorkspaceRemoveRequest::new("workspace:wks_target:remove:v1", "wks_target", "target")
}

async fn reconcile(config: &RuntimeConfig, supervisor: &Supervisor) -> ResponseFrame {
    dispatch_after_writer_contention(
        supervisor,
        RequestFrame::ReconcileWorkspaceRemoval(WorkspaceRemovalRequest {
            runtime: current(config, supervisor).await,
            request: remove_request(),
        }),
    )
    .await
}

async fn dispatch_after_writer_contention(
    supervisor: &Supervisor,
    request: RequestFrame,
) -> ResponseFrame {
    // Parallel fixtures may temporarily retain a writer lock. Retry only the
    // documented transient refusal, preserving the exact pinned boot/request.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let response = supervisor.dispatch(request.clone()).await;
        let busy = matches!(&response, ResponseFrame::Failure(error)
            if error.code == "workspace_removal_writer_busy" && error.retryable);
        if !busy || std::time::Instant::now() >= deadline {
            return response;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

fn observed(response: ResponseFrame) -> WorkspaceRemovalObservation {
    let ResponseFrame::ReconcileWorkspaceRemoval(value) = response else {
        panic!("{response:?}")
    };
    value
}

async fn prepare(config: &RuntimeConfig, supervisor: &Supervisor) {
    workspace_removal_tests::prepare_child(config, supervisor, "target").await;
}

fn begin(config: &RuntimeConfig) -> WorkspaceBegin<WorkspaceRemoveReceipt> {
    let request = remove_request();
    workspace_journal::begin(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
        "remove",
        &request.operation_id,
        &request.request_sha256,
        &request,
    )
    .unwrap()
}

#[tokio::test]
async fn completed_removal_reconciles_same_id_from_real_archive_and_preserves_canonical_barrier() {
    let config = persistent("workspace-reconcile-archive");
    let supervisor = provision(&config).await;
    prepare(&config, &supervisor).await;
    std::fs::create_dir_all(config.home.join("retained-sibling")).unwrap();
    std::fs::write(
        config.home.join("retained-sibling/conversation"),
        b"retained conversation",
    )
    .unwrap();
    let archive =
        crate::checkpoint_archive::seal(&config.home, &config.checkpoint_root, &[]).unwrap();
    let first = observed(reconcile(&config, &supervisor).await);
    assert_eq!(first.outcome, WorkspaceRemovalOutcome::CurrentlyAbsent);
    assert!(!config.home.join("target").exists());
    crate::checkpoint::CheckpointStore::new(&config)
        .restore(&layer3_adapter::RestoreRequest {
            object_reference: archive.reference,
            sha256: archive.sha256,
        })
        .await
        .unwrap();
    assert!(config
        .home
        .join("target/.git/info/cma-workspace-id")
        .is_file());
    // Completed history alone never starts archive scrubbing.
    supervisor.recover_journals().await.unwrap();
    assert!(config.home.join("target").exists());
    let legacy = supervisor
        .dispatch(RequestFrame::RemoveWorkspace(remove_request()))
        .await;
    assert_eq!(
        failure(legacy).code,
        "workspace_removal_reconciliation_required"
    );
    let second = observed(reconcile(&config, &supervisor).await);
    assert_eq!(second.receipt, first.receipt);
    assert_eq!(second.outcome, WorkspaceRemovalOutcome::CurrentlyAbsent);
    assert!(!config.home.join("target").exists());
    assert_eq!(
        std::fs::read(config.home.join("retained-sibling/conversation")).unwrap(),
        b"retained conversation"
    );
    let late_prepare = supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_target:prepare:v1",
            "wks_target",
            "target",
            WorkspaceSource::Init,
        )))
        .await;
    assert_eq!(failure(late_prepare).code, "workspace_removed");
    assert_eq!(
        observed(reconcile(&config, &supervisor).await).receipt,
        first.receipt
    );
    cleanup(&config);
}

#[tokio::test]
async fn completed_successor_is_no_effect_and_never_current_absence() {
    let config = persistent("workspace-reconcile-successor");
    let supervisor = provision(&config).await;
    let first = observed(reconcile(&config, &supervisor).await);
    let successor = supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_successor:prepare:v1",
            "wks_successor",
            "target",
            WorkspaceSource::Init,
        )))
        .await;
    assert!(
        matches!(successor, ResponseFrame::PrepareWorkspace(_)),
        "{successor:?}"
    );
    std::fs::write(config.home.join("target/successor"), b"never delete").unwrap();
    let replay = observed(reconcile(&config, &supervisor).await);
    assert_eq!(replay.receipt, first.receipt);
    assert_eq!(replay.outcome, WorkspaceRemovalOutcome::SuccessorPresent);
    let legacy = supervisor
        .dispatch(RequestFrame::RemoveWorkspace(remove_request()))
        .await;
    assert!(
        matches!(legacy, ResponseFrame::RemoveWorkspace(_)),
        "{legacy:?}"
    );
    assert_eq!(
        std::fs::read(config.home.join("target/successor")).unwrap(),
        b"never delete"
    );
    cleanup(&config);
}

#[tokio::test]
async fn stale_same_generation_boot_cannot_reclaim_by_probe_or_replay_and_handover_waits_for_io() {
    let config = persistent("workspace-reconcile-stale-boot");
    let old = provision(&config).await;
    prepare(&config, &old).await;
    let request = WorkspaceRemovalRequest {
        runtime: current(&config, &old).await,
        request: remove_request(),
    };
    let held = crate::workspace_removal_writer::guard(&config, &request.runtime.boot_id).unwrap();
    assert!(
        matches!(Supervisor::new(config.clone(), SupervisorBoundary::default()), Err(error) if error.code == "workspace_removal_writer_busy")
    );
    assert!(config.home.join("target").exists());
    drop(held);
    let replacement = recreate(&config).await;
    for _ in 0..2 {
        assert_eq!(
            failure(
                dispatch_after_writer_contention(
                    &old,
                    RequestFrame::WorkspaceRemovalSupport(identity(&config)),
                )
                .await
            )
            .code,
            "workspace_removal_writer_stale"
        );
        assert_eq!(
            failure(
                dispatch_after_writer_contention(
                    &old,
                    RequestFrame::ReconcileWorkspaceRemoval(request.clone()),
                )
                .await
            )
            .code,
            "workspace_removal_writer_stale"
        );
    }
    assert_eq!(
        failure(
            dispatch_after_writer_contention(
                &replacement,
                RequestFrame::ReconcileWorkspaceRemoval(request),
            )
            .await
        )
        .code,
        "workspace_removal_runtime_invalid"
    );
    assert!(config.home.join("target").exists());
    assert_eq!(
        observed(reconcile(&config, &replacement).await).outcome,
        WorkspaceRemovalOutcome::CurrentlyAbsent
    );
    cleanup(&config);
}

#[tokio::test]
async fn current_removal_started_before_lost_io_never_scrubs_from_startup_or_legacy_retry() {
    let config = persistent("workspace-reconcile-pending-boot");
    let old = provision(&config).await;
    prepare(&config, &old).await;
    crate::workspace_removal_intent::record(&config, &remove_request()).unwrap();
    assert!(matches!(begin(&config), WorkspaceBegin::New(_)));
    drop(old);
    let replacement = Supervisor::start(config.clone(), SupervisorBoundary::default())
        .await
        .unwrap();
    assert!(config.home.join("target").exists());
    assert!(matches!(begin(&config), WorkspaceBegin::Started(_)));
    credential(&replacement).await;
    assert_eq!(
        failure(
            replacement
                .dispatch(RequestFrame::RemoveWorkspace(remove_request()))
                .await
        )
        .code,
        "workspace_removal_reconciliation_required"
    );
    assert!(config.home.join("target").exists());
    assert_eq!(
        observed(reconcile(&config, &replacement).await).outcome,
        WorkspaceRemovalOutcome::CurrentlyAbsent
    );
    cleanup(&config);
}

#[tokio::test]
async fn archive_scrub_requires_no_live_or_durable_target_agent() {
    for restart in [false, true] {
        let config = persistent(&format!("workspace-reconcile-agent-{restart}"));
        let mut supervisor = provision(&config).await;
        let target = workspace_removal_tests::prepare_child(&config, &supervisor, "target").await;
        workspace_removal_tests::open(&supervisor, &target).await;
        if restart {
            let bridges = supervisor
                .agents
                .read()
                .await
                .values()
                .map(|agent| Arc::clone(&agent.bridge))
                .collect::<Vec<_>>();
            for bridge in bridges {
                bridge.terminate().await.unwrap();
            }
            drop(supervisor);
            supervisor = recreate(&config).await;
        }
        assert_eq!(
            failure(reconcile(&config, &supervisor).await).code,
            "workspace_agent_open"
        );
        assert!(config.home.join("target").exists());
        workspace_removal_tests::close(&supervisor, &target).await;
        assert_eq!(
            observed(reconcile(&config, &supervisor).await).outcome,
            WorkspaceRemovalOutcome::CurrentlyAbsent
        );
        cleanup(&config);
    }
}

#[tokio::test]
async fn held_checkpoint_excludes_reconciliation_before_any_journal_or_path_mutation() {
    let config = persistent("workspace-reconcile-checkpoint");
    let supervisor = provision(&config).await;
    prepare(&config, &supervisor).await;
    let held = supervisor
        .dispatch(RequestFrame::CheckpointForTeardown(
            layer3_adapter::CheckpointHoldRequest {
                checkpoint: layer3_adapter::CheckpointRequest {
                    epoch: 1,
                    exclusions: vec![],
                },
                writer_resume_token: "a".repeat(64),
            },
        ))
        .await;
    assert!(
        matches!(held, ResponseFrame::CheckpointForTeardown(_)),
        "{held:?}"
    );
    assert_eq!(
        failure(reconcile(&config, &supervisor).await).code,
        "checkpoint_writers_held"
    );
    assert!(
        !crate::workspace_removal_intent::requires_current(&config, &remove_request()).unwrap()
    );
    assert!(config.home.join("target").exists());
    let resumed = supervisor
        .dispatch(RequestFrame::ResumeCheckpointWriters(
            layer3_adapter::CheckpointWriterResumeRequest {
                writer_resume_token: "a".repeat(64),
            },
        ))
        .await;
    assert!(
        matches!(resumed, ResponseFrame::ResumeCheckpointWriters(_)),
        "{resumed:?}"
    );
    assert_eq!(
        observed(reconcile(&config, &supervisor).await).outcome,
        WorkspaceRemovalOutcome::CurrentlyAbsent
    );
    cleanup(&config);
}

#[tokio::test]
async fn unsafe_identity_symlinks_and_ambiguous_staging_hold_without_effects() {
    for mode in [
        "missing",
        "marker-link",
        "git-link",
        "path-link",
        "staged",
        "quarantine-link",
    ] {
        let config = persistent(&format!("workspace-reconcile-unsafe-{mode}"));
        let supervisor = provision(&config).await;
        prepare(&config, &supervisor).await;
        let marker = config.home.join("target/.git/info/cma-workspace-id");
        let outside = config.home.with_extension("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), b"outside").unwrap();
        match mode {
            "missing" => std::fs::remove_file(&marker).unwrap(),
            "marker-link" => {
                std::fs::rename(&marker, outside.join("marker")).unwrap();
                std::os::unix::fs::symlink(outside.join("marker"), &marker).unwrap();
            }
            "git-link" => {
                std::fs::rename(config.home.join("target/.git"), outside.join("git")).unwrap();
                std::os::unix::fs::symlink(outside.join("git"), config.home.join("target/.git"))
                    .unwrap();
            }
            "path-link" => {
                std::fs::rename(config.home.join("target"), outside.join("target")).unwrap();
                std::os::unix::fs::symlink(outside.join("target"), config.home.join("target"))
                    .unwrap();
            }
            "staged" => {
                std::fs::create_dir_all(config.home.join(".cma/preparing/wks_target")).unwrap()
            }
            "quarantine-link" => {
                std::fs::create_dir_all(config.home.join(".cma")).unwrap();
                std::os::unix::fs::symlink(&outside, config.home.join(".cma/removing")).unwrap();
            }
            _ => unreachable!(),
        }
        let response = failure(reconcile(&config, &supervisor).await);
        assert!(
            matches!(
                response.code.as_str(),
                "workspace_removal_path_unsafe" | "workspace_incarnation_ambiguous"
            ),
            "{mode}: {response:?}"
        );
        assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"outside");
        assert!(
            !crate::workspace_removal_intent::requires_current(&config, &remove_request()).unwrap()
        );
        cleanup(&config);
        std::fs::remove_dir_all(outside).unwrap();
    }
}

#[tokio::test]
async fn replacement_runtime_without_old_journal_reconciles_restored_workspace_after_child_home() {
    let source = persistent("workspace-reconcile-source-runtime");
    let original = provision(&source).await;
    prepare(&source, &original).await;
    std::fs::create_dir_all(source.home.join("retained-sibling")).unwrap();
    std::fs::write(source.home.join("retained-sibling/history"), b"retain me").unwrap();
    let archive =
        crate::checkpoint_archive::seal(&source.home, &source.checkpoint_root, &[]).unwrap();
    let original_removal = observed(reconcile(&source, &original).await);
    let replacement = persistent("workspace-reconcile-replacement-runtime");
    let config = RuntimeConfig {
        generation: source.generation + 1,
        ..replacement
    };
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let restore = layer3_adapter::RestoreRequest {
        object_reference: archive.reference.clone(),
        sha256: archive.sha256.clone(),
    };
    let bootstrap = supervisor
        .dispatch(RequestFrame::BootstrapRestore(
            layer3_adapter::RestoreBootstrapRequest::new(
                "workspace-restore",
                &config.sandbox_id,
                restore.clone(),
            ),
        ))
        .await;
    assert!(
        matches!(bootstrap, ResponseFrame::BootstrapRestore(_)),
        "{bootstrap:?}"
    );
    credential(&supervisor).await;
    std::fs::create_dir_all(&config.checkpoint_root).unwrap();
    std::fs::copy(
        &archive.path,
        config.checkpoint_root.join(&archive.reference),
    )
    .unwrap();
    let restored = supervisor.dispatch(RequestFrame::Restore(restore)).await;
    assert!(
        matches!(restored, ResponseFrame::Restore(_)),
        "{restored:?}"
    );
    assert!(config.home.join("target").exists());
    let slot = AgentSlot::new("agent-target");
    let closed = supervisor
        .dispatch(close_agent_request(AgentCloseRequest::new(
            "agent:agent-target:close:v1",
            slot.clone(),
        )))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)), "{closed:?}");
    let removed_home = supervisor
        .dispatch(remove_home_request(AgentHomeRemoveRequest::new(
            "agent:agent-target:home:remove:v1",
            slot,
        )))
        .await;
    assert!(
        matches!(removed_home, ResponseFrame::RemoveAgentHome(_)),
        "{removed_home:?}"
    );
    let fresh = observed(reconcile(&config, &supervisor).await);
    assert_eq!(fresh.receipt, original_removal.receipt);
    assert_ne!(fresh.runtime.boot_id, original_removal.runtime.boot_id);
    assert_eq!(fresh.runtime.generation, source.generation + 1);
    assert!(!config.home.join("target").exists());
    assert!(!config.agent_home("agent-target").exists());
    assert_eq!(
        std::fs::read(config.home.join("retained-sibling/history")).unwrap(),
        b"retain me"
    );
    cleanup(&source);
    cleanup(&config);
}

#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "requires isolated supervisor process boundary; run explicitly in backend CI"]
async fn real_linux_writer_boundary_reconciles_target_and_resumes_live_sibling() {
    let config = persistent("workspace-reconcile-linux-writers");
    let boundary = SupervisorBoundary::install().unwrap();
    assert!(boundary.root.is_some());
    let supervisor = Supervisor::new(config.clone(), boundary).unwrap();
    let installed = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install-linux-reconcile",
            vec![],
        )))
        .await;
    assert!(
        matches!(installed, ResponseFrame::Install(_)),
        "{installed:?}"
    );
    credential(&supervisor).await;
    prepare(&config, &supervisor).await;
    let sibling = workspace_removal_tests::prepare_child(&config, &supervisor, "sibling").await;
    workspace_removal_tests::open(&supervisor, &sibling).await;
    let expected = current(&config, &supervisor).await;
    assert_eq!(expected.agent_processes, 1);
    let observation = observed(
        dispatch_after_writer_contention(
            &supervisor,
            RequestFrame::ReconcileWorkspaceRemoval(WorkspaceRemovalRequest {
                runtime: expected,
                request: remove_request(),
            }),
        )
        .await,
    );
    assert_eq!(
        observation.outcome,
        WorkspaceRemovalOutcome::CurrentlyAbsent
    );
    assert_eq!(observation.runtime.agent_processes, 1);
    assert!(!config.home.join("target").exists());
    assert_eq!(
        std::fs::read(PathBuf::from(&sibling.cwd).join("sentinel")).unwrap(),
        b"sibling"
    );
    let root_pid = {
        let agents = supervisor.agents.read().await;
        let process = agents.get(&sibling.agent_id).unwrap();
        assert!(!process.bridge.is_ended());
        process.bridge.writer_session_root().unwrap()
    };
    let table = crate::writer_process::process_table().unwrap();
    assert!(table.contains_key(&root_pid));
    assert!(!matches!(table[&root_pid].state, 'T' | 't' | 'Z'));
    workspace_removal_tests::close(&supervisor, &sibling).await;
    cleanup(&config);
}

#[tokio::test]
async fn removal_writer_probe_rejects_symlink_and_fifo_without_hanging_or_reclaiming() {
    for mode in ["symlink", "fifo"] {
        let config = persistent(&format!("workspace-removal-writer-{mode}"));
        let supervisor = provision(&config).await;
        prepare(&config, &supervisor).await;
        let record = config.state_root.join("workspace-removal-writer-v1.json");
        let original = std::fs::read(&record).unwrap();
        std::fs::remove_file(&record).unwrap();
        let outside = config.state_root.join("outside-record");
        std::fs::write(&outside, &original).unwrap();
        if mode == "symlink" {
            std::os::unix::fs::symlink(&outside, &record).unwrap();
        } else {
            assert!(std::process::Command::new("mkfifo")
                .arg(&record)
                .status()
                .unwrap()
                .success());
        }
        let response = dispatch_after_writer_contention(
            &supervisor,
            RequestFrame::WorkspaceRemovalSupport(identity(&config)),
        )
        .await;
        assert!(
            matches!(response, ResponseFrame::Failure(_)),
            "{mode}: {response:?}"
        );
        assert_eq!(std::fs::read(&outside).unwrap(), original);
        assert!(config.home.join("target").exists());
        std::fs::remove_file(&record).unwrap();
        std::fs::write(&record, original).unwrap();
        assert_eq!(
            observed(reconcile(&config, &supervisor).await).outcome,
            WorkspaceRemovalOutcome::CurrentlyAbsent
        );
        cleanup(&config);
    }
}
