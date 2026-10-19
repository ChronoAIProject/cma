use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support_tests::*;

#[tokio::test]
async fn archive_requires_owner_and_capability_and_replays_after_owner_record_disappears() {
    for retire_first in [false, true] {
        let app = TestApp::new().await;
        let created = create_trigger(&app, "always_new", "create-archive").await;
        let id = created["id"].as_str().unwrap();
        let published = publish_trigger(&app, id, "publish-archive").await;
        let path = format!("/api/v1/triggers/{id}");
        if retire_first {
            let (status, _) = json_response(
                &app.router,
                publisher_request(
                    Method::POST,
                    &format!("{path}/retire"),
                    None,
                    Some("retire-before-archive"),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
        }
        for (subject, authority, expected) in [
            (OWNER, visitor_authority(), StatusCode::FORBIDDEN),
            (VISITOR, publisher_authority(), StatusCode::NOT_FOUND),
        ] {
            let (status, _) = json_response(
                &app.router,
                request(
                    Method::DELETE,
                    &path,
                    None,
                    Some((subject, authority)),
                    Some("refused-archive"),
                ),
            )
            .await;
            assert_eq!(status, expected);
        }
        let deleted = raw_response(
            &app.router,
            publisher_request(Method::DELETE, &path, None, Some("archive-owned")),
        )
        .await;
        assert_eq!(deleted.0, StatusCode::NO_CONTENT);
        let replay = raw_response(
            &app.router,
            publisher_request(Method::DELETE, &path, None, Some("archive-owned")),
        )
        .await;
        assert_eq!(replay.0, deleted.0);
        assert_eq!(replay.2, deleted.2);
        let (status, _) = json_response(
            &app.router,
            publisher_request(Method::GET, &path, None, None),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, page) = json_response(
            &app.router,
            publisher_request(Method::GET, "/api/v1/triggers", None, None),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["items"], json!([]));
        let (status, _) = json_response(
            &app.router,
            publisher_request(
                Method::POST,
                &format!("{path}/publish"),
                Some(json!({"draft_version": 1})),
                Some("republish-archived"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, preview) = json_response(
            &app.router,
            request(
                Method::GET,
                &format!(
                    "/api/v1/triggers/public/{}",
                    published["public_id"].as_str().unwrap()
                ),
                None,
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(preview["availability"], "retired");
    }
}
