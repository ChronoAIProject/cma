use super::*;
use crate::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};

struct Prerequisite(Mutex<Result<(), SandboxCredentialPrerequisiteError>>);

#[async_trait]
impl SandboxCredentialPrerequisite for Prerequisite {
    async fn check(&self, owner: &SubjectId) -> Result<(), SandboxCredentialPrerequisiteError> {
        assert_eq!(owner.as_str(), VISITOR);
        self.0.lock().unwrap().clone()
    }
}

fn service(fixture: &Fixture, prerequisite: Arc<Prerequisite>) -> Arc<LaunchService> {
    LaunchService::new(
        fixture.definitions.clone(),
        fixture.profiles.clone(),
        fixture.launches.clone(),
        fixture.contexts.clone(),
        fixture.broker.clone(),
        Arc::new(LaunchPolicy {
            enabled: true,
            orchestration_paused: false,
            visitor_launches_per_day: 5,
        }),
        prerequisite,
    )
}

#[tokio::test]
async fn missing_credential_prerequisite_precedes_service_requirements_and_admission() {
    let fixture = fixture(
        default_reuse(),
        VISITOR,
        false,
        false,
        vec![ServiceId::new("github")],
    )
    .await;
    let prerequisite = Arc::new(Prerequisite(Mutex::new(Err(
        SandboxCredentialPrerequisiteError::SetupRequired,
    ))));
    let service = service(&fixture, prerequisite.clone());
    assert_eq!(
        service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("missing-credential")
            )
            .await
            .unwrap_err(),
        LaunchServiceError::CredentialSetupRequired
    );
    assert_eq!(fixture.broker.calls(), 0);
    assert!(service
        .list(&fixture.caller, &LaunchPageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
    *prerequisite.0.lock().unwrap() = Ok(());
    let created = service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("new-credential-command"),
        )
        .await
        .unwrap();
    *prerequisite.0.lock().unwrap() = Err(SandboxCredentialPrerequisiteError::SetupRequired);
    let replayed = service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("new-credential-command"),
        )
        .await
        .unwrap();
    assert_eq!(replayed.launch, created.launch);
}

#[tokio::test]
async fn start_refusal_keeps_draft_unchanged_then_can_be_retried() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, Vec::new()).await;
    let draft = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("draft-before-expiry"),
        )
        .await
        .unwrap()
        .launch;
    let prerequisite = Arc::new(Prerequisite(Mutex::new(Err(
        SandboxCredentialPrerequisiteError::SetupRequired,
    ))));
    let service = service(&fixture, prerequisite.clone());
    let request = StartLaunch {
        trust_confirmed_digest: None,
        acknowledgements: Vec::new(),
    };
    assert_eq!(
        service
            .start(&fixture.caller, &draft.id, request.clone())
            .await
            .unwrap_err(),
        LaunchServiceError::CredentialSetupRequired
    );
    assert_eq!(
        service.read(&fixture.caller, &draft.id).await.unwrap(),
        draft
    );
    *prerequisite.0.lock().unwrap() = Ok(());
    assert_eq!(
        service
            .start(&fixture.caller, &draft.id, request)
            .await
            .unwrap()
            .state,
        LaunchState::Resolving
    );
}

#[tokio::test]
async fn reused_sandbox_and_agent_do_not_require_new_credential_authority() {
    for context in [ExistingContext::SandboxPaused, ExistingContext::LiveAgent] {
        let fixture = fixture(default_reuse(), VISITOR, false, false, Vec::new()).await;
        fixture.set_context(context);
        let prerequisite = Arc::new(Prerequisite(Mutex::new(Err(
            SandboxCredentialPrerequisiteError::SetupRequired,
        ))));
        let service = service(&fixture, prerequisite);
        let created = service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("reused-no-credential"),
            )
            .await
            .unwrap()
            .launch;
        let started = service
            .start(
                &fixture.caller,
                &created.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            started.state,
            LaunchState::Ready | LaunchState::Resolving
        ));
    }
}

#[tokio::test]
async fn credential_dependency_outage_is_not_reported_as_setup_required() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, Vec::new()).await;
    let service = service(
        &fixture,
        Arc::new(Prerequisite(Mutex::new(Err(
            SandboxCredentialPrerequisiteError::Unavailable("fixture unavailable".into()),
        )))),
    );
    assert!(matches!(
        service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("credential-outage")
            )
            .await,
        Err(LaunchServiceError::Unavailable(_))
    ));
}
