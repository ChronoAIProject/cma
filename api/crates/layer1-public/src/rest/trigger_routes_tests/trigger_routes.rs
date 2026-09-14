use axum::http::{header, Method, StatusCode};
use serde_json::json;

use super::support_tests::*;

#[tokio::test]
async fn create_requires_publisher_authority_and_replays_exact_bytes() {
    let app = TestApp::new().await;
    let denied = request(
        Method::POST,
        "/api/v1/triggers",
        Some(trigger_body("always_new")),
        Some((VISITOR, visitor_authority())),
        Some("create-one"),
    );
    let (status, body) = json_response(&app.router, denied).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "forbidden");
    assert_eq!(app.operation_reservations(), 0);

    let first = raw_response(
        &app.router,
        publisher_request(
            Method::POST,
            "/api/v1/triggers",
            Some(trigger_body("always_new")),
            Some("create-one"),
        ),
    )
    .await;
    let replay = raw_response(
        &app.router,
        publisher_request(
            Method::POST,
            "/api/v1/triggers",
            Some(trigger_body("always_new")),
            Some("create-one"),
        ),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED);
    assert_eq!(replay.0, first.0);
    assert_eq!(replay.1, first.1);
    assert_eq!(replay.2, first.2);

    let (status, body) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            "/api/v1/triggers",
            Some(trigger_body("ask")),
            Some("create-one"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "idempotency_key_reused");
}

#[tokio::test]
async fn singular_trigger_routes_are_authorized_by_ownership() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "create-owner-auth").await;
    let id = trigger["id"].as_str().unwrap();
    let owner = || Some((OWNER, layer2_core::Authorization::none()));

    let (status, _) = json_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/triggers/{id}"),
            None,
            owner(),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, updated) = json_response(
        &app.router,
        request(
            Method::PUT,
            &format!("/api/v1/triggers/{id}"),
            Some(json!({
                "draft_version": 1,
                "content": trigger_body("ask")["content"]
            })),
            owner(),
            Some("owner-auth-update"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");

    let (status, published) = json_response(
        &app.router,
        request(
            Method::POST,
            &format!("/api/v1/triggers/{id}/publish"),
            Some(json!({ "draft_version": 2 })),
            owner(),
            Some("owner-auth-publish"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{published}");

    let (status, problem) = json_response(
        &app.router,
        request(Method::GET, "/api/v1/triggers", None, owner(), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(problem["code"], "forbidden");
}

#[tokio::test]
async fn owner_concealment_precedes_mutation_reservation_and_draft_cas_is_evidenced() {
    let app = TestApp::new().await;
    let created = create_trigger(&app, "always_new", "create-cas").await;
    let id = created["id"].as_str().unwrap();
    let before = app.operation_reservations();
    let foreign = request(
        Method::PUT,
        &format!("/api/v1/triggers/{id}"),
        Some(json!({ "draft_version": 1, "content": trigger_body("ask")["content"] })),
        Some((OTHER, publisher_authority())),
        Some("foreign-update"),
    );
    let (status, body) = json_response(&app.router, foreign).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "trigger_not_found");
    assert_eq!(app.operation_reservations(), before);

    let (status, updated) = json_response(
        &app.router,
        publisher_request(
            Method::PUT,
            &format!("/api/v1/triggers/{id}"),
            Some(json!({ "draft_version": 1, "content": trigger_body("ask")["content"] })),
            Some("owner-update"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["draft_version"], 2);

    let (status, conflict) = json_response(
        &app.router,
        publisher_request(
            Method::PUT,
            &format!("/api/v1/triggers/{id}"),
            Some(json!({ "draft_version": 1, "content": trigger_body("ask")["content"] })),
            Some("stale-update"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["code"], "trigger_draft_changed");
    assert_eq!(conflict["expected"], 1);
    assert_eq!(conflict["actual"], 2);

    let deletion = raw_response(
        &app.router,
        publisher_request(
            Method::DELETE,
            &format!("/api/v1/triggers/{id}"),
            None,
            Some("delete-draft"),
        ),
    )
    .await;
    assert_eq!(deletion.0, StatusCode::NO_CONTENT);
    assert!(deletion.2.is_empty());
    let replay = raw_response(
        &app.router,
        publisher_request(
            Method::DELETE,
            &format!("/api/v1/triggers/{id}"),
            None,
            Some("delete-draft"),
        ),
    )
    .await;
    assert_eq!(replay.0, deletion.0);
    assert_eq!(replay.2, deletion.2);
}

#[tokio::test]
async fn publication_placements_preview_revisions_stats_and_retirement_form_one_contract() {
    let app = TestApp::new().await;
    let created = create_trigger(&app, "always_new", "create-publish").await;
    let id = created["id"].as_str().unwrap();
    let (status, placement) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{id}/placements"),
            Some(json!({
                "source_url": "https://source.example/readme?view=1#section",
                "label": "README"
            })),
            Some("placement-create"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{placement}");
    assert_eq!(
        placement["source_url"],
        "https://source.example/readme?view=1"
    );
    let placement_id = placement["id"].as_str().unwrap();

    let published = publish_trigger(&app, id, "publish-one").await;
    let public_id = published["public_id"].as_str().unwrap();
    assert_eq!(published["revision"], 1);
    assert_eq!(published["availability"], "published");

    let (status, headers, preview) = raw_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/triggers/public/{public_id}"),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CACHE_CONTROL], "public, max-age=60");
    assert_eq!(headers[header::REFERRER_POLICY], "no-referrer");
    let preview: serde_json::Value = serde_json::from_slice(&preview).unwrap();
    assert_eq!(preview["publisher_display_name"], "CMA publisher");
    assert_eq!(
        preview["first_message"].as_str().unwrap().chars().count(),
        140
    );
    let encoded = preview.to_string();
    assert!(preview.get("owner").is_none());
    assert!(!encoded.contains("sbp_route_sandbox"));
    assert!(!encoded.contains(&"x".repeat(180)));

    let (status, revisions) = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{id}/revisions"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revisions.as_array().unwrap().len(), 1);

    let (status, stats) = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{id}/stats"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stats}");
    assert_eq!(stats["totals"]["launches_total"], 0);
    assert_eq!(stats["placements"][0]["placement_id"], placement_id);

    let (status, retired_placement) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{id}/placements/{placement_id}/retire"),
            None,
            Some("placement-retire"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(retired_placement["retired_at"].is_string());
    let (status, placements) = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{id}/placements"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(placements, json!([]));

    let (status, retired) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{id}/retire"),
            None,
            Some("trigger-retire"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retired["availability"], "retired");
    let (status, public_retired) = json_response(
        &app.router,
        request(
            Method::GET,
            &format!("/api/v1/triggers/public/{public_id}"),
            None,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(public_retired["availability"], "retired");
}
