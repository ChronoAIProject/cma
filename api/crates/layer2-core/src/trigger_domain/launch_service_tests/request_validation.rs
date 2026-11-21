use super::*;

#[tokio::test]
async fn requested_placements_must_exist_and_remain_live() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let placement = fixture
        .definitions
        .create_placement(
            &fixture.public.trigger_id,
            &fixture.public.owner,
            "https://example.com/readme".into(),
            "README".into(),
        )
        .await
        .unwrap();
    let mut request = fixture.request("known-placement");
    request.placement_id = Some(placement.id.clone());
    assert_eq!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, request)
            .await
            .unwrap()
            .launch
            .placement_id,
        Some(placement.id.clone())
    );

    let mut unknown = fixture.request("unknown-placement");
    unknown.placement_id = Some(PlacementId("plc_unknown".into()));
    assert_eq!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, unknown)
            .await,
        Err(LaunchServiceError::InvalidRequest(
            "placement_id is unavailable".into()
        ))
    );
    fixture
        .definitions
        .retire_placement(
            &fixture.public.trigger_id,
            &fixture.public.owner,
            &placement.id,
            "2026-09-05T00:03:00Z".into(),
        )
        .await
        .unwrap();
    let mut retired = fixture.request("retired-placement");
    retired.placement_id = Some(placement.id);
    assert_eq!(
        fixture
            .service
            .create_or_resume(&fixture.caller, &fixture.public.public_id, retired)
            .await,
        Err(LaunchServiceError::InvalidRequest(
            "placement_id is unavailable".into()
        ))
    );
    assert_eq!(fixture.stats().await.launches_total, 1);
}

#[tokio::test]
async fn launch_listing_is_owner_scoped_without_current_launch_authority() {
    let fixture = fixture(default_reuse(), VISITOR, false, false, vec![]).await;
    let launch = fixture
        .service
        .create_or_resume(
            &fixture.caller,
            &fixture.public.public_id,
            fixture.request("listed-launch"),
        )
        .await
        .unwrap()
        .launch;
    let owner = Caller::new(SubjectId::new(VISITOR), Authorization::none());
    assert_eq!(
        fixture
            .service
            .list(&owner, &LaunchPageRequest::default())
            .await
            .unwrap()
            .items,
        vec![launch]
    );
    let other = Caller::new(SubjectId::new("other"), Authorization::none());
    assert!(fixture
        .service
        .list(&other, &LaunchPageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn request_bounds_and_requirement_identity_are_defensive() {
    let fixture = fixture(
        default_reuse(),
        VISITOR,
        false,
        false,
        vec![ServiceId::new("github"), ServiceId::new("slack")],
    )
    .await;
    for key in [
        String::new(),
        "has space".into(),
        "x".repeat(256),
        "snowman-\u{2603}".into(),
    ] {
        assert_eq!(
            fixture
                .service
                .create_or_resume(
                    &fixture.caller,
                    &fixture.public.public_id,
                    fixture.request(key),
                )
                .await
                .unwrap_err(),
            LaunchServiceError::InvalidIdempotencyKey
        );
    }
    fixture.broker.set(vec![
        requirement("slack", ServiceRequirementState::Satisfied),
        requirement("github", ServiceRequirementState::Satisfied),
    ]);
    assert!(matches!(
        fixture
            .service
            .create_or_resume(
                &fixture.caller,
                &fixture.public.public_id,
                fixture.request("wrong-order"),
            )
            .await,
        Err(LaunchServiceError::Corrupt(_))
    ));

    let mut invalid_start = StartLaunch {
        trust_confirmed_digest: None,
        acknowledgements: vec!["ok".into(); 33],
    };
    let launch_id = LaunchId::generate();
    assert!(matches!(
        fixture
            .service
            .start(&fixture.caller, &launch_id, invalid_start.clone())
            .await,
        Err(LaunchServiceError::InvalidRequest(_))
    ));
    invalid_start.acknowledgements = vec!["x".repeat(129)];
    assert!(matches!(
        fixture
            .service
            .start(&fixture.caller, &launch_id, invalid_start)
            .await,
        Err(LaunchServiceError::InvalidRequest(_))
    ));
}
