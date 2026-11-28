use super::*;

#[tokio::test]
async fn owner_names_follow_authorized_cross_owner_reads_and_never_grant_control() {
    use std::sync::atomic::Ordering;
    let fixture = fixture(false);
    fixture
        .sandboxes
        .create_sandbox(sandbox("sbx_names", "owner-b", "runtime-a"))
        .await
        .unwrap();
    for path in [
        "/api/v1/admin/sandboxes",
        "/api/v1/admin/sandboxes/sbx_names",
        "/api/v1/admin/activity",
    ] {
        let response = send(&fixture.app, Method::GET, path, None, []).await;
        assert!(matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
        assert!(!String::from_utf8(body_bytes(response).await)
            .unwrap()
            .contains("Sam Lee"));
    }
    assert_eq!(fixture.names.calls.load(Ordering::SeqCst), 0);
    let page = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let page = response_json(page).await;
    assert_eq!(page["items"][0]["owner"], "owner-b");
    assert_eq!(page["items"][0]["owner_name"], "Sam Lee");
    let detail = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_names",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    let detail = response_json(detail).await;
    assert_eq!(detail["owner_name"], "Sam Lee");
    assert_eq!(detail["sandbox"]["owner"], "owner-b");
    let activity = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/activity",
        None,
        [Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(
        response_json(activity).await["items"][0]["actor_name"],
        "Morgan Park"
    );
    let refused = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/sandboxes/sbx_names/retire",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn settings_support_read_replace_presence_aware_patch_and_reset() {
    let fixture = fixture(false);
    let capability = [Capability::ConfigurePlatform];
    let initial = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/settings",
        None,
        capability,
    )
    .await;
    assert_eq!(initial.status(), StatusCode::OK);
    let initial = response_json(initial).await;
    assert_eq!(initial["revision"], 0);
    assert!(initial["settings"].get("checkpoints").is_none());
    assert!(initial["settings"]["retention"]
        .get("checkpoint_grace_days")
        .is_none());

    let changed = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": false}
        })),
        capability,
    )
    .await;
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = response_json(changed).await;
    assert_eq!(changed["revision"], 1);
    assert_eq!(changed["settings"]["marketplace"]["enabled"], false);
    assert!(changed["settings"].get("checkpoints").is_none());
    assert!(changed["settings"]["retention"]
        .get("checkpoint_grace_days")
        .is_none());
    assert_eq!(
        changed["settings"]["inactivity"],
        initial["settings"]["inactivity"]
    );

    let mut replacement = changed["settings"].clone();
    replacement["limits"]["max_sandboxes_per_user"] = 9.into();
    let replaced = send(
        &fixture.app,
        Method::PUT,
        "/api/v1/admin/settings",
        Some(json!({"expected_revision": 1, "settings": replacement})),
        capability,
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    assert_eq!(
        response_json(replaced).await["settings"]["limits"]["max_sandboxes_per_user"],
        9
    );

    let reset = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/settings/reset",
        Some(json!({"expected_revision": 2})),
        capability,
    )
    .await;
    assert_eq!(reset.status(), StatusCode::OK);
    let reset = response_json(reset).await;
    assert_eq!(reset["revision"], 3);
    assert_eq!(reset["settings"], initial["settings"]);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn platform_settings_attribution_requires_activity_permission_and_covers_only_changed_settings(
) {
    let fixture = fixture(false);
    let changed = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({"expected_revision": 0, "marketplace": {"enabled": false}})),
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(changed.status(), StatusCode::OK);
    assert!(response_json(changed)
        .await
        .get("setting_changes")
        .is_none());

    let read = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/settings",
        None,
        [Capability::ConfigurePlatform, Capability::ReadActivityLog],
    )
    .await;
    assert_eq!(read.status(), StatusCode::OK);
    let read = response_json(read).await;
    let changes = read["setting_changes"].as_object().unwrap();
    assert_eq!(changes.len(), 1);
    let stamp = &changes["marketplace"];
    assert_eq!(stamp["revision"], 1);
    assert!(stamp["actor_reference"]
        .as_str()
        .unwrap()
        .starts_with("actor-"));
    assert!(stamp.get("actor").is_none());
    assert!(layer2_core::seconds_from_rfc3339(stamp["at"].as_str().unwrap()).is_some());
}

#[tokio::test]
async fn settings_refusals_are_confidential_specific_and_revision_safe() {
    let fixture = fixture(false);
    let capability = [Capability::ConfigurePlatform];
    let unknown = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": true, "private_token": "must-not-return"}
        })),
        capability,
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let unknown = response_json(unknown).await;
    assert_eq!(unknown["field"], "private_token");
    assert_eq!(unknown["path"], "marketplace.private_token");
    assert!(!unknown.to_string().contains("must-not-return"));

    for body in [
        json!({"expected_revision": 0}),
        json!({"expected_revision": 0, "marketplace": null}),
        json!({"expected_revision": 0, "inactivity": {"pause_after_seconds": 59}}),
        json!({"expected_revision": 0, "checkpoints": {"interval_seconds": 1800}}),
    ] {
        let response = send(
            &fixture.app,
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(body),
            capability,
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response_json(response).await["code"], "invalid_request");
    }

    let malformed = fixture
        .app
        .clone()
        .oneshot(raw_request(
            Method::PATCH,
            "/api/v1/admin/settings",
            r#"{"expected_revision":0,"marketplace":"credential-value""#,
            capability,
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let malformed = response_json(malformed).await;
    assert_eq!(malformed["code"], "malformed_json");
    assert!(!malformed.to_string().contains("credential-value"));

    let written = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": false}
        })),
        capability,
    )
    .await;
    assert_eq!(written.status(), StatusCode::OK);
    let stale = send(
        &fixture.app,
        Method::PATCH,
        "/api/v1/admin/settings",
        Some(json!({
            "expected_revision": 0,
            "marketplace": {"enabled": true}
        })),
        capability,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(stale).await["code"], "settings_conflict");
}

#[tokio::test]
async fn collection_operations_return_the_explicit_operator_refusal() {
    let fixture = fixture(true);
    for (method, path, body) in [
        (Method::GET, "/api/v1/admin/settings", None),
        (
            Method::PUT,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "settings": settings::PlatformSettingsBody::from(PlatformSettings::default())
            })),
        ),
        (
            Method::PATCH,
            "/api/v1/admin/settings",
            Some(json!({
                "expected_revision": 0,
                "marketplace": {"enabled": false}
            })),
        ),
        (
            Method::POST,
            "/api/v1/admin/settings/reset",
            Some(json!({"expected_revision": 0})),
        ),
        (Method::GET, "/api/v1/admin/sandboxes", None),
        (Method::GET, "/api/v1/admin/profiles", None),
        (Method::GET, "/api/v1/admin/runtime-inventory", None),
        (Method::GET, "/api/v1/admin/activity", None),
        (Method::POST, "/api/v1/admin/triggers/pause", None),
        (Method::POST, "/api/v1/admin/triggers/resume", None),
    ] {
        let response = send(&fixture.app, method, path, body, []).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
        let body = response_json(response).await;
        assert!(body["type"].as_str().unwrap().ends_with("/not-an-operator"));
    }
}

#[tokio::test]
async fn trigger_controls_project_the_platform_kill_switch() {
    let fixture = fixture(false);
    let paused = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/triggers/pause",
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(paused.status(), StatusCode::OK);
    let paused = response_json(paused).await;
    assert_eq!(paused["revision"], 1);
    assert_eq!(paused["settings"]["triggers"]["orchestration_paused"], true);

    let resumed = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/triggers/resume",
        None,
        [Capability::ConfigurePlatform],
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed = response_json(resumed).await;
    assert_eq!(resumed["revision"], 2);
    assert_eq!(
        resumed["settings"]["triggers"]["orchestration_paused"],
        false
    );
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn identifier_refusals_are_byte_identical_to_missing_targets() {
    let fixture = fixture(false);
    fixture
        .sandboxes
        .create_sandbox(sandbox("sbx_exists", "owner-a", "v2"))
        .await
        .unwrap();
    fixture
        .profiles
        .create_agent_profile(agent_profile(
            "agp_exists",
            "owner-a",
            ProfileVisibility::Private,
        ))
        .await
        .unwrap();

    for (method, existing, missing, capability) in [
        (
            Method::GET,
            "/api/v1/admin/sandboxes/sbx_exists",
            "/api/v1/admin/sandboxes/sbx_missing",
            Capability::ReadAllSandboxes,
        ),
        (
            Method::GET,
            "/api/v1/admin/sandboxes/sbx_exists/events",
            "/api/v1/admin/sandboxes/sbx_missing/events",
            Capability::ReadAllSandboxes,
        ),
        (
            Method::POST,
            "/api/v1/admin/sandboxes/sbx_exists/retire",
            "/api/v1/admin/sandboxes/sbx_missing/retire",
            Capability::ControlAnySandbox,
        ),
        (
            Method::POST,
            "/api/v1/admin/profiles/agent/agp_exists/retire",
            "/api/v1/admin/profiles/agent/agp_missing/retire",
            Capability::ControlAnySandbox,
        ),
    ] {
        let hidden = send(&fixture.app, method.clone(), existing, None, []).await;
        let absent = send(&fixture.app, method, missing, None, [capability]).await;
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND, "{existing}");
        assert_eq!(absent.status(), StatusCode::NOT_FOUND, "{missing}");
        assert_eq!(
            body_bytes(hidden).await,
            body_bytes(absent).await,
            "{existing}"
        );
    }
}

#[tokio::test]
async fn sandbox_routes_filter_read_events_and_retire_idempotently() {
    let fixture = fixture(false);
    let wanted = sandbox("sbx_wanted", "owner-a", "v2");
    fixture
        .sandboxes
        .create_sandbox(wanted.clone())
        .await
        .unwrap();
    fixture
        .sandboxes
        .create_sandbox(sandbox("sbx_other", "owner-b", "v1"))
        .await
        .unwrap();
    fixture
        .sandboxes
        .append_sandbox_event(&wanted.id, NOW.into(), SandboxEventKind::Created)
        .await
        .unwrap();

    let listed = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes?owner=owner-a&status=active&runtime_version=v2",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["id"], "sbx_wanted");
    assert_eq!(listed["items"][0]["agent_count"], 0);

    let detail = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_wanted",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = response_json(detail).await;
    assert_eq!(detail["sandbox"]["owner"], "owner-a");
    assert_eq!(detail["workspaces"], json!([]));
    assert_eq!(detail["agents"], json!([]));

    let events = send(
        &fixture.app,
        Method::GET,
        "/api/v1/admin/sandboxes/sbx_wanted/events?after=0&limit=10",
        None,
        [Capability::ReadAllSandboxes],
    )
    .await;
    assert_eq!(events.status(), StatusCode::OK);
    assert_eq!(response_json(events).await[0]["kind"]["type"], "created");

    let first = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/sandboxes/sbx_wanted/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first = body_bytes(first).await;
    let repeated = send(
        &fixture.app,
        Method::POST,
        "/api/v1/admin/sandboxes/sbx_wanted/retire",
        None,
        [Capability::ControlAnySandbox],
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(first, body_bytes(repeated).await);
    let events = fixture
        .sandboxes
        .sandbox_events(&wanted.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(fixture.activity.records.lock().unwrap().len(), 5);
}
