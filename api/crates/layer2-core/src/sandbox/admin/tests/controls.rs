use super::*;
use crate::profiles::ProfileVisibility;
use crate::sandbox::SandboxWorkStep;

#[tokio::test]
async fn stopped_retirement_retries_only_at_the_current_revision_and_original_step() {
    let fixture = fixture();
    let mut parent = sandbox(
        "sbx_stopped_retirement",
        "owner-a",
        SandboxStatus::Active,
        "v2",
    );
    let at = "2026-11-26T00:00:00Z";
    parent.begin_retirement(at.into()).unwrap();
    parent
        .work
        .schedule(SandboxWorkStep::RevokeRetirementCredentials, at.into());
    parent.work.attempts = crate::sandbox::work_retry::WORK_RETRY_BUDGET;
    let fence = parent
        .work
        .claim("worker".into(), at, "2026-11-26T00:01:00Z".into())
        .unwrap();
    parent.work.stop_retirement("worker", fence).unwrap();
    fixture
        .sandboxes
        .create_sandbox(parent.clone())
        .await
        .unwrap();
    let operator = caller([Capability::ControlAnySandbox]);
    assert!(
        !fixture
            .service
            .retire_sandbox(&operator, &parent.id, None)
            .await
            .unwrap()
            .changed
    );
    assert!(matches!(
        fixture
            .service
            .retire_sandbox(&operator, &parent.id, Some(parent.revision - 1))
            .await,
        Err(OperatorError::InvalidState(_))
    ));
    let result = fixture
        .service
        .retire_sandbox(&operator, &parent.id, Some(parent.revision))
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.sandbox.status, SandboxStatus::Retiring);
    assert_eq!(result.sandbox.work.step, parent.work.step);
    assert_eq!(result.sandbox.work.next_fence, parent.work.next_fence);
    assert_eq!(result.sandbox.work.attempts, 0);
    assert_eq!(result.sandbox.runtime, parent.runtime);
    assert_eq!(result.sandbox.credential, parent.credential);
}

#[tokio::test]
async fn retirement_uses_the_sandbox_transition_and_never_stops_an_agent_directly() {
    let fixture = fixture();
    let sandbox = sandbox("sbx_retire", "owner-a", SandboxStatus::Active, "v2");
    let workspace = workspace(&sandbox, "wks_retire");
    let working = agent(&sandbox, &workspace, "agt_retire", AgentStatus::Working);
    fixture
        .sandboxes
        .create_sandbox(sandbox.clone())
        .await
        .unwrap();
    fixture.sandboxes.create_workspace(workspace).await.unwrap();
    fixture
        .sandboxes
        .create_agent(working.clone())
        .await
        .unwrap();

    let operator = caller([Capability::ControlAnySandbox]);
    let retired = fixture
        .service
        .retire_sandbox(&operator, &sandbox.id, None)
        .await
        .unwrap();
    assert!(retired.changed);
    assert_eq!(retired.sandbox.status, SandboxStatus::Retiring);
    assert_eq!(retired.sandbox.work.step, SandboxWorkStep::StopAgents);
    assert!(retired.sandbox.work.lease.is_none());
    assert_eq!(
        fixture
            .sandboxes
            .get_agent(&working.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Working
    );
    let events = fixture
        .sandboxes
        .sandbox_events(&sandbox.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(
        events[0].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Retiring,
        }
    );

    let again = fixture
        .service
        .retire_sandbox(&operator, &sandbox.id, None)
        .await
        .unwrap();
    assert!(!again.changed);
    assert_eq!(
        fixture
            .sandboxes
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let records = fixture.activity.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| {
        record.actor.as_ref() == Some(&operator.subject)
            && record.detail.get(DetailKey::Subject) == Some("owner-a")
            && record.detail.get(DetailKey::Status) == Some("retiring")
    }));
}

#[tokio::test]
async fn any_nonterminal_sandbox_can_enter_retirement() {
    for status in [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
    ] {
        let fixture = fixture();
        let mut value = sandbox("sbx_any", "owner-a", status, "v2");
        if status == SandboxStatus::Failed {
            value.failure = Some(super::super::super::TypedFailure {
                class: "terminal_validation".into(),
                detail: "profile invalid".into(),
            });
        }
        fixture.sandboxes.create_sandbox(value).await.unwrap();
        let retired = fixture
            .service
            .retire_sandbox(
                &caller([Capability::ControlAnySandbox]),
                &SandboxId("sbx_any".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            retired.sandbox.status,
            SandboxStatus::Retiring,
            "{status:?}"
        );
    }
}

#[tokio::test]
async fn profile_inventory_filters_and_operator_retirement_are_audited() {
    let fixture = fixture();
    for profile in [
        agent_profile("agp_public_a", "owner-a", ProfileVisibility::Public),
        agent_profile("agp_private_a", "owner-a", ProfileVisibility::Private),
        agent_profile("agp_public_b", "owner-b", ProfileVisibility::Public),
    ] {
        fixture
            .profiles
            .create_agent_profile(profile)
            .await
            .unwrap();
    }
    let reader = caller([Capability::ReadAllSandboxes]);
    let page = fixture
        .service
        .list_profiles(
            &reader,
            &ProfileAdminQuery {
                kind: Some(ProfileKind::Agent),
                visibility: Some(ProfileVisibility::Public),
                owner: Some(SubjectId::new("owner-a")),
                page: crate::profiles::ProfilePageRequest::default(),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, "agp_public_a");

    let controller = caller([Capability::ControlAnySandbox]);
    let locator = ProfileLocator::Agent(crate::AgentProfileId("agp_public_b".into()));
    let retired = fixture
        .service
        .retire_profile(&controller, &locator)
        .await
        .unwrap();
    assert_eq!(retired.owner, SubjectId::new("owner-b"));
    assert!(retired.retired_at.is_some());
    let records = fixture.activity.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[1].target,
        Some(ActivityTarget::Resource(
            "profile:agent:agp_public_b".into()
        ))
    );
    assert_eq!(records[1].detail.get(DetailKey::ProfileKind), Some("agent"));
    assert_eq!(records[1].detail.get(DetailKey::Visibility), Some("public"));
    assert_eq!(records[1].detail.get(DetailKey::Subject), Some("owner-b"));
}

#[tokio::test]
async fn controls_require_only_control_any_sandbox_before_target_lookup() {
    let fixture = fixture();
    let caller = caller([Capability::ReadAllSandboxes, Capability::ConfigurePlatform]);
    assert_eq!(
        fixture
            .service
            .retire_sandbox(&caller, &SandboxId("sbx_missing".into()), None)
            .await,
        Err(OperatorError::Forbidden)
    );
    assert_eq!(
        fixture
            .service
            .retire_profile(
                &caller,
                &ProfileLocator::Sandbox(crate::SandboxProfileId("sbp_missing".into())),
            )
            .await,
        Err(OperatorError::Forbidden)
    );
    assert!(fixture.activity.records.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deployment_default_profiles_cannot_be_retired_by_an_operator() {
    let fixture = fixture();
    let operator = caller([Capability::ControlAnySandbox]);
    for locator in [
        ProfileLocator::Agent(crate::AgentProfileId(
            crate::DEFAULT_AGENT_PROFILE_ID.into(),
        )),
        ProfileLocator::Workspace(crate::WorkspaceProfileId(
            crate::DEFAULT_WORKSPACE_PROFILE_ID.into(),
        )),
        ProfileLocator::Sandbox(crate::SandboxProfileId(
            crate::DEFAULT_SANDBOX_PROFILE_ID.into(),
        )),
    ] {
        assert_eq!(
            fixture.service.retire_profile(&operator, &locator).await,
            Err(OperatorError::ProtectedProfile)
        );
    }
    assert!(fixture.activity.records.lock().unwrap().is_empty());
}
