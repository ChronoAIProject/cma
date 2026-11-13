use super::*;

#[tokio::test]
async fn every_read_requires_only_read_all_sandboxes_before_store_access() {
    let fixture = fixture();
    let caller = caller([
        Capability::ControlAnySandbox,
        Capability::ConfigurePlatform,
        Capability::ReadEventLog,
    ]);
    let invalid_sandbox_query = SandboxAdminQuery {
        page: PageRequest {
            limit: 0,
            cursor: None,
        },
        ..SandboxAdminQuery::default()
    };
    let invalid_profile_query = ProfileAdminQuery {
        page: crate::profiles::ProfilePageRequest {
            limit: 0,
            cursor: None,
        },
        ..ProfileAdminQuery::default()
    };

    assert_eq!(
        fixture
            .service
            .list_sandboxes(&caller, &invalid_sandbox_query)
            .await,
        Err(OperatorError::Forbidden)
    );
    assert_eq!(
        fixture
            .service
            .read_sandbox(&caller, &SandboxId("sbx_missing".into()))
            .await,
        Err(OperatorError::Forbidden)
    );
    assert_eq!(
        fixture
            .service
            .sandbox_events(&caller, &SandboxId("sbx_missing".into()), 0, 0)
            .await,
        Err(OperatorError::Forbidden)
    );
    assert_eq!(
        fixture
            .service
            .list_profiles(&caller, &invalid_profile_query)
            .await,
        Err(OperatorError::Forbidden)
    );
    assert!(fixture.activity.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn sandbox_inventory_applies_every_filter_and_reads_child_summaries() {
    let fixture = fixture();
    let wanted = sandbox("sbx_wanted", "owner-a", SandboxStatus::Active, "v2");
    let other_owner = sandbox("sbx_owner", "owner-b", SandboxStatus::Active, "v2");
    let other_status = sandbox("sbx_status", "owner-a", SandboxStatus::Paused, "v2");
    let other_runtime = sandbox("sbx_runtime", "owner-a", SandboxStatus::Active, "v1");
    for value in [&wanted, &other_owner, &other_status, &other_runtime] {
        fixture
            .sandboxes
            .create_sandbox(value.clone())
            .await
            .unwrap();
    }
    let workspace = workspace(&wanted, "wks_wanted");
    fixture
        .sandboxes
        .create_workspace(workspace.clone())
        .await
        .unwrap();
    let working = agent(&wanted, &workspace, "agt_working", AgentStatus::Working);
    let deleted = agent(&wanted, &workspace, "agt_deleted", AgentStatus::Deleted);
    fixture
        .sandboxes
        .create_agent(working.clone())
        .await
        .unwrap();
    fixture.sandboxes.create_agent(deleted).await.unwrap();
    fixture
        .sandboxes
        .append_sandbox_event(&wanted.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();

    let operator = caller([Capability::ReadAllSandboxes]);
    let listed = fixture
        .service
        .list_sandboxes(
            &operator,
            &SandboxAdminQuery {
                owner: Some(SubjectId::new("owner-a")),
                status: Some(SandboxStatus::Active),
                runtime_version: Some("v2".into()),
                always_on: None,
                page: PageRequest::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, wanted.id);
    assert_eq!(listed.items[0].agent_count, 1);

    let detail = fixture
        .service
        .read_sandbox(&operator, &wanted.id)
        .await
        .unwrap();
    assert_eq!(detail.sandbox, wanted);
    assert_eq!(detail.workspaces[0].profile_revision, 2);
    assert_eq!(detail.agents.len(), 2);
    assert_eq!(detail.agents[1].profile_revision, 3);
    let events = fixture
        .service
        .sandbox_events(&operator, &detail.sandbox.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn missing_sandbox_events_are_not_reported_as_an_empty_log() {
    let fixture = fixture();
    let operator = caller([Capability::ReadAllSandboxes]);
    assert_eq!(
        fixture
            .service
            .sandbox_events(&operator, &SandboxId("sbx_missing".into()), 0, 10)
            .await,
        Err(OperatorError::NotFound)
    );
    assert!(fixture.activity.records.lock().unwrap().is_empty());
}
