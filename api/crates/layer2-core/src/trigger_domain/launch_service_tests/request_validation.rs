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
async fn launch_listing_requires_launch_authority_and_hides_other_owners() {
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
    assert_eq!(
        fixture
            .service
            .list(&fixture.caller, &LaunchPageRequest::default())
            .await
            .unwrap()
            .items,
        vec![launch]
    );
    let denied = Caller::new(SubjectId::new(VISITOR), Authorization::none());
    assert_eq!(
        fixture
            .service
            .list(&denied, &LaunchPageRequest::default())
            .await,
        Err(LaunchServiceError::Forbidden)
    );
    let other = Caller::new(
        SubjectId::new("other"),
        Authorization::new([Capability::LaunchTriggers]),
    );
    assert!(fixture
        .service
        .list(&other, &LaunchPageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
