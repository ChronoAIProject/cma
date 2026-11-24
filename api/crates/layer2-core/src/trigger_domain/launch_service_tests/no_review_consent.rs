use super::*;

fn services() -> Vec<ServiceId> {
    ["ornn-api", "api-github", "chrono-llm-public"]
        .into_iter()
        .map(ServiceId::new)
        .collect()
}

fn requirements(connected: usize) -> Vec<ServiceRequirement> {
    services()
        .into_iter()
        .enumerate()
        .map(|(index, service)| ServiceRequirement {
            service,
            state: if index < connected {
                ServiceRequirementState::Satisfied
            } else {
                ServiceRequirementState::Missing
            },
        })
        .collect()
}

#[tokio::test]
async fn no_review_click_retains_exact_consent_through_connection_setup_and_restart() {
    let fixture = fixture(default_reuse(), PUBLISHER, true, false, services()).await;
    fixture.broker.set(requirements(0));
    let mut request = fixture.request("no-review-connect");
    request.start_if_ready = true;
    let draft = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request.clone())
        .await
        .unwrap()
        .launch;
    assert_eq!(draft.state, LaunchState::Draft);
    assert_eq!(draft.confirmations.len(), 1);
    assert!(draft.trust.confirmation_digest.is_some());
    assert_eq!(
        draft.confirmations[0].trust_confirmed_digest,
        draft.trust.confirmation_digest
    );
    assert_eq!(
        draft.confirmations[0].surface,
        LaunchConsentSurface::Landing
    );
    assert_eq!(
        draft.review.needs,
        services()
            .into_iter()
            .map(|service| ReviewNeed::Connection { service })
            .collect::<Vec<_>>()
    );
    assert!(draft.started_at.is_none());
    let check = draft
        .steps
        .iter()
        .find(|step| step.key == LaunchStepKey::CheckingRequirements)
        .unwrap();
    assert_eq!(check.status, LaunchStepStatus::Blocked);
    assert!(check.completed_at.is_none());
    assert_eq!(check.safe_detail, "Connect required accounts to continue");
    assert_eq!(draft.work.step, LaunchWorkStep::None);
    assert!(fixture
        .launches
        .due_launches(&crate::now_rfc3339(), 10)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        serde_json::from_value::<Launch>(serde_json::to_value(&draft).unwrap()).unwrap(),
        draft
    );
    let replay = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
        .await
        .unwrap();
    assert!(!replay.created);
    assert_eq!(replay.launch, draft);

    let restarted = fixture.service_with_policy(LaunchPolicy {
        enabled: true,
        orchestration_paused: false,
        visitor_launches_per_day: 5,
    });
    for connected in 0..3 {
        fixture.broker.set(requirements(connected));
        assert_eq!(
            restarted
                .start(
                    &fixture.caller,
                    &draft.id,
                    StartLaunch {
                        trust_confirmed_digest: None,
                        acknowledgements: Vec::new(),
                    }
                )
                .await
                .unwrap_err(),
            LaunchServiceError::RequirementsNotMet {
                services: services()[connected..].to_vec()
            }
        );
        let refreshed = restarted.read(&fixture.caller, &draft.id).await.unwrap();
        assert_eq!(refreshed.confirmations, draft.confirmations);
        assert!(refreshed
            .review
            .needs
            .iter()
            .all(|need| matches!(need, ReviewNeed::Connection { .. })));
        assert_eq!(refreshed.state, LaunchState::Draft);
        assert_eq!(
            refreshed
                .steps
                .iter()
                .find(|step| step.key == LaunchStepKey::CheckingRequirements)
                .unwrap(),
            check
        );
    }
    fixture.broker.set(requirements(3));
    let started = restarted
        .start(
            &fixture.caller,
            &draft.id,
            StartLaunch {
                trust_confirmed_digest: None,
                acknowledgements: Vec::new(),
            },
        )
        .await
        .unwrap();
    assert_eq!(started.state, LaunchState::Resolving);
    assert_eq!(started.confirmations, draft.confirmations);
    assert_eq!(started.trust, draft.trust);
    assert_eq!(started.revision_digest, draft.revision_digest);
    assert!(started.review.needs.is_empty());
    let completed = started
        .steps
        .iter()
        .find(|step| step.key == LaunchStepKey::CheckingRequirements)
        .unwrap();
    assert_eq!(completed.status, LaunchStepStatus::Completed);
    assert_eq!(completed.completed_at, started.started_at);
    assert_eq!(completed.safe_detail, "Required accounts are connected");
    assert_eq!(started.first_message, FirstMessageDelivery::Queued);
    let events = restarted
        .events(&fixture.caller, &draft.id, 0, None)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == LaunchEventKind::ConfirmationRecorded)
            .count(),
        1
    );
    assert_eq!(fixture.stats().await.launches_total, 1);
    assert_eq!(events.iter().filter(|event| matches!(&event.kind, LaunchEventKind::StepChanged { step } if step.key == LaunchStepKey::CheckingRequirements && step.status == LaunchStepStatus::Completed)).count(), 1);
}

#[tokio::test]
async fn publisher_review_and_nonstarting_requests_do_not_record_implicit_consent() {
    for (always_review, start_if_ready) in [(true, true), (false, false)] {
        let fixture = fixture(default_reuse(), PUBLISHER, true, always_review, services()).await;
        fixture.broker.set(requirements(0));
        let mut request = fixture.request("requires-review");
        request.start_if_ready = start_if_ready;
        let draft = fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
            .await
            .unwrap()
            .launch;
        assert!(draft.confirmations.is_empty());
        assert_eq!(
            draft.review.trust_digest(),
            draft.trust.confirmation_digest.as_deref()
        );
        fixture.broker.set(requirements(3));
        assert_eq!(
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
                .await
                .unwrap_err(),
            LaunchServiceError::TrustConfirmationMismatch
        );
        let refreshed = fixture
            .service
            .read(&fixture.caller, &draft.id)
            .await
            .unwrap();
        assert!(refreshed.confirmations.is_empty());
    }
}

#[tokio::test]
async fn confirmed_draft_cancellation_does_not_start_work_or_remove_consent() {
    let fixture = fixture(default_reuse(), PUBLISHER, true, false, services()).await;
    fixture.broker.set(requirements(0));
    let mut request = fixture.request("cancel-confirmed");
    request.start_if_ready = true;
    let draft = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
        .await
        .unwrap()
        .launch;
    let cancelled = fixture
        .service
        .cancel(&fixture.caller, &draft.id)
        .await
        .unwrap();
    assert_eq!(cancelled.state, LaunchState::Cancelled);
    assert_eq!(cancelled.confirmations, draft.confirmations);
    assert!(cancelled.started_at.is_none());
    assert!(!cancelled.is_worker_due(&crate::now_rfc3339()));
    cancelled.validate().unwrap();
    assert_eq!(
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
            .await
            .unwrap_err(),
        LaunchServiceError::NotStartable
    );
}

#[tokio::test]
async fn recorded_landing_consent_cannot_be_forged_or_overwritten() {
    let fixture = fixture(default_reuse(), PUBLISHER, true, false, services()).await;
    fixture.broker.set(requirements(0));
    let mut request = fixture.request("frozen-confirmation");
    request.start_if_ready = true;
    let draft = fixture
        .service
        .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
        .await
        .unwrap()
        .launch;
    for mutation in 0..4 {
        let mut changed = draft.clone();
        match mutation {
            0 => changed.confirmations[0].surface = LaunchConsentSurface::Review,
            1 => changed.confirmations[0].trust_confirmed_digest = Some("b".repeat(64)),
            2 => changed.confirmations.push(changed.confirmations[0].clone()),
            _ => changed.review.needs.push(ReviewNeed::OwnerRequiresReview),
        }
        assert!(changed.validate().is_err());
    }
    let mut changed = draft.clone();
    changed.confirmations[0]
        .acknowledgements
        .push("changed-after-click".into());
    changed.revision += 1;
    assert!(matches!(
        fixture
            .launches
            .commit_launch(LaunchCommit {
                expected_revision: draft.revision,
                event_at: changed.updated_at.clone(),
                launch: changed,
                events: Vec::new(),
            })
            .await,
        Err(LaunchStoreError::Corrupt(_))
    ));
    assert_eq!(
        fixture
            .service
            .read(&fixture.caller, &draft.id)
            .await
            .unwrap(),
        draft
    );
    fixture.broker.set(requirements(3));
    assert_eq!(
        fixture
            .service
            .start(
                &fixture.caller,
                &draft.id,
                StartLaunch {
                    trust_confirmed_digest: Some("b".repeat(64)),
                    acknowledgements: Vec::new(),
                }
            )
            .await
            .unwrap_err(),
        LaunchServiceError::TrustConfirmationMismatch
    );
    let refreshed = fixture
        .service
        .read(&fixture.caller, &draft.id)
        .await
        .unwrap();
    assert_eq!(refreshed.confirmations, draft.confirmations);
}
