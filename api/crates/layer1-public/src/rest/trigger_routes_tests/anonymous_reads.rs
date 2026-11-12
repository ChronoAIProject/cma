use axum::http::{header, Method, StatusCode};

use super::support_tests::*;

#[tokio::test]
async fn anonymous_reads_and_navigation_change_no_state() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "anonymous-read-create").await;
    let trigger_id = trigger["id"].as_str().unwrap();
    let (status, placement) = json_response(
        &app.router,
        publisher_request(
            Method::POST,
            &format!("/api/v1/triggers/{trigger_id}/placements"),
            Some(serde_json::json!({
                "source_url": "https://source.example/anonymous-read",
                "label": "README"
            })),
            Some("anonymous-read-placement"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{placement}");
    let placement_id = placement["id"].as_str().unwrap();
    let published = publish_trigger(&app, trigger_id, "anonymous-read-publish").await;
    let public_id = published["public_id"].as_str().unwrap();

    let before = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{trigger_id}"),
            None,
            None,
        ),
    )
    .await
    .1;
    let before_stats = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{trigger_id}/stats"),
            None,
            None,
        ),
    )
    .await
    .1;
    let before_operations = app.operation_reservations();

    for _ in 0..20 {
        let (status, _, _) = raw_response(
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

        let (status, _, _) = raw_response(
            &app.router,
            request(
                Method::GET,
                &format!("/api/v1/triggers/public/{public_id}/badge.svg"),
                None,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        for deliberate in [false, true] {
            let mut navigation = request(
                Method::GET,
                &format!("/api/v1/triggers/public/{public_id}/open/{placement_id}"),
                None,
                None,
                None,
            );
            if deliberate {
                navigation
                    .headers_mut()
                    .insert("sec-fetch-user", "?1".parse().unwrap());
                navigation
                    .headers_mut()
                    .insert("sec-fetch-mode", "navigate".parse().unwrap());
                navigation
                    .headers_mut()
                    .insert("sec-fetch-dest", "document".parse().unwrap());
            }
            let (status, headers, _) = raw_response(&app.router, navigation).await;
            assert_eq!(status, StatusCode::SEE_OTHER);
            assert!(headers.contains_key(header::SET_COOKIE));
        }
    }

    let after = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{trigger_id}"),
            None,
            None,
        ),
    )
    .await
    .1;
    let after_stats = json_response(
        &app.router,
        publisher_request(
            Method::GET,
            &format!("/api/v1/triggers/{trigger_id}/stats"),
            None,
            None,
        ),
    )
    .await
    .1;
    assert_eq!(after["draft_version"], before["draft_version"]);
    assert_eq!(after["updated_at"], before["updated_at"]);
    assert_eq!(after["stats"], before["stats"]);
    assert_eq!(after_stats, before_stats);
    assert_eq!(app.operation_reservations(), before_operations);

    let (status, launches) = json_response(
        &app.router,
        visitor_request(
            Method::GET,
            &format!("/api/v1/launches?trigger={trigger_id}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{launches}");
    assert_eq!(launches["items"], serde_json::json!([]));
}
