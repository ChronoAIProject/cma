use super::*;

#[tokio::test]
async fn cancel_works_before_login_and_while_waiting_for_a_name() {
    let mut login = Fixture::new();
    login.dependencies.identities.lock().unwrap().clear();
    assert_eq!(login.begin().await.step, SetupStep::Login);
    assert_eq!(
        login.text("/choice CANCEL").await.step,
        SetupStep::Cancelled
    );

    let mut name = Fixture::new();
    assert_eq!(name.text("hello").await.step, SetupStep::Owner);
    assert_eq!(name.text("personal").await.step, SetupStep::Name);
    assert_eq!(name.text("cancel").await.step, SetupStep::Cancelled);
    assert_eq!(name.dependencies.provision_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn simple_text_edits_remain_editable_and_reach_the_reviewed_plan() {
    let mut fixture = Fixture::new();
    fixture.begin().await;
    fixture.text("personal").await;
    assert_eq!(fixture.text("custom").await.step, SetupStep::AgentCustom);
    let view = fixture
        .text("instructions: Explain papers in Chinese.")
        .await;
    assert_eq!(view.step, SetupStep::AgentCustom);
    assert_eq!(
        view.configuration.agent.system_prompt,
        "Explain papers in Chinese."
    );
    assert!(view.notice.is_some());
    let view = fixture.text("model: custom-model").await;
    assert_eq!(
        view.configuration.agent.model.as_deref(),
        Some("custom-model")
    );
    assert_eq!(fixture.text("done").await.step, SetupStep::Workspace);
    fixture.text("custom").await;
    let view = fixture
        .text("repository: https://github.com/example/project stable")
        .await;
    assert_eq!(view.step, SetupStep::WorkspaceCustom);
    assert!(matches!(view.configuration.workspace.location,
        WorkspaceLocation::GitRepository { ref url, ref reference }
        if url == "https://github.com/example/project" && reference.as_deref() == Some("stable")));
    assert_eq!(fixture.text("done").await.step, SetupStep::Sandbox);
    fixture.text("custom").await;
    let view = fixture.text("resources: 2000 4096").await;
    assert_eq!(view.step, SetupStep::SandboxCustom);
    assert_eq!(view.configuration.sandbox.cpu_millis, 2000);
    assert_eq!(view.configuration.sandbox.memory_mib, 4096);
    let view = fixture.text("sandbox:always_live").await;
    assert_eq!(view.step, SetupStep::Services);
    fixture.text("done").await;
    fixture.text("bot:external-bot").await;
    let review = fixture.text("public").await;
    assert_eq!(review.step, SetupStep::Review);
    assert_eq!(
        review.configuration.agent.system_prompt,
        "Explain papers in Chinese."
    );
    assert_eq!(review.configuration.sandbox.cpu_millis, 2000);
    assert!(review.configuration.availability.always_keep_alive);
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn invalid_text_edit_preserves_the_last_valid_configuration() {
    let mut fixture = Fixture::new();
    fixture.begin().await;
    fixture.text("personal").await;
    fixture.text("defaults").await;
    let before = fixture.text("custom").await;
    let event = fixture.event(SetupInput::Text(
        "repository: https://evil.invalid/repo".into(),
    ));
    assert!(matches!(
        fixture.advance(&event).await,
        Err(BotError::Invalid(_))
    ));
    let after = fixture.store.get(&fixture.actor).await.unwrap().unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(
        after.configuration.workspace,
        before.configuration.workspace
    );
    assert_eq!(after.step, SetupStep::WorkspaceCustom);
}

#[tokio::test]
async fn full_profile_customization_survives_restart_and_is_not_created_implicitly() {
    let mut fixture = Fixture::new();
    fixture.begin().await;
    fixture.text("personal").await;
    let before = fixture.text("custom").await;
    let mut agent = before.configuration.agent;
    agent.system_prompt = "Custom durable instructions".into();
    agent.model = Some("selected-model".into());
    agent.permission_level = PermissionLevel::Autonomous;
    let event = fixture.event(SetupInput::Text("{\"agent\":{...}}".into()));
    let view = fixture
        .service
        .advance(
            "binding",
            "binding-fixture",
            &event,
            SetupDraft::default(),
            SetupCustomization {
                agent: Some(agent.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(view.step, SetupStep::Workspace);
    fixture.service = BotSetupService::new(fixture.store.clone(), fixture.dependencies.clone());
    let view = fixture.text("custom").await;
    let mut workspace = view.configuration.workspace;
    workspace.setup_commands = vec!["cargo build".into()];
    workspace.location = WorkspaceLocation::GitRepository {
        url: "https://github.com/example/project".into(),
        reference: Some("main".into()),
    };
    let event = fixture.event(SetupInput::Text("{\"workspace\":{...}}".into()));
    let view = fixture
        .service
        .advance(
            "binding",
            "binding-fixture",
            &event,
            SetupDraft::default(),
            SetupCustomization {
                workspace: Some(workspace.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(view.step, SetupStep::Sandbox);
    let view = fixture.text("custom").await;
    let mut sandbox = view.configuration.sandbox;
    sandbox.cpu_millis = 2000;
    sandbox.memory_mib = 4096;
    sandbox.software_install_commands = vec!["apt-get update".into()];
    let availability = SandboxProfileAvailability {
        always_keep_alive: true,
        ..Default::default()
    };
    let event = fixture.event(SetupInput::Text("{\"sandbox\":{...}}".into()));
    let view = fixture
        .service
        .advance(
            "binding",
            "binding-fixture",
            &event,
            SetupDraft::default(),
            SetupCustomization {
                sandbox: Some(sandbox.clone()),
                availability: Some(availability),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(view.step, SetupStep::Services);
    fixture.text("done").await;
    fixture.text("bot:external-bot").await;
    let review = fixture.text("private").await;
    assert_eq!(review.configuration.agent, agent);
    assert_eq!(review.configuration.workspace, workspace);
    assert_eq!(review.configuration.sandbox.cpu_millis, sandbox.cpu_millis);
    assert_eq!(
        review.configuration.sandbox.software_install_commands,
        sandbox.software_install_commands
    );
    assert!(review.configuration.availability.always_keep_alive);
    assert_eq!(
        fixture.dependencies.provision_calls.load(Ordering::SeqCst),
        0
    );
}
