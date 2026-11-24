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

#[tokio::test]
async fn activity_actor_references_survive_view_changes_and_name_resolution_failures() {
    let fixture = fixture(true);
    let mut expected = std::collections::BTreeMap::new();
    for subject in [
        Some("operator-a"),
        Some("unknown-directory-subject"),
        Some("fixture-service-principal"),
        None,
    ] {
        let mut record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded);
        record.actor = subject.map(SubjectId::new);
        expected.insert(record.id.0.clone(), subject);
        fixture.activity.append(&record).await.unwrap();
    }

    let mut references = std::collections::BTreeMap::new();
    for query in ["", "?view=summary"] {
        let response = send(
            &fixture.app,
            Method::GET,
            &format!("/api/v1/admin/activity{query}"),
            None,
            [Capability::ReadActivityLog],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let rows = body["items"].as_array().unwrap();
        assert_eq!(rows.len(), expected.len());
        for row in rows {
            let id = row["id"].as_str().unwrap();
            match expected[id] {
                Some(subject) => {
                    let reference = row["actor_reference"].as_str().unwrap();
                    assert!(reference.starts_with("actor-"));
                    assert_eq!(reference.len(), 22);
                    assert!(!reference.contains(subject));
                    match references.insert(id.to_owned(), reference.to_owned()) {
                        Some(previous) => assert_eq!(previous, reference),
                        None => assert!(query.is_empty()),
                    }
                    if subject == "operator-a" {
                        assert_eq!(row["actor_name"], "Morgan Park");
                    } else {
                        assert!(row["actor_name"].is_null());
                    }
                }
                None => {
                    assert!(row["actor"].is_null());
                    assert!(row.get("actor_reference").is_none());
                }
            }
        }
    }
    assert_eq!(
        references
            .values()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
}

#[tokio::test]
async fn activity_summary_is_explicit_preserves_full_records_and_refuses_bad_queries() {
    let available = fixture(true);
    let mut row = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("operator-a"));
    row.detail
        .record(layer2_core::DetailKey::ResourceName, "retained detail");
    available.activity.append(&row).await.unwrap();
    for (suffix, summary) in [("", false), ("?view=summary", true)] {
        let response = send(
            &available.app,
            Method::GET,
            &format!("/api/v1/admin/activity{suffix}"),
            None,
            [Capability::ReadActivityLog],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["items"][0]["actor_name"], "Morgan Park");
        assert_eq!(body["items"][0].get("detail").is_none(), summary);
        if !summary {
            assert_eq!(
                body["items"][0]["detail"]["resource_name"],
                "retained detail"
            );
        }
        let denied = send(
            &available.app,
            Method::GET,
            &format!("/api/v1/admin/activity{suffix}"),
            None,
            [Capability::ReadAllSandboxes],
        )
        .await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    }
    for suffix in [
        "view=unknown",
        "view=summary&limit=0",
        "view=summary&cursor=bad",
        "view=summary&cursor=1%3A",
        "view=summary&limit=201",
    ] {
        let response = send(
            &available.app,
            Method::GET,
            &format!("/api/v1/admin/activity?{suffix}"),
            None,
            [Capability::ReadActivityLog],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{suffix}");
    }
}

#[tokio::test]
async fn activity_audit_filter_keeps_security_events_and_requires_read_authority() {
    let fixture = fixture(true);
    for row in [
        ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded),
        ActivityRecord::new(
            ActivityAction::Read,
            ActivityOutcome::Refused("forbidden".into()),
        ),
        ActivityRecord::new(ActivityAction::Configure, ActivityOutcome::Succeeded),
    ] {
        fixture.activity.append(&row).await.unwrap();
    }
    let path = "/api/v1/admin/activity?view=summary&exclude_successful_reads=true";
    let denied = send(&fixture.app, Method::GET, path, None, []).await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let response = send(
        &fixture.app,
        Method::GET,
        path,
        None,
        [Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    let items = body["items"].as_array().unwrap();
    assert!(items.iter().any(|row| row["action"] == "configure"));
    assert!(items
        .iter()
        .any(|row| row["outcome"]["status"] == "refused"));
    assert!(items
        .iter()
        .all(|row| { row["action"] != "read" || row["outcome"]["status"] != "succeeded" }));
}
