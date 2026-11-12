use super::*;

#[tokio::test]
async fn profile_routes_filter_retire_and_protect_deployment_defaults() {
    let fixture = fixture(false);
    for profile in [
        agent_profile("agp_public", "owner-a", ProfileVisibility::Public),
        agent_profile("agp_private", "owner-b", ProfileVisibility::Private),
        agent_profile(
            layer2_core::DEFAULT_AGENT_PROFILE_ID,
            "system",
            ProfileVisibility::Public,
        ),
    ] {
        fixture
            .profiles
            .create_agent_profile(profile)
            .await
            .unwrap();
    }
    let listed = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/profiles?kind=agent&visibility=public&owner=owner-a",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["id"], "agp_public");

    let first = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/profiles/agent/agp_public/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = body_bytes(first).await;
    let repeated = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/profiles/agent/agp_public/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(first, body_bytes(repeated).await);

    let protected = send(
        &fixture.app,
        Method::POST,
        &format!(
            "/api/v1/admin/profiles/agent/{}/retire",
            layer2_core::DEFAULT_AGENT_PROFILE_ID
        ),
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(protected.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(protected).await["code"], "profile_protected");

    let system = fixture
        .profiles
        .list_agent_profiles(
            Some(&SubjectId::new("system")),
            false,
            &layer2_core::profiles::ProfilePageRequest::default(),
        )
        .await
        .unwrap();
    assert_eq!(system.items[0].retired_at, None);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn runtime_inventory_and_activity_each_require_their_exact_capability() {
    let available = fixture(true);
    available
        .activity
        .append(
            &ActivityRecord::new(ActivityAction::Configure, ActivityOutcome::Succeeded)
                .by(SubjectId::new("operator-a"))
                .on(ActivityTarget::Subject(SubjectId::new("owner-b"))),
        )
        .await
        .unwrap();

    let wrong = send(
        &available.app,
        Method::GET,
        "/api/v1/admin/runtime-inventory",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let inventory = send(
        &available.app,
        Method::GET,
        "/api/v1/admin/runtime-inventory",
        None,
        [Capability::ReadRuntimeInventory],
    )
    .await;
    assert_eq!(inventory.status(), StatusCode::OK);
    let inventory = response_json(inventory).await;
    assert_eq!(inventory["runtimes"][0]["generation"], 3);
    assert_eq!(inventory["runtimes"][0]["status"], "ready");

    let activity = send(
        &available.app,
        Method::GET,
        "/api/v1/admin/activity?action=configure&limit=1",
        None,
        [Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(activity.status(), StatusCode::OK);
    let activity = response_json(activity).await;
    assert_eq!(activity["items"][0]["action"], "configure");
    assert_eq!(activity["items"][0]["actor_name"], "Morgan Park");
    assert_eq!(activity["items"][0]["target_name"], "Sam Lee");

    let unavailable = fixture(false);
    let response = send(
        &unavailable.app,
        Method::GET,
        "/api/v1/admin/runtime-inventory",
        None,
        [Capability::ReadRuntimeInventory],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
