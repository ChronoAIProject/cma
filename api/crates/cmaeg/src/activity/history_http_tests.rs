use super::*;
use crate::{
    activity::history::{self, HistoryPage, HistoryQuery},
    auth::owner_keys::tests::OwnerFixture,
};
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;

#[path = "history_detail_http_tests.rs"]
mod details;

fn enable_history(state: &mut ManagementState) {
    let history = history::ActivityHistory::new(
        Some(Arc::new(history::test_repository::Memory::default())),
        Arc::new(fixture::keys()),
        history::Config {
            enabled: true,
            capture: true,
            ..Default::default()
        },
    );
    state.activity = Arc::new((*state.activity).clone().with_history(Arc::new(history)));
}
fn uri(id: &str) -> String {
    let now = time::OffsetDateTime::now_utc();
    let from = (now - time::Duration::minutes(1))
        .replace_nanosecond(0)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let until = (now + time::Duration::seconds(0))
        .replace_nanosecond(0)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    format!("/v1/channels/{id}/activity/history?from={from}&until={until}&limit=1")
}
#[tokio::test]
async fn history_http_rechecks_permission_revocation_and_channel_deletion_per_page() {
    for refusal in ["permission", "revoked", "deleted"] {
        let (server, mut state, mut request, creator) = fixture_app().await;
        enable_history(&mut state);
        let auth = OwnerFixture::start(&creator.subject).await;
        state.creator_auth = Arc::new(auth.auth.clone());
        for id in ["a", "b"] {
            request.activity.event_id = fixture::event_id(id);
            state
                .activity
                .received_at(
                    &creator.subject,
                    request.channel.definition.record_id.as_str(),
                    &request.activity,
                    time::OffsetDateTime::now_utc() - time::Duration::seconds(2),
                )
                .await
                .unwrap();
        }
        let app = crate::http::management::management_router(state.clone());
        let send = |path: String| {
            let mut r = Request::builder().uri(path).body(Body::empty()).unwrap();
            *r.headers_mut() = auth.headers.clone();
            r
        };
        let first = app
            .clone()
            .oneshot(send(uri(request.channel.id.as_str())))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()["cache-control"], "no-store");
        let page: HistoryPage =
            serde_json::from_slice(&to_bytes(first.into_body(), 1_048_576).await.unwrap()).unwrap();
        let token = page.next_cursor.unwrap();
        match refusal {
            "permission" => auth.claims.lock().unwrap()["permissions"] = serde_json::json!([]),
            "revoked" => auth.claims.lock().unwrap()["active"] = serde_json::json!(false),
            "deleted" => {
                server
                    .memory
                    .channels
                    .lock()
                    .unwrap()
                    .remove(request.channel.definition.record_id.as_str());
            }
            _ => unreachable!(),
        }
        let response = app
            .oneshot(send(format!(
                "/v1/channels/{}/activity/history?cursor={token}",
                request.channel.id
            )))
            .await
            .unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ),
            "{refusal}: {}",
            response.status()
        );
    }
}
#[tokio::test]
async fn history_http_rejects_other_owner_worker_and_anonymous_access() {
    let (_server, mut state, request, _) = fixture_app().await;
    enable_history(&mut state);
    let auth = OwnerFixture::start("another-owner").await;
    state.creator_auth = Arc::new(auth.auth.clone());
    let app = crate::http::management::management_router(state);
    let build = || {
        let mut r = Request::builder()
            .uri(uri(request.channel.id.as_str()))
            .body(Body::empty())
            .unwrap();
        *r.headers_mut() = auth.headers.clone();
        r
    };
    assert_eq!(
        app.clone().oneshot(build()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    use crate::worker_credentials::WorkerHistory;
    auth.history
        .record(&crate::channels::definition::hash_secret(&auth.token))
        .await
        .unwrap();
    assert!(matches!(
        app.clone().oneshot(build()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(
        app.oneshot(
            Request::builder()
                .uri(uri(request.channel.id.as_str()))
                .body(Body::empty())
                .unwrap()
        )
        .await
        .unwrap()
        .status(),
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn history_is_wired_before_launch_and_sse_cursor_is_not_export_cursor() {
    let (_server, mut state, request, creator) = fixture_app().await;
    enable_history(&mut state);
    state.activity.received(&request).await.unwrap();
    let recent = state
        .activity
        .latest(
            &creator.subject,
            request.channel.definition.record_id.as_str(),
        )
        .await
        .unwrap();
    let response = crate::http::activity_history::history(
        State(state.clone()),
        creator,
        Path(request.channel.id.to_string()),
        Ok(Query(HistoryQuery {
            cursor: Some(recent.cursor),
            ..Default::default()
        })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn history_preserves_all_55_admitted_receipts_while_recent_remains_50() {
    let (_server, mut state, mut request, creator) = fixture_app().await;
    enable_history(&mut state);
    let record = request.channel.definition.record_id.as_str();
    let at = time::OffsetDateTime::now_utc() - time::Duration::seconds(2);
    for index in 0..55 {
        request.activity.event_id = fixture::event_id(&format!("event-{index:03}"));
        state
            .activity
            .received_at(&creator.subject, record, &request.activity, at)
            .await
            .unwrap();
    }
    assert_eq!(
        state
            .activity
            .latest(&creator.subject, record)
            .await
            .unwrap()
            .items
            .len(),
        50
    );
    let mut query = HistoryQuery {
        from: Some(
            (at - time::Duration::seconds(1))
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        ),
        until: Some(
            (at + time::Duration::seconds(1))
                .replace_nanosecond(0)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        ),
        limit: Some(10),
        cursor: None,
    };
    let mut identities = std::collections::BTreeSet::new();
    loop {
        let response = crate::http::activity_history::history(
            State(state.clone()),
            creator.clone(),
            Path(request.channel.id.to_string()),
            Ok(Query(query)),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let page: HistoryPage =
            serde_json::from_slice(&to_bytes(response.into_body(), 1_048_576).await.unwrap())
                .unwrap();
        for row in page.items {
            assert!(identities.insert(row.activity.event_id));
        }
        if page.next_cursor.is_none() {
            break;
        }
        query = HistoryQuery {
            cursor: page.next_cursor,
            ..Default::default()
        };
    }
    assert_eq!(identities.len(), 55);
    assert!(identities.contains("event-000") && identities.contains("event-054"));
}
