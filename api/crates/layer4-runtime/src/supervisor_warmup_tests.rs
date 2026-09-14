use super::*;

#[tokio::test]
async fn warm_process_and_setup_are_reused_after_credential_binding() {
    let config = RuntimeConfig::for_test(root("warm-handoff"));
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    let installed = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install-warm-test",
            Vec::new(),
        )))
        .await;
    assert!(
        matches!(installed, ResponseFrame::Install(_)),
        "{installed:?}"
    );
    let workspace = prepare_workspace(&supervisor).await;
    let setup = WorkspaceSetupRequest::new(
        "workspace:wks_mutations:setup:v1",
        "wks_mutations",
        "project",
        vec!["printf prepared >> preparation-count".into()],
    );
    let prepared = supervisor
        .dispatch(RequestFrame::WorkspaceSetup(setup.clone()))
        .await;
    assert!(
        matches!(prepared, ResponseFrame::WorkspaceSetup(_)),
        "{prepared:?}"
    );
    let (_, worktree) = add_worktree(&supervisor, workspace.resolved_commit).await;
    let plan = home_plan("agent-1", b"prepared profile");
    let home = supervisor
        .dispatch(materialize_home_request(plan.clone()))
        .await;
    assert!(
        matches!(home, ResponseFrame::MaterializeAgentHome(_)),
        "{home:?}"
    );
    let request = launch(&config, &worktree, "while IFS= read -r line; do :; done");
    let opened = supervisor
        .dispatch(open_agent_request(request.clone()))
        .await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    let bridge = supervisor.bridge("agent-1").await.unwrap();
    assert!(!bridge.is_ended());
    assert_eq!(supervisor.phase().await, Phase::Installed);

    let owner_setup = WorkspaceSetupRequest::new(
        "workspace:wks_mutations:credential-setup:v1",
        "wks_mutations",
        "project",
        vec!["printf owner >> identity-count".into()],
    );
    assert_eq!(
        failure(
            supervisor
                .dispatch(RequestFrame::WorkspaceSetup(owner_setup.clone()))
                .await
        )
        .code,
        "credential_not_installed"
    );
    assert!(!config.home.join("project/identity-count").exists());

    credential(&supervisor).await;
    assert_eq!(
        supervisor
            .dispatch(RequestFrame::WorkspaceSetup(setup))
            .await,
        prepared
    );
    let identity = supervisor
        .dispatch(RequestFrame::WorkspaceSetup(owner_setup.clone()))
        .await;
    assert!(
        matches!(identity, ResponseFrame::WorkspaceSetup(_)),
        "{identity:?}"
    );
    assert_eq!(
        supervisor
            .dispatch(RequestFrame::WorkspaceSetup(owner_setup))
            .await,
        identity
    );
    assert_eq!(
        std::fs::read(config.home.join("project/preparation-count")).unwrap(),
        b"prepared"
    );
    assert_eq!(
        std::fs::read(config.home.join("project/identity-count")).unwrap(),
        b"owner"
    );
    let replay = supervisor
        .dispatch(RequestFrame::OpenAgent(FencedRequest::new(2, request)))
        .await;
    assert!(matches!(replay, ResponseFrame::OpenAgent(_)), "{replay:?}");
    assert!(Arc::ptr_eq(
        &bridge,
        &supervisor.bridge("agent-1").await.unwrap()
    ));
    let replay_home = supervisor
        .dispatch(RequestFrame::MaterializeAgentHome(FencedRequest::new(
            2, plan,
        )))
        .await;
    assert!(
        matches!(replay_home, ResponseFrame::MaterializeAgentHome(_)),
        "{replay_home:?}"
    );
    let closed = supervisor
        .dispatch(RequestFrame::CloseAgent(FencedRequest::new(
            3,
            AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1")),
        )))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)), "{closed:?}");
    cleanup(&config);
}
