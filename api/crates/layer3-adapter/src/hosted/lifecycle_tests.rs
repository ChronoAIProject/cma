use crate::codex::{CodexAdapter, CodexSandboxSpec};
use crate::hosted::HostedDeployment;
use crate::wire::{RequestFrame, WireOperation};
use crate::{
    AdapterError, AgentHomeRequest, AgentOpenRequest, AgentSlot, CheckpointRequest,
    CredentialFrame, InstallRequest, PermissionLevel, ReasoningEffort, RestoreRequest,
    RuntimeCredential, RuntimeImage, RuntimeOptions, SandboxAdapter, SandboxSpec, WorkspaceRequest,
    WorkspaceSetupRequest, WorkspaceSource, WorktreeRef, WorktreeRequest, RUNTIME_CONTRACT_LABEL,
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
    }
}

fn options(slot: &str, cwd: String, own_credential: bool) -> RuntimeOptions {
    RuntimeOptions {
        model: Some(format!("model-{slot}")),
        reasoning_effort: ReasoningEffort::High,
        permission: PermissionLevel::Autonomous,
        own_credential,
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
    assert_eq!(mismatch.expected, "2");
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
async fn credential_delivery_requires_proof_of_successful_install() {
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
        if detail.contains("successful install receipt")));
    assert!(backend
        .requests()
        .await
        .iter()
        .all(|(_, request)| !matches!(request, RequestFrame::Credential(_))));

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
    assert!(adapter
        .deliver_credential(&handle, credential())
        .await
        .is_err());

    adapter
        .run_install(&handle, &install)
        .await
        .expect("retried install succeeds");
    adapter
        .deliver_credential(&handle, credential())
        .await
        .expect("credential follows the successful receipt");

    let operations: Vec<_> = backend
        .requests()
        .await
        .iter()
        .map(|(_, request)| request.operation())
        .collect();
    assert_eq!(
        operations,
        [
            WireOperation::Install,
            WireOperation::Install,
            WireOperation::Install,
            WireOperation::Credential
        ]
    );
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
            &WorkspaceRequest {
                name: "alpha".into(),
                source: WorkspaceSource::Clone {
                    url: "https://github.com/example/alpha".into(),
                    reference: None,
                },
            },
        )
        .await
        .expect("first workspace");
    let workspace_b = adapter
        .prepare_workspace(
            &handle,
            &WorkspaceRequest {
                name: "beta".into(),
                source: WorkspaceSource::Init,
            },
        )
        .await
        .expect("second workspace");
    adapter
        .run_workspace_setup(
            &handle,
            &WorkspaceSetupRequest {
                name: "alpha".into(),
                commands: vec!["make setup".into()],
            },
        )
        .await
        .expect("workspace setup");

    let tree_a = adapter
        .add_worktree(
            &handle,
            &WorktreeRequest {
                workspace: "alpha".into(),
                name: "wt-1".into(),
                base: workspace_a.resolved_commit,
            },
        )
        .await
        .expect("first worktree");
    let tree_b = adapter
        .add_worktree(
            &handle,
            &WorktreeRequest {
                workspace: "beta".into(),
                name: "wt-2".into(),
                base: workspace_b.resolved_commit,
            },
        )
        .await
        .expect("second worktree");

    let options_a = options("agent-a", tree_a.path.clone(), false);
    let options_b = options("agent-b", tree_b.path.clone(), false);
    for agent_options in [&options_a, &options_b] {
        adapter
            .materialize_agent_home(
                &handle,
                &AgentHomeRequest {
                    options: agent_options.clone(),
                    system_prompt: format!("Prompt for {}", agent_options.slot.0),
                    skills: Vec::new(),
                },
            )
            .await
            .expect("agent home materializes");
    }

    let (updates_a, _updates_a_rx) = tokio::sync::mpsc::channel(8);
    let (updates_b, _updates_b_rx) = tokio::sync::mpsc::channel(8);
    let channel_a = adapter
        .open_agent(
            &handle,
            &AgentOpenRequest {
                options: options_a.clone(),
                resume_conversation: None,
            },
            updates_a,
        )
        .await
        .expect("first agent opens");
    let channel_b = adapter
        .open_agent(
            &handle,
            &AgentOpenRequest {
                options: options_b.clone(),
                resume_conversation: Some("conversation-restored-b".into()),
            },
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
            RequestFrame::OpenAgent(launch) => Some(launch),
            _ => None,
        })
        .collect();
    assert_eq!(launches.len(), 2);
    assert_ne!(launches[0].agent_id, launches[1].agent_id);
    assert_ne!(launches[0].home, launches[1].home);
    assert_ne!(launches[0].cwd, launches[1].cwd);
    assert!(launches.iter().all(|launch| launch.program == "codex-acp"));

    adapter
        .close_agent(&handle, &options_a.slot)
        .await
        .expect("first agent closes");
    adapter
        .close_agent(&handle, &options_b.slot)
        .await
        .expect("second agent closes");
    adapter
        .remove_worktree(
            &handle,
            &WorktreeRef {
                workspace: "alpha".into(),
                name: "wt-1".into(),
            },
        )
        .await
        .expect("first worktree removes");
    adapter
        .remove_worktree(
            &handle,
            &WorktreeRef {
                workspace: "beta".into(),
                name: "wt-2".into(),
            },
        )
        .await
        .expect("second worktree removes");
    adapter
        .remove_workspace(&handle, "alpha")
        .await
        .expect("first workspace removes");
    adapter
        .remove_workspace(&handle, "beta")
        .await
        .expect("second workspace removes");
    adapter.teardown(&handle).await.expect("sandbox tears down");
    assert!(backend.snapshot(&handle).await.is_err());
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

    let mismatched = AgentHomeRequest {
        options: options(
            "agent-wrong-mode",
            "/home/cma/repo/.worktrees/wt-1".into(),
            false,
        ),
        system_prompt: String::new(),
        skills: Vec::new(),
    };
    let error = adapter
        .materialize_agent_home(&handle, &mismatched)
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
