use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::hosted::HostedDeployment;
use crate::wire::{RequestFrame, WireOperation};
use crate::{
    AdapterError, AgentCloseRequest, AgentHomeRemoveRequest, AgentHomeRequest, AgentOpenRequest,
    AgentSlot, CheckpointHoldRequest, CheckpointRequest, CheckpointWriterResumeRequest,
    CredentialFrame, InstallRequest, PermissionLevel, ReasoningEffort, RestoreRequest,
    RuntimeCredential, RuntimeImage, RuntimeOptions, SandboxAdapter, SandboxSpec,
    WorkspaceRemoveRequest, WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource, WorktreeRef,
    WorktreeRequest, WriterState, RUNTIME_CONTRACT_LABEL,
};

fn sandbox_spec(own_credential: bool) -> SandboxSpec {
    SandboxSpec {
        sandbox_id: "sandbox-test".into(),
        own_credential,
        environment: std::collections::BTreeMap::from([("PROJECT_MODE".into(), "research".into())]),
        reaches: vec!["api-github".into(), "ornn-api".into()],
        created_by: Some("control-plane-test".into()),
        ..SandboxSpec::default()
    }
}

fn credential() -> CredentialFrame {
    CredentialFrame {
        operation_id: "credential-1".into(),
        value: RuntimeCredential::new("opaque-sandbox-key"),
        custody_generation: Some(3),
        github_route: None,
    }
}

fn options(slot: &str, cwd: String, own_credential: bool) -> RuntimeOptions {
    RuntimeOptions {
        model: Some(format!("model-{slot}")),
        reasoning_effort: ReasoningEffort::High,
        permission: PermissionLevel::Autonomous,
        own_credential,
        isolated_agent: false,
        working_directory: cwd,
        home_directory: format!("/home/cma/.cma/agents/{slot}/home"),
        slot: AgentSlot(slot.into()),
    }
}

#[tokio::test]
async fn contract_label_is_exact_and_capabilities_do_not_follow_the_image_name() {
    let mut bad = CodexSandboxSpec::default();
    bad.deployment.image = RuntimeImage {
        uri: "codex-contract-2-in-its-name".into(),
        labels: std::collections::BTreeMap::from([(RUNTIME_CONTRACT_LABEL.into(), "02".into())]),
    };
    let (adapter, backend) = CodexAdapter::in_memory(bad);
    assert_eq!(
        adapter.capabilities(),
        crate::AdapterCapabilities::default()
    );
    let error = adapter
        .provision(&sandbox_spec(false))
        .await
        .expect_err("non-exact contract label must fail");
    let AdapterError::RuntimeContractMismatch(mismatch) = error else {
        panic!("expected typed contract mismatch");
    };
    assert_eq!(mismatch.expected, "3");
    assert_eq!(mismatch.found.as_deref(), Some("02"));
    assert!(backend.requests().await.is_empty());

    let mut good = CodexSandboxSpec::default();
    good.deployment.image.uri = "an-arbitrary-image-name".into();
    assert!(good.deployment.capabilities().conversation);
    assert!(good.deployment.capabilities().snapshot);
    assert!(good.deployment.capabilities().per_agent_home);
}

#[tokio::test]
async fn provisioning_replay_reuses_only_the_exact_sandbox_specification() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let mut spec = sandbox_spec(false);
    let first = adapter.provision(&spec).await.unwrap();
    let replay = adapter.provision(&spec).await.unwrap();
    assert_eq!(replay, first);
    assert!(backend.snapshot(&first).await.is_ok());

    adapter
        .run_install(
            &first,
            &InstallRequest::new("sandbox-install-v1:reattach", Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(&first, credential())
        .await
        .unwrap();
    spec.created_by = Some("control-plane-after-rollout".into());
    assert_eq!(adapter.provision(&spec).await.unwrap(), first);

    let mut changed = spec;
    changed.capacity.cpu_millis += 1;
    let error = adapter.provision(&changed).await.unwrap_err();
    assert!(matches!(error, AdapterError::Rejected(detail)
        if detail.contains("another specification")));
}

#[tokio::test]
async fn held_checkpoints_replay_exactly_and_resume_only_for_the_owning_token() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter.provision(&sandbox_spec(false)).await.unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:held-checkpoint", Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(&handle, credential())
        .await
        .unwrap();

    let request = CheckpointHoldRequest {
        checkpoint: CheckpointRequest {
            epoch: 17,
            exclusions: vec![".cache".into()],
        },
        writer_resume_token: "a".repeat(64),
    };
    let receipt = adapter
        .checkpoint_for_teardown(&handle, &request)
        .await
        .unwrap();
    assert_eq!(receipt.checkpoint.writers, WriterState::Held);
    assert_eq!(receipt.writer_resume_token, request.writer_resume_token);
    assert_eq!(
        adapter
            .verify_checkpoint_hold(&handle, &request)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        adapter
            .checkpoint_for_teardown(&handle, &request)
            .await
            .unwrap(),
        receipt
    );
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert!(snapshot.writers_held);
    assert_eq!(snapshot.writer_fence_captures, 1);

    let mut conflicting_hold = request.clone();
    conflicting_hold.checkpoint.epoch += 1;
    let conflict = adapter
        .checkpoint_for_teardown(&handle, &conflicting_hold)
        .await
        .unwrap_err();
    assert!(matches!(conflict, AdapterError::Rejected(detail)
        if detail.contains("checkpoint_hold_conflict")));
    assert!(backend.snapshot(&handle).await.unwrap().writers_held);

    let blocked = adapter
        .checkpoint(
            &handle,
            &CheckpointRequest {
                epoch: 18,
                exclusions: Vec::new(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(blocked, AdapterError::Rejected(detail)
        if detail.contains("checkpoint_writers_held")));
    assert!(adapter.resources(&handle).await.is_ok());
    let capture = adapter
        .capture_agent(&handle, &AgentSlot::new("missing"))
        .await
        .unwrap_err();
    assert!(matches!(capture, AdapterError::Rejected(detail)
        if detail.contains("agent_not_found")));

    let wrong_resume = adapter
        .resume_checkpoint_writers(
            &handle,
            &CheckpointWriterResumeRequest {
                writer_resume_token: "b".repeat(64),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(wrong_resume, AdapterError::Rejected(detail)
        if detail.contains("checkpoint_hold_conflict")));
    assert!(backend.snapshot(&handle).await.unwrap().writers_held);

    let resumed = adapter
        .resume_checkpoint_writers(
            &handle,
            &CheckpointWriterResumeRequest {
                writer_resume_token: request.writer_resume_token.clone(),
            },
        )
        .await
        .unwrap();
    assert!(resumed.writers_resumed);
    assert!(!backend.snapshot(&handle).await.unwrap().writers_held);
    assert!(
        adapter
            .resume_checkpoint_writers(
                &handle,
                &CheckpointWriterResumeRequest {
                    writer_resume_token: resumed.writer_resume_token,
                },
            )
            .await
            .unwrap()
            .writers_resumed
    );
    assert!(matches!(
        adapter.checkpoint_for_teardown(&handle, &request).await,
        Err(AdapterError::Rejected(detail)) if detail.contains("checkpoint_hold_retired")
    ));
    assert!(matches!(
        adapter.verify_checkpoint_hold(&handle, &request).await,
        Err(AdapterError::Rejected(detail)) if detail.contains("checkpoint_hold_retired")
    ));
    assert!(!backend.snapshot(&handle).await.unwrap().writers_held);
    let mut next = request;
    next.writer_resume_token = "d".repeat(64);
    assert!(adapter
        .checkpoint_for_teardown(&handle, &next)
        .await
        .is_ok());
    assert!(backend.snapshot(&handle).await.unwrap().writers_held);
}

#[tokio::test]
async fn resuming_an_unseen_token_retires_a_delayed_first_hold() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter.provision(&sandbox_spec(false)).await.unwrap();
    adapter
        .run_install(&handle, &InstallRequest::new("delayed-hold", Vec::new()))
        .await
        .unwrap();
    adapter
        .deliver_credential(&handle, credential())
        .await
        .unwrap();
    let token = "e".repeat(64);
    adapter
        .resume_checkpoint_writers(
            &handle,
            &CheckpointWriterResumeRequest {
                writer_resume_token: token.clone(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        adapter.checkpoint_for_teardown(&handle, &CheckpointHoldRequest {
            checkpoint: CheckpointRequest { epoch: 1, exclusions: Vec::new() },
            writer_resume_token: token,
        }).await,
        Err(AdapterError::Rejected(detail)) if detail.contains("checkpoint_hold_retired")
    ));
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert!(!snapshot.writers_held);
    assert_eq!(snapshot.writer_fence_captures, 0);
}

#[tokio::test]
async fn lost_held_checkpoint_response_is_recovered_without_recapturing() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter.provision(&sandbox_spec(false)).await.unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:lost-held-checkpoint", Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(&handle, credential())
        .await
        .unwrap();

    backend.lose_next_response(WireOperation::CheckpointForTeardown);
    let error = adapter
        .checkpoint_for_teardown(
            &handle,
            &CheckpointHoldRequest {
                checkpoint: CheckpointRequest {
                    epoch: 23,
                    exclusions: Vec::new(),
                },
                writer_resume_token: "c".repeat(64),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AdapterError::Unavailable(_)));
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert!(!snapshot.writers_held);
    assert_eq!(snapshot.writer_fence_captures, 1);
    assert_eq!(
        backend
            .requests()
            .await
            .iter()
            .rev()
            .take(2)
            .map(|(_, request)| request.operation())
            .collect::<Vec<_>>(),
        [
            WireOperation::ResumeCheckpointWriters,
            WireOperation::CheckpointForTeardown,
        ]
    );
}

#[tokio::test]
async fn runtime_enforces_credential_order_after_adapter_side_failures() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter
        .provision(&sandbox_spec(false))
        .await
        .expect("sandbox provisions");

    let refused = adapter
        .deliver_credential(&handle, credential())
        .await
        .expect_err("credential before install must be refused");
    assert!(matches!(refused, AdapterError::Rejected(detail)
        if detail.contains("install_not_completed")));
    assert!(matches!(
        backend.requests().await.last(),
        Some((_, RequestFrame::Credential(_)))
    ));

    backend.fail_next(
        WireOperation::Install,
        "command_failed",
        "command 0 exited 1",
    );
    let install = InstallRequest::new("sandbox-install-v1:failure", vec!["false".into()]);
    assert!(adapter.run_install(&handle, &install).await.is_err());
    assert!(adapter
        .deliver_credential(&handle, credential())
        .await
        .is_err());

    backend.corrupt_next_install_receipt();
    assert!(adapter.run_install(&handle, &install).await.is_err());
    adapter
        .deliver_credential(&handle, credential())
        .await
        .expect("runtime accepts credentials after its durable install completed");

    adapter
        .run_install(&handle, &install)
        .await
        .expect("runtime replays the durable install receipt");

    let operations: Vec<_> = backend
        .requests()
        .await
        .iter()
        .map(|(_, request)| request.operation())
        .collect();
    assert_eq!(
        operations,
        [
            WireOperation::Credential,
            WireOperation::Install,
            WireOperation::Credential,
            WireOperation::Install,
            WireOperation::Credential,
            WireOperation::Install
        ]
    );
}

#[tokio::test]
async fn recreated_adapter_uses_the_reattached_runtime_lifecycle() {
    let backend = crate::hosted::InMemoryHostedBackend::default();
    let adapter_a = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let spec = sandbox_spec(false);
    let handle = adapter_a
        .provision(&spec)
        .await
        .expect("sandbox provisions");
    adapter_a
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:adapter-recreation", Vec::new()),
        )
        .await
        .expect("install completes");
    adapter_a
        .deliver_credential(&handle, credential())
        .await
        .expect("credential installs");
    let workspace = adapter_a
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                "workspace:wks-recreated:prepare:v1",
                "wks-recreated",
                "recreated",
                WorkspaceSource::Init,
            ),
        )
        .await
        .expect("workspace prepares");
    drop(adapter_a);

    let adapter_b = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let reattached = adapter_b
        .provision(&spec)
        .await
        .expect("new adapter reattaches");
    assert_eq!(reattached, handle);
    let worktree = adapter_b
        .add_worktree(
            &reattached,
            1,
            &WorktreeRequest::new(
                "worktree:recreated:after-restart:add:v1",
                "recreated",
                "after-restart",
                workspace.resolved_commit,
            ),
        )
        .await
        .expect("runtime accepts a credentialed mutation without install replay");
    assert_eq!(worktree.name, "after-restart");
    adapter_b
        .resources(&reattached)
        .await
        .expect("reattached runtime serves resource state");
}

#[tokio::test]
async fn correlated_mutations_replay_exactly_across_adapter_recreation() {
    let backend = crate::hosted::InMemoryHostedBackend::default();
    let spec = SandboxSpec {
        sandbox_id: "sandbox-correlated-replay".into(),
        ..sandbox_spec(false)
    };
    let adapter_a = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let handle = adapter_a.provision(&spec).await.unwrap();
    adapter_a
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:correlated-replay", Vec::new()),
        )
        .await
        .unwrap();
    adapter_a
        .deliver_credential(&handle, credential())
        .await
        .unwrap();
    let workspace = adapter_a
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                "workspace:wks-replay:prepare:v1",
                "wks-replay",
                "replay",
                WorkspaceSource::Init,
            ),
        )
        .await
        .unwrap();

    let add = WorktreeRequest::new(
        "worktree:replay:agent-replay:add:v1",
        "replay",
        "agent-replay",
        workspace.resolved_commit,
    );
    let worktree = adapter_a.add_worktree(&handle, 1, &add).await.unwrap();
    let mut changed_add = add.clone();
    changed_add.base = "f".repeat(64);
    changed_add.request_sha256 = changed_add.computed_sha256();
    assert!(matches!(
        adapter_a.add_worktree(&handle, 1, &changed_add).await,
        Err(AdapterError::Rejected(_))
    ));

    let agent_options = options("agent-replay", worktree.path.clone(), false);
    let home = AgentHomeRequest::new(
        "agent:agent-replay:home:create:v1",
        agent_options.clone(),
        "original prompt",
        Vec::new(),
    );
    adapter_a
        .materialize_agent_home(&handle, 1, &home)
        .await
        .unwrap();
    let mut changed_home = home.clone();
    changed_home.system_prompt.push_str(" changed");
    changed_home.request_sha256 = changed_home.computed_sha256();
    assert!(matches!(
        adapter_a
            .materialize_agent_home(&handle, 1, &changed_home)
            .await,
        Err(AdapterError::Rejected(_))
    ));

    let open = AgentOpenRequest::new(
        "agent:agent-replay:open:v1",
        agent_options.clone(),
        Some("conversation-replay".into()),
    );
    let (updates, mut updates_rx) = tokio::sync::mpsc::channel(4);
    let channel = adapter_a
        .open_agent(&handle, 10, &open, updates)
        .await
        .unwrap();
    assert_eq!(
        channel.conversation_id.as_deref(),
        Some("conversation-replay")
    );
    let mut changed_open = open.clone();
    changed_open.resume_conversation = Some("conversation-other".into());
    changed_open.request_sha256 = changed_open.computed_sha256();
    let (updates, _updates_rx) = tokio::sync::mpsc::channel(1);
    assert!(matches!(
        adapter_a
            .open_agent(&handle, 10, &changed_open, updates)
            .await,
        Err(AdapterError::OperationIdentityConflict(_))
    ));
    let (stale_updates, _stale_updates_rx) = tokio::sync::mpsc::channel(1);
    assert!(matches!(
        adapter_a.open_agent(&handle, 9, &open, stale_updates).await,
        Err(AdapterError::Rejected(_))
    ));
    backend
        .send_agent_update(
            &handle,
            &agent_options.slot,
            crate::ConversationUpdate::Lost { frames: 1 },
        )
        .await
        .unwrap();
    assert!(matches!(
        updates_rx.recv().await,
        Some(crate::ConversationUpdate::Lost { frames: 1 })
    ));

    let (current_updates, mut current_updates_rx) = tokio::sync::mpsc::channel(4);
    adapter_a
        .open_agent(&handle, 11, &open, current_updates)
        .await
        .unwrap();
    backend
        .send_agent_update(
            &handle,
            &agent_options.slot,
            crate::ConversationUpdate::Lost { frames: 2 },
        )
        .await
        .unwrap();
    assert!(matches!(
        current_updates_rx.recv().await,
        Some(crate::ConversationUpdate::Lost { frames: 2 })
    ));
    drop(adapter_a);

    let adapter_b = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    assert_eq!(adapter_b.provision(&spec).await.unwrap(), handle);
    assert_eq!(
        adapter_b.add_worktree(&handle, 1, &add).await.unwrap(),
        worktree
    );
    adapter_b
        .materialize_agent_home(&handle, 1, &home)
        .await
        .expect("completed home creation replays after adapter recreation");
    let (updates, _updates_rx) = tokio::sync::mpsc::channel(1);
    let replayed_channel = adapter_b
        .open_agent(&handle, 11, &open, updates)
        .await
        .expect("a retained live process can replay its open receipt");
    assert_eq!(replayed_channel.conversation_id, channel.conversation_id);
    backend
        .send_agent_update(
            &handle,
            &agent_options.slot,
            crate::ConversationUpdate::Lost { frames: 3 },
        )
        .await
        .unwrap();
    assert!(matches!(
        current_updates_rx.recv().await,
        Some(crate::ConversationUpdate::Lost { frames: 3 })
    ));

    let close = AgentCloseRequest::new("agent:agent-replay:close:v1", agent_options.slot.clone());
    adapter_b.close_agent(&handle, 12, &close).await.unwrap();
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert!(snapshot.homes.contains_key(&agent_options.slot));
    assert!(snapshot.agents.contains_key(&agent_options.slot));
    let changed_close =
        AgentCloseRequest::new("agent:agent-replay:close:v1", AgentSlot::new("agent-other"));
    assert!(matches!(
        adapter_b.close_agent(&handle, 12, &changed_close).await,
        Err(AdapterError::Rejected(_))
    ));
    drop(adapter_b);

    let adapter_c = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    assert_eq!(adapter_c.provision(&spec).await.unwrap(), handle);
    adapter_c
        .close_agent(&handle, 12, &close)
        .await
        .expect("completed close replays after adapter recreation");

    let remove_home = AgentHomeRemoveRequest::new(
        "agent:agent-replay:home:remove:v1",
        agent_options.slot.clone(),
    );
    adapter_c
        .remove_agent_home(&handle, 13, &remove_home)
        .await
        .unwrap();
    let changed_remove_home = AgentHomeRemoveRequest::new(
        "agent:agent-replay:home:remove:v1",
        AgentSlot::new("agent-other"),
    );
    assert!(matches!(
        adapter_c
            .remove_agent_home(&handle, 13, &changed_remove_home)
            .await,
        Err(AdapterError::Rejected(_))
    ));

    let remove_worktree = WorktreeRef::new(
        "worktree:replay:agent-replay:remove:v1",
        "replay",
        "agent-replay",
    );
    adapter_c
        .remove_worktree(&handle, 14, &remove_worktree)
        .await
        .unwrap();
    let changed_remove_worktree = WorktreeRef::new(
        "worktree:replay:agent-replay:remove:v1",
        "replay",
        "agent-other",
    );
    assert!(matches!(
        adapter_c
            .remove_worktree(&handle, 14, &changed_remove_worktree)
            .await,
        Err(AdapterError::Rejected(_))
    ));
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert!(!snapshot.homes.contains_key(&agent_options.slot));
    assert!(snapshot.worktrees.is_empty());
    drop(adapter_c);

    let adapter_d = CodexAdapter::with_backend(backend, CodexSandboxSpec::default());
    assert_eq!(adapter_d.provision(&spec).await.unwrap(), handle);
    adapter_d
        .remove_agent_home(&handle, 13, &remove_home)
        .await
        .expect("completed home removal replays after adapter recreation");
    adapter_d
        .remove_worktree(&handle, 14, &remove_worktree)
        .await
        .expect("completed worktree removal replays after adapter recreation");
}

#[tokio::test]
async fn a_failed_credential_rotation_keeps_the_installed_generation_usable() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter
        .provision(&sandbox_spec(false))
        .await
        .expect("sandbox provisions");
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:rotation", Vec::new()),
        )
        .await
        .expect("install completes");
    adapter
        .deliver_credential(&handle, credential())
        .await
        .expect("initial credential installs");

    backend.fail_next(
        WireOperation::Credential,
        "credential_refused",
        "replacement was not installed",
    );
    let rotated = CredentialFrame {
        operation_id: "credential-2".into(),
        value: RuntimeCredential::new("opaque-rotated-key"),
        custody_generation: Some(4),
        github_route: None,
    };
    assert!(adapter
        .deliver_credential(&handle, rotated.clone())
        .await
        .is_err());
    adapter
        .resources(&handle)
        .await
        .expect("the prior credential remains usable after a refused rotation");
    adapter
        .deliver_credential(&handle, rotated)
        .await
        .expect("the credential rotation can be retried");
}

#[tokio::test]
async fn a_trait_object_drives_two_agents_in_distinct_worktrees_end_to_end() {
    let (concrete, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let adapter: &dyn SandboxAdapter = &concrete;
    let handle = adapter
        .provision(&sandbox_spec(false))
        .await
        .expect("sandbox provisions");
    adapter
        .run_install(
            &handle,
            &InstallRequest::new(
                "sandbox-install-v1:end-to-end",
                vec!["install-toolchain".into(), "install-helper".into()],
            ),
        )
        .await
        .expect("install completes");
    adapter
        .deliver_credential(&handle, credential())
        .await
        .expect("credential installs");

    let workspace_a = adapter
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                "workspace:wks-alpha:prepare:v1",
                "wks-alpha",
                "alpha",
                WorkspaceSource::Clone {
                    url: "https://github.com/example/alpha".into(),
                    reference: None,
                },
            ),
        )
        .await
        .expect("first workspace");
    let workspace_b = adapter
        .prepare_workspace(
            &handle,
            &WorkspaceRequest::new(
                "workspace:wks-beta:prepare:v1",
                "wks-beta",
                "beta",
                WorkspaceSource::Init,
            ),
        )
        .await
        .expect("second workspace");
    adapter
        .run_workspace_setup(
            &handle,
            &WorkspaceSetupRequest::new(
                "workspace:wks-alpha:setup:v1",
                "wks-alpha",
                "alpha",
                vec!["make setup".into()],
            ),
        )
        .await
        .expect("workspace setup");

    let tree_a = adapter
        .add_worktree(
            &handle,
            1,
            &WorktreeRequest::new(
                "worktree:alpha:wt-1:add:v1",
                "alpha",
                "wt-1",
                workspace_a.resolved_commit,
            ),
        )
        .await
        .expect("first worktree");
    let tree_b = adapter
        .add_worktree(
            &handle,
            1,
            &WorktreeRequest::new(
                "worktree:beta:wt-2:add:v1",
                "beta",
                "wt-2",
                workspace_b.resolved_commit,
            ),
        )
        .await
        .expect("second worktree");

    let options_a = options("agent-a", tree_a.path.clone(), false);
    let options_b = options("agent-b", tree_b.path.clone(), false);
    for agent_options in [&options_a, &options_b] {
        adapter
            .materialize_agent_home(
                &handle,
                1,
                &AgentHomeRequest::new(
                    format!("agent:{}:home:create:v1", agent_options.slot.as_str()),
                    agent_options.clone(),
                    format!("Prompt for {}", agent_options.slot.0),
                    Vec::new(),
                ),
            )
            .await
            .expect("agent home materializes");
    }

    let (updates_a, _updates_a_rx) = tokio::sync::mpsc::channel(8);
    let (updates_b, _updates_b_rx) = tokio::sync::mpsc::channel(8);
    let channel_a = adapter
        .open_agent(
            &handle,
            1,
            &AgentOpenRequest::new("agent:agent-a:open:v1", options_a.clone(), None),
            updates_a,
        )
        .await
        .expect("first agent opens");
    let channel_b = adapter
        .open_agent(
            &handle,
            1,
            &AgentOpenRequest::new(
                "agent:agent-b:open:v1",
                options_b.clone(),
                Some("conversation-restored-b".into()),
            ),
            updates_b,
        )
        .await
        .expect("second agent opens");
    assert_ne!(channel_a.conversation_id, channel_b.conversation_id);
    assert_eq!(
        channel_b.conversation_id.as_deref(),
        Some("conversation-restored-b")
    );

    backend.corrupt_next_capture_digest();
    assert!(adapter
        .capture_agent(&handle, &options_a.slot)
        .await
        .is_err());
    let capture_a = adapter
        .capture_agent(&handle, &options_a.slot)
        .await
        .expect("first conversation captures");
    let capture_b = adapter
        .capture_agent(&handle, &options_b.slot)
        .await
        .expect("second conversation captures");
    assert_ne!(capture_a.conversation_id, capture_b.conversation_id);

    let checkpoint = adapter
        .checkpoint(
            &handle,
            &CheckpointRequest {
                epoch: 11,
                exclusions: vec![".cache".into()],
            },
        )
        .await
        .expect("whole home checkpoints");
    adapter
        .restore(
            &handle,
            &RestoreRequest {
                object_reference: checkpoint.object_reference.clone(),
                sha256: checkpoint.sha256,
            },
        )
        .await
        .expect("checkpoint restores");
    assert_eq!(
        adapter
            .resources(&handle)
            .await
            .expect("resources")
            .cpu_millis_limit,
        Some(1_000)
    );

    let snapshot = backend
        .snapshot(&handle)
        .await
        .expect("runtime is inspectable");
    assert_eq!(snapshot.worktrees.len(), 2);
    assert_eq!(snapshot.homes.len(), 2);
    assert_eq!(snapshot.agents.len(), 2);
    assert_eq!(snapshot.writer_fence_captures, 1);
    assert_eq!(
        snapshot.homes[&options_a.slot].agent_id,
        AgentSlot("agent-a".into())
    );
    assert_eq!(
        snapshot.homes[&options_b.slot].agent_id,
        AgentSlot("agent-b".into())
    );

    let launches: Vec<_> = backend
        .requests()
        .await
        .into_iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::OpenAgent(launch) => Some(launch.request),
            _ => None,
        })
        .collect();
    assert_eq!(launches.len(), 2);
    assert_ne!(launches[0].agent_id, launches[1].agent_id);
    assert_ne!(launches[0].home, launches[1].home);
    assert_ne!(launches[0].cwd, launches[1].cwd);
    assert!(launches.iter().all(|launch| launch.program == "codex-acp"));

    adapter
        .close_agent(
            &handle,
            1,
            &AgentCloseRequest::new("agent:agent-a:close:v1", options_a.slot.clone()),
        )
        .await
        .expect("first agent closes");
    adapter
        .close_agent(
            &handle,
            1,
            &AgentCloseRequest::new("agent:agent-b:close:v1", options_b.slot.clone()),
        )
        .await
        .expect("second agent closes");
    adapter
        .remove_agent_home(
            &handle,
            1,
            &AgentHomeRemoveRequest::new("agent:agent-a:home:remove:v1", options_a.slot.clone()),
        )
        .await
        .expect("first agent home removes");
    adapter
        .remove_agent_home(
            &handle,
            1,
            &AgentHomeRemoveRequest::new("agent:agent-b:home:remove:v1", options_b.slot.clone()),
        )
        .await
        .expect("second agent home removes");
    adapter
        .remove_worktree(
            &handle,
            1,
            &WorktreeRef::new("worktree:alpha:wt-1:remove:v1", "alpha", "wt-1"),
        )
        .await
        .expect("first worktree removes");
    adapter
        .remove_worktree(
            &handle,
            1,
            &WorktreeRef::new("worktree:beta:wt-2:remove:v1", "beta", "wt-2"),
        )
        .await
        .expect("second worktree removes");
    adapter
        .remove_workspace(
            &handle,
            &WorkspaceRemoveRequest::new("workspace:wks-alpha:remove:v1", "wks-alpha", "alpha"),
        )
        .await
        .expect("first workspace removes");
    adapter
        .remove_workspace(
            &handle,
            &WorkspaceRemoveRequest::new("workspace:wks-beta:remove:v1", "wks-beta", "beta"),
        )
        .await
        .expect("second workspace removes");
    adapter.teardown(&handle).await.expect("sandbox tears down");
    assert!(backend.snapshot(&handle).await.is_err());
}

#[tokio::test]
async fn stale_workspace_operations_cannot_act_on_a_reused_name() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter.provision(&sandbox_spec(false)).await.unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:workspace-lineage", Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(&handle, credential())
        .await
        .unwrap();

    let old_prepare = WorkspaceRequest::new(
        "workspace:wks-old:prepare:v1",
        "wks-old",
        "shared",
        WorkspaceSource::Init,
    );
    adapter
        .prepare_workspace(&handle, &old_prepare)
        .await
        .unwrap();
    let completed_old_remove =
        WorkspaceRemoveRequest::new("workspace:wks-old:remove:v1", "wks-old", "shared");
    adapter
        .remove_workspace(&handle, &completed_old_remove)
        .await
        .unwrap();

    let successor = WorkspaceRequest::new(
        "workspace:wks-new:prepare:v1",
        "wks-new",
        "shared",
        WorkspaceSource::Init,
    );
    adapter
        .prepare_workspace(&handle, &successor)
        .await
        .unwrap();
    assert!(matches!(
        adapter.prepare_workspace(&handle, &old_prepare).await,
        Err(AdapterError::Rejected(_))
    ));
    let stale_setup = WorkspaceSetupRequest::new(
        "workspace:wks-old:setup:v1",
        "wks-old",
        "shared",
        vec!["make setup".into()],
    );
    assert!(matches!(
        adapter.run_workspace_setup(&handle, &stale_setup).await,
        Err(AdapterError::Rejected(_))
    ));
    let unseen_old_remove =
        WorkspaceRemoveRequest::new("workspace:wks-old:remove:v2", "wks-old", "shared");
    assert!(matches!(
        adapter.remove_workspace(&handle, &unseen_old_remove).await,
        Err(AdapterError::Rejected(_))
    ));

    adapter
        .remove_workspace(&handle, &completed_old_remove)
        .await
        .expect("a terminal old replay returns its original acknowledgement");
    let snapshot = backend.snapshot(&handle).await.unwrap();
    assert_eq!(snapshot.workspaces["shared"].workspace_id, "wks-new");
}

#[tokio::test]
async fn removing_an_absent_workspace_blocks_its_delayed_first_prepare() {
    let (adapter, backend) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter.provision(&sandbox_spec(false)).await.unwrap();
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:absent-workspace", Vec::new()),
        )
        .await
        .unwrap();
    adapter
        .deliver_credential(&handle, credential())
        .await
        .unwrap();
    let removal = WorkspaceRemoveRequest::new("workspace:wks_old:remove:v1", "wks_old", "shared");
    adapter.remove_workspace(&handle, &removal).await.unwrap();
    let delayed = WorkspaceRequest::new(
        "workspace:wks_old:prepare:v1",
        "wks_old",
        "shared",
        WorkspaceSource::Init,
    );
    assert!(matches!(
        adapter.prepare_workspace(&handle, &delayed).await,
        Err(AdapterError::Rejected(_))
    ));
    assert!(backend
        .snapshot(&handle)
        .await
        .unwrap()
        .workspaces
        .is_empty());

    let successor = WorkspaceRequest::new(
        "workspace:wks_next:prepare:v1",
        "wks_next",
        "shared",
        WorkspaceSource::Init,
    );
    adapter
        .prepare_workspace(&handle, &successor)
        .await
        .unwrap();
    adapter.remove_workspace(&handle, &removal).await.unwrap();
    let setup = WorkspaceSetupRequest::new(
        "workspace:wks_old:setup:v1",
        "wks_old",
        "shared",
        vec!["touch stale".into()],
    );
    assert!(matches!(
        adapter.run_workspace_setup(&handle, &setup).await,
        Err(AdapterError::Rejected(_))
    ));
    assert_eq!(
        backend.snapshot(&handle).await.unwrap().workspaces["shared"].workspace_id,
        "wks_next"
    );
}

#[tokio::test]
async fn every_agent_uses_the_sandbox_wide_model_access_mode() {
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let handle = adapter
        .provision(&sandbox_spec(true))
        .await
        .expect("own-credential sandbox provisions");
    adapter
        .run_install(
            &handle,
            &InstallRequest::new("sandbox-install-v1:own-credential", Vec::new()),
        )
        .await
        .expect("empty install still returns proof");
    adapter
        .deliver_credential(&handle, credential())
        .await
        .expect("NyxID credential is independent of model access");

    let mismatched = AgentHomeRequest::new(
        "agent:agent-wrong-mode:home:create:v1",
        options(
            "agent-wrong-mode",
            "/home/cma/repo/.worktrees/wt-1".into(),
            false,
        ),
        String::new(),
        Vec::new(),
    );
    let error = adapter
        .materialize_agent_home(&handle, 1, &mismatched)
        .await
        .expect_err("agent cannot change sandbox model access");
    assert!(matches!(error, AdapterError::Rejected(detail)
        if detail.contains("credential mode differs")));
}

#[test]
fn hosted_request_keeps_size_environment_and_sandbox_reaches() {
    let deployment = HostedDeployment {
        upstream_base_url: "https://models.example/v1".into(),
        platform_base_url: "https://identity.example".into(),
        ..HostedDeployment::default()
    };
    let request = deployment
        .render(&sandbox_spec(false))
        .expect("provider request renders");

    assert_eq!(request.resource_limits["cpu"], "1000m");
    assert_eq!(request.resource_limits["memory"], "1024Mi");
    assert_eq!(request.image.uri, "cma-runtime:dev");
    assert_eq!(request.env["NYXID_URL"], "http://127.0.0.2:9092");
    assert_eq!(
        request.env["NYXID_ACCESS_TOKEN"],
        "local-proxy-no-credential-needed"
    );
    assert_eq!(request.env["CHRONO_SANDBOX_REACHES"], "api-github,ornn-api");
    assert!(!request.env.contains_key("CHRONO_SESSION_REACHES"));
    assert_eq!(request.env["PROJECT_MODE"], "research");
    let fingerprint = &request.metadata["cma/spec-sha256"];
    assert_eq!(fingerprint.len(), 43);
    assert!(fingerprint
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')));
    assert!(request
        .env
        .values()
        .all(|value| value != "opaque-sandbox-key"));
    assert_eq!(request.entrypoint, ["/usr/local/bin/layer4-runtime"]);
}
