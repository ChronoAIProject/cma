use super::*;

#[tokio::test]
async fn indexed_owner_stop_close_proof_refuses_admitted_successor_without_recovery() {
    let mut config = indexed_config();
    config.agent_process_restart_enabled = true;
    config.home_identity = Some("a".repeat(43));
    std::fs::create_dir_all(&config.home).unwrap();
    let (supervisor, ledger) = indexed(&config, 32_768).await;
    let workspace = prepare_workspace(&supervisor).await;
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"retained conversation fixture");
    assert!(matches!(
        supervisor.dispatch(materialize_home_request(plan)).await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let mut launch = launch(&config, &worktree, "while IFS= read -r line; do :; done");
    assert!(matches!(
        supervisor
            .dispatch(open_agent_request(launch.clone()))
            .await,
        ResponseFrame::OpenAgent(_)
    ));
    let close = AgentCloseRequest::new("agent:agent-1:close:v1", launch.agent_id.clone());
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::CloseAgent(FencedRequest::new(
                2,
                close.clone()
            )))
            .await,
        ResponseFrame::CloseAgent(_)
    ));
    let receipt = layer3_adapter::wire::AgentProcessCloseProof {
        runtime: layer3_adapter::AgentRestartRuntime {
            runtime: layer3_adapter::RuntimeStatusRequest {
                sandbox_id: config.sandbox_id.clone(),
                generation: config.generation,
                home_identity: config.home_identity.clone().unwrap(),
            },
            boot_id: supervisor.boot_id.clone(),
        },
        close,
        open_request_sha256: launch.request_sha256.clone(),
        worktree_path: launch.cwd.clone(),
    };
    let request = RequestFrame::ProveAgentProcessClosed(receipt.clone());
    assert_eq!(
        supervisor.dispatch(request.clone()).await,
        ResponseFrame::ProveAgentProcessClosed(receipt.clone())
    );

    launch.operation_id = "agent:agent-1:open:2:v1".into();
    launch.request_sha256 = "c".repeat(64);
    let fence = layer3_adapter::wire::AgentRestartFence {
        predecessor: receipt,
        successor_operation_id: launch.operation_id.clone(),
        authority_fence: 4,
        successor_open_request_sha256: None,
    };
    let fence_request = RequestFrame::FenceAgentRestart(fence.clone());
    for _ in 0..2 {
        assert_eq!(
            supervisor.dispatch(fence_request.clone()).await,
            ResponseFrame::FenceAgentRestart(fence.clone())
        );
    }
    let late = super::persistent_home_tests::restart_open(&supervisor, 3, &launch);
    assert_eq!(
        failure(supervisor.dispatch(late).await).code,
        "mutation_authority_stale"
    );

    let operation = Operation::open_agent(&launch);
    assert!(!supervisor
        .mutations
        .has_process_writer("agent-1", 2)
        .await
        .unwrap());
    assert!(!ledger.admitted(operation.clone()).await.unwrap());
    ledger.admit(5, vec![operation.clone()]).await.unwrap();
    assert_eq!(
        failure(supervisor.dispatch(fence_request.clone()).await).code,
        "agent_process_close_unproved"
    );
    assert_eq!(
        failure(supervisor.dispatch(request).await).code,
        "agent_process_close_unproved"
    );
    assert!(!ledger.admitted(operation.clone()).await.unwrap());
    assert!(supervisor.bridge("agent-1").await.is_none());

    let mut close_successor = fence.clone();
    close_successor.authority_fence = 6;
    close_successor.successor_open_request_sha256 = Some(launch.request_sha256.clone());
    let close_request = RequestFrame::FenceAgentRestart(close_successor.clone());
    assert_eq!(
        failure(supervisor.dispatch(close_request.clone()).await).code,
        "agent_process_close_unproved"
    );
    assert!(!ledger.admitted(operation.clone()).await.unwrap());
    assert!(supervisor.bridge("agent-1").await.is_none());

    let pinned = super::persistent_home_tests::restart_open(&supervisor, 5, &launch);
    // The process started before its Open receipt could become durable.
    assert!(ledger.begin(operation.clone()).await.unwrap().new);
    let bridge = Arc::new(crate::bridge::Bridge::start(&launch, &Default::default()).unwrap());
    supervisor.agents.write().await.insert(
        launch.agent_id.clone(),
        AgentProcess {
            launch: launch.clone(),
            bridge: Arc::clone(&bridge),
        },
    );
    let mut stale = close_successor.clone();
    stale.authority_fence = 4;
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::FenceAgentRestart(stale))
                .await
        )
        .code,
        "mutation_authority_stale"
    );
    assert!(!ledger.completed(operation.clone()).await.unwrap());
    let mut wrong_digest = close_successor.clone();
    wrong_digest.successor_open_request_sha256 = Some("d".repeat(64));
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::FenceAgentRestart(wrong_digest))
                .await
        )
        .code,
        "agent_process_close_unproved"
    );
    assert!(!bridge.is_ended());
    {
        let _gate = supervisor.operation_gate.lock().await;
        let _writer = supervisor.removal_writer_guard().unwrap().unwrap();
        let close = supervisor
            .prepare_restart_successor_close(&close_successor, 2, &launch.request_sha256)
            .await
            .unwrap();
        assert_eq!(close.operation_id, "agent:agent-1:close:2:v1");
        // Lose the request after committing the Close fence, before beginning Close.
    }
    supervisor.recover_journals().await.unwrap();
    assert!(ledger.completed(operation).await.unwrap());
    assert!(!bridge.is_ended());
    for _ in 0..2 {
        assert_eq!(
            supervisor.dispatch(close_request.clone()).await,
            ResponseFrame::FenceAgentRestart(close_successor.clone())
        );
    }
    assert!(bridge.is_ended());
    assert!(supervisor.bridge("agent-1").await.is_none());
    assert!(!ledger
        .active(crate::mutation_ledger::ProcessFilter::Any)
        .await
        .unwrap());
    assert!(matches!(
        failure(supervisor.dispatch(pinned).await).code.as_str(),
        "mutation_authority_retired" | "mutation_authority_stale"
    ));
    close_store(supervisor, ledger).await;
    cleanup(&config);
}
