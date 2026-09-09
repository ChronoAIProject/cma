use super::*;

fn services() -> Vec<ServiceId> {
    ["chrono-llm", "ornn-api", "api-github"]
        .into_iter()
        .map(ServiceId::new)
        .collect()
}

fn requirements(state: ServiceRequirementState) -> Vec<ServiceRequirement> {
    vec![
        requirement("chrono-llm", ServiceRequirementState::Satisfied),
        requirement("ornn-api", ServiceRequirementState::Satisfied),
        requirement("api-github", state),
    ]
}

#[tokio::test]
async fn reused_parents_and_agents_cannot_skip_missing_service_checks() {
    for context in [
        ExistingContext::SandboxPaused,
        ExistingContext::Workspace,
        ExistingContext::LiveAgent,
    ] {
        for state in [
            ServiceRequirementState::Missing,
            ServiceRequirementState::Inactive,
            ServiceRequirementState::Unknown,
        ] {
            let fixture = fixture(default_reuse(), VISITOR, false, false, services()).await;
            fixture.set_context(context);
            fixture.broker.set(requirements(state));
            let before = fixture.contexts.state.lock().unwrap().sandbox.clone();
            let mut request = fixture.request(format!("missing-{context:?}-{state:?}"));
            request.start_if_ready = true;
            let draft = fixture
                .service
                .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
                .await
                .unwrap()
                .launch;
            assert_eq!(draft.state, LaunchState::Draft);
            assert!(matches!(
                draft.resolution.sandbox,
                ResolvedResource::Reused(_)
            ));
            assert_eq!(draft.requirements.services, requirements(state));
            assert_eq!(
                draft.review.needs,
                vec![ReviewNeed::Connection {
                    service: ServiceId::new("api-github")
                }]
            );
            assert_eq!(draft.confirmations.len(), 1);
            assert_eq!(
                draft.confirmations[0].surface,
                LaunchConsentSurface::Landing
            );
            assert!(draft.started_at.is_none());
            assert_eq!(draft.work.step, LaunchWorkStep::None);
            assert_eq!(fixture.broker.calls(), 1);
            assert_eq!(fixture.contexts.state.lock().unwrap().sandbox, before);
        }
    }
}

#[tokio::test]
async fn reused_agent_start_rechecks_each_service_and_preserves_the_conversation() {
    for missing_index in 0..3 {
        let fixture = fixture(default_reuse(), VISITOR, false, false, services()).await;
        fixture.set_context(ExistingContext::LiveAgent);
        let draft = fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request(format!("recheck-{missing_index}")),
            )
            .await
            .unwrap()
            .launch;
        let mut changed = requirements(ServiceRequirementState::Satisfied);
        changed[missing_index].state = ServiceRequirementState::Missing;
        fixture.broker.set(changed.clone());
        let start = StartLaunch {
            trust_confirmed_digest: None,
            acknowledgements: Vec::new(),
        };
        assert_eq!(
            fixture
                .service
                .start(&fixture.caller, &draft.id, start.clone())
                .await
                .unwrap_err(),
            LaunchServiceError::RequirementsNotMet {
                services: vec![changed[missing_index].service.clone()]
            }
        );
        let blocked = fixture
            .service
            .read(&fixture.caller, &draft.id)
            .await
            .unwrap();
        assert_eq!(blocked.state, LaunchState::Draft);
        assert_eq!(blocked.requirements.services, changed);
        assert_eq!(blocked.resolution, draft.resolution);
        assert!(blocked.confirmations.is_empty());

        fixture
            .broker
            .set(requirements(ServiceRequirementState::Satisfied));
        let started = fixture
            .service
            .start(&fixture.caller, &draft.id, start)
            .await
            .unwrap();
        assert_eq!(started.state, LaunchState::Resolving);
        assert_eq!(started.resolution, draft.resolution);
        assert_eq!(started.first_message, FirstMessageDelivery::Completed);
        assert_eq!(started.confirmations.len(), 1);
        assert!(started.review.needs.is_empty());
        assert_eq!(fixture.broker.calls(), 3);
        let events = fixture
            .service
            .events(&fixture.caller, &draft.id, 0, None)
            .await
            .unwrap();
        let changes = events
            .iter()
            .filter_map(|event| match &event.kind {
                LaunchEventKind::RequirementChanged { service, state } => Some((service, *state)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            changes,
            vec![
                (
                    &changed[missing_index].service,
                    ServiceRequirementState::Missing
                ),
                (
                    &changed[missing_index].service,
                    ServiceRequirementState::Satisfied
                ),
            ]
        );
        started.validate().unwrap();
    }
}

#[tokio::test]
async fn reused_launch_refuses_incomplete_or_foreign_requirement_results() {
    let correct = requirements(ServiceRequirementState::Satisfied);
    let mut foreign = correct.clone();
    foreign[2].service = ServiceId::new("unrequested-service");
    let mut reordered = correct.clone();
    reordered.reverse();
    for invalid in [Vec::new(), correct[..2].to_vec(), foreign, reordered] {
        let fixture = fixture(default_reuse(), VISITOR, false, false, services()).await;
        fixture.set_context(ExistingContext::LiveAgent);
        let draft = fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("valid-draft"),
            )
            .await
            .unwrap()
            .launch;
        fixture.broker.set(invalid);
        assert!(matches!(
            fixture
                .service
                .create_or_resume(
                    &fixture.caller,
                    &fixture.public.public_id,
                    fixture.request("invalid-check")
                )
                .await,
            Err(LaunchServiceError::Corrupt(_))
        ));
        assert!(matches!(
            fixture
                .service
                .start(
                    &fixture.caller,
                    &draft.id,
                    StartLaunch {
                        trust_confirmed_digest: None,
                        acknowledgements: Vec::new(),
                    }
                )
                .await,
            Err(LaunchServiceError::Corrupt(_))
        ));
        assert_eq!(
            fixture
                .service
                .read(&fixture.caller, &draft.id)
                .await
                .unwrap(),
            draft
        );
        assert_eq!(fixture.stats().await.launches_total, 1);
    }
}

#[tokio::test]
async fn reused_launch_outage_refuses_new_work_but_does_not_break_command_replay() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, services()).await;
    fixture.set_context(ExistingContext::LiveAgent);
    let mut request = fixture.request("reused-start");
    request.start_if_ready = true;
    let admitted = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request.clone())
        .await
        .unwrap()
        .launch;
    assert_eq!(admitted.state, LaunchState::Resolving);
    fixture.broker.set_unavailable(true);
    let replay = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, admitted);
    assert_eq!(fixture.broker.calls(), 1);
    assert!(matches!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("new-during-outage")
            )
            .await,
        Err(LaunchServiceError::Unavailable(_))
    ));
    assert_eq!(fixture.stats().await.launches_total, 1);
}

#[tokio::test]
async fn legacy_reused_draft_records_each_newly_checked_service() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, services()).await;
    fixture.set_context(ExistingContext::LiveAgent);
    let mut draft = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("legacy-reused-draft"),
        )
        .await
        .unwrap()
        .launch;
    let expected_revision = draft.revision;
    let now = crate::now_rfc3339();
    draft
        .refresh_review(
            RequirementsSnapshot {
                services: Vec::new(),
                checked_at: now.clone(),
            },
            draft.review.clone(),
            now.clone(),
        )
        .unwrap();
    fixture
        .launches
        .commit_launch(LaunchCommit {
            expected_revision,
            launch: draft.clone(),
            event_at: now,
            events: Vec::new(),
        })
        .await
        .unwrap();
    fixture
        .broker
        .set(requirements(ServiceRequirementState::Missing));
    assert!(matches!(
        fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: None,
                    acknowledgements: Vec::new(),
                }
            )
            .await,
        Err(LaunchServiceError::RequirementsNotMet { .. })
    ));
    let events = fixture
        .service
        .events(&fixture.caller, &draft.id, draft.event_head, None)
        .await
        .unwrap();
    let refreshed = fixture
        .service
        .read(&fixture.caller, &draft.id)
        .await
        .unwrap();
    let check = refreshed
        .steps
        .iter()
        .find(|step| step.key == LaunchStepKey::CheckingRequirements)
        .unwrap();
    assert_eq!(check.status, LaunchStepStatus::Blocked);
    assert!(check.completed_at.is_none());
    assert_eq!(
        events.iter().map(|event| &event.kind).collect::<Vec<_>>(),
        requirements(ServiceRequirementState::Missing)
            .into_iter()
            .map(|requirement| LaunchEventKind::RequirementChanged {
                service: requirement.service,
                state: requirement.state,
            })
            .chain([LaunchEventKind::StepChanged {
                step: check.clone()
            }])
            .collect::<Vec<_>>()
            .iter()
            .collect::<Vec<_>>()
    );
}
