use axum::http::{header, Method, StatusCode};
use layer2_core::trigger_domain::{BrokerHandoffState, ServiceRequirementState};
use serde_json::json;

use super::support_tests::*;

#[tokio::test]
async fn reviewed_launch_replays_without_recharging_and_projects_durable_progress() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "create-launch-trigger").await;
    let trigger_id = trigger["id"].as_str().unwrap();
    let published = publish_trigger(&app, trigger_id, "publish-launch-trigger").await;
    let public_id = published["public_id"].as_str().unwrap();
    let path = format!("/api/v1/triggers/public/{public_id}/launches");
    let first = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(true)),
            Some("launch-review"),
        ),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED);
    let launch: serde_json::Value = serde_json::from_slice(&first.2).unwrap();
    assert_eq!(launch["state"], "draft");
    assert_eq!(launch["review"]["can_start_now"], false);
    assert_eq!(launch["review"]["needs"][0]["type"], "connection");
    assert_eq!(launch["review"]["needs"][0]["service"], "github");
    assert!(launch.get("owner").is_none());
    assert!(launch.get("command_key_hash").is_none());
    let launch_id = launch["id"].as_str().unwrap();
    let event_head = launch["event_head"].as_u64().unwrap();
    assert_eq!(app.quota.reserve_calls(), 1);
    assert_eq!(app.quota.settlements(), [true]);

    let replay = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(true)),
            Some("launch-review"),
        ),
    )
    .await;
    assert_eq!(replay.0, first.0);
    assert_eq!(replay.1, first.1);
    assert_eq!(replay.2, first.2);
    assert_eq!(app.quota.reserve_calls(), 1);
    assert_eq!(app.quota.settlements(), [true]);

    let (status, requirements) = json_response(
        &app.router,
        visitor_request(
            Method::GET,
            &format!("/api/v1/launches/{launch_id}/requirements"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(requirements["services"][0]["state"], "missing");

    let (status, events) = json_response(
        &app.router,
        visitor_request(
            Method::GET,
            &format!("/api/v1/launches/{launch_id}/events?after=0&limit=10"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    assert_eq!(events["launch_id"], launch_id);
    assert_eq!(events["events"][0]["kind"]["type"], "created");
    assert_eq!(events["next_cursor"], event_head);

    let (status, empty) = json_response(
        &app.router,
        visitor_request(
            Method::GET,
            &format!("/api/v1/launches/{launch_id}/events?after={event_head}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["events"], json!([]));
    assert_eq!(empty["next_cursor"], event_head);

    let (status, beyond) = json_response(
        &app.router,
        visitor_request(
            Method::GET,
            &format!(
                "/api/v1/launches/{launch_id}/events?after={}",
                event_head + 1
            ),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(beyond["code"], "cursor_beyond_end");
    assert_eq!(beyond["head"], event_head);

    let (status, page) = json_response(
        &app.router,
        visitor_request(
            Method::GET,
            &format!("/api/v1/launches?trigger={trigger_id}&state=draft"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn immediate_and_reviewed_start_and_cancel_use_the_documented_statuses() {
    let immediate = TestApp::new().await;
    immediate
        .broker
        .set_requirement(ServiceRequirementState::Satisfied);
    let trigger = create_trigger(&immediate, "always_new", "create-immediate").await;
    let published = publish_trigger(
        &immediate,
        trigger["id"].as_str().unwrap(),
        "publish-immediate",
    )
    .await;
    let (status, launch) = json_response(
        &immediate.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(launch_body(true)),
            Some("start-immediately"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{launch}");
    assert_eq!(launch["state"], "resolving");

    let reviewed = TestApp::new().await;
    reviewed
        .broker
        .set_requirement(ServiceRequirementState::Satisfied);
    let trigger = create_trigger(&reviewed, "always_new", "create-reviewed").await;
    let published = publish_trigger(
        &reviewed,
        trigger["id"].as_str().unwrap(),
        "publish-reviewed",
    )
    .await;
    let public_id = published["public_id"].as_str().unwrap();
    let (status, draft) = json_response(
        &reviewed.router,
        visitor_request(
            Method::POST,
            &format!("/api/v1/triggers/public/{public_id}/launches"),
            Some(launch_body(false)),
            Some("review-before-start"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let launch_id = draft["id"].as_str().unwrap();
    let (status, started) = json_response(
        &reviewed.router,
        visitor_request(
            Method::POST,
            &format!("/api/v1/launches/{launch_id}/start"),
            Some(json!({})),
            Some("start-reviewed"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["state"], "resolving");

    let (status, cancellable) = json_response(
        &reviewed.router,
        visitor_request(
            Method::POST,
            &format!("/api/v1/triggers/public/{public_id}/launches"),
            Some(launch_body(false)),
            Some("cancel-this-launch"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let cancel_id = cancellable["id"].as_str().unwrap();
    let (status, cancelled) = json_response(
        &reviewed.router,
        visitor_request(
            Method::POST,
            &format!("/api/v1/launches/{cancel_id}/cancel"),
            None,
            Some("cancel-reviewed"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["state"], "cancelled");
}

#[tokio::test]
async fn existing_launch_routes_are_authorized_by_ownership() {
    let app = TestApp::new().await;
    app.broker
        .set_requirement(ServiceRequirementState::Satisfied);
    let trigger = create_trigger(&app, "always_new", "create-launch-owner-auth").await;
    let trigger_id = trigger["id"].as_str().unwrap();
    let published = publish_trigger(&app, trigger_id, "publish-launch-owner-auth").await;
    let public_id = published["public_id"].as_str().unwrap();
    let (status, launch) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &format!("/api/v1/triggers/public/{public_id}/launches"),
            Some(launch_body(false)),
            Some("create-launch-owner-auth"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{launch}");
    let launch_id = launch["id"].as_str().unwrap();
    let owner = || Some((VISITOR, layer2_core::Authorization::none()));

    for path in [
        format!("/api/v1/launches/{launch_id}"),
        format!("/api/v1/launches/{launch_id}/requirements"),
        format!("/api/v1/launches/{launch_id}/events"),
        "/api/v1/launches".to_owned(),
    ] {
        let (status, _) = json_response(
            &app.router,
            request(Method::GET, &path, None, owner(), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }

    let (status, started) = json_response(
        &app.router,
        request(
            Method::POST,
            &format!("/api/v1/launches/{launch_id}/start"),
            Some(json!({})),
            owner(),
            Some("start-launch-owner-auth"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
}

#[tokio::test]
async fn repeat_choice_and_visitor_quota_problems_include_bounded_actionable_evidence() {
    let repeat = TestApp::new().await;
    let trigger = create_trigger(&repeat, "ask", "create-repeat").await;
    let trigger_id = trigger["id"].as_str().unwrap();
    let published = publish_trigger(&repeat, trigger_id, "publish-repeat").await;
    repeat.set_existing_agent(trigger_id);
    let (status, problem) = json_response(
        &repeat.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(launch_body(true)),
            Some("repeat-choice"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(problem["code"], "trigger_repeat_choice_required");
    assert_eq!(problem["existing"]["agent_id"], "agt_existing");
    assert_eq!(problem["existing"]["title"], "Existing Agent");
    assert_eq!(
        problem["existing"]["first_message_excerpt"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        140
    );
    assert_eq!(repeat.quota.settlements(), [false]);

    let limited = TestApp::new().await;
    let trigger = create_trigger(&limited, "always_new", "create-limited").await;
    let published =
        publish_trigger(&limited, trigger["id"].as_str().unwrap(), "publish-limited").await;
    limited.quota.refuse();
    let path = format!(
        "/api/v1/triggers/public/{}/launches",
        published["public_id"].as_str().unwrap()
    );
    let first = raw_response(
        &limited.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("limited-launch"),
        ),
    )
    .await;
    assert_eq!(first.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(first.1.contains_key(header::RETRY_AFTER));
    let problem: serde_json::Value = serde_json::from_slice(&first.2).unwrap();
    assert_eq!(problem["code"], "resource_quota");
    assert_eq!(problem["resource"], "visitor_launches");
    assert_eq!(problem["limit"], 5);
    assert!(problem["reset_at"].is_string());
    let replay = raw_response(
        &limited.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("limited-launch"),
        ),
    )
    .await;
    assert_eq!(replay.0, first.0);
    assert_eq!(replay.1, first.1);
    assert_eq!(replay.2, first.2);
    assert_eq!(limited.quota.reserve_calls(), 1);
}

#[tokio::test]
async fn launch_reads_conceal_foreign_ownership() {
    let app = TestApp::new().await;
    app.broker
        .set_requirement(ServiceRequirementState::Satisfied);
    let trigger = create_trigger(&app, "always_new", "create-owned-launch").await;
    let published = publish_trigger(
        &app,
        trigger["id"].as_str().unwrap(),
        "publish-owned-launch",
    )
    .await;
    let (_, launch) = json_response(
        &app.router,
        visitor_request(
            Method::POST,
            &format!(
                "/api/v1/triggers/public/{}/launches",
                published["public_id"].as_str().unwrap()
            ),
            Some(launch_body(false)),
            Some("owned-launch"),
        ),
    )
    .await;
    let launch_id = launch["id"].as_str().unwrap();
    let (status, problem) = json_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/launches/{launch_id}"),
            None,
            Some((OTHER, visitor_authority())),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(problem["code"], "launch_not_found");

    app.broker.observe(BrokerHandoffState::Pending);
}
