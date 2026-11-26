use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};

#[tokio::test]
async fn detail_http_reads_chunked_summary_then_rechecks_current_authority() {
    for refusal in ["permission", "revoked", "deleted"] {
        let (server, mut state, request, creator) = fixture_app().await;
        enable_history(&mut state);
        let auth = OwnerFixture::start(&creator.subject).await;
        state.creator_auth = Arc::new(auth.auth.clone());
        let record = request.channel.definition.record_id.as_str();
        state
            .activity
            .received_at(
                &creator.subject,
                record,
                &request.activity,
                time::OffsetDateTime::now_utc() - time::Duration::seconds(2),
            )
            .await
            .unwrap();
        for index in 0..16 {
            let mut change =
                Change::state(&format!("consumer-{index}"), ConsumerState::Completed, None);
            change.display_name = Some("detail".repeat(800));
            state
                .activity
                .history
                .change(
                    &creator.subject,
                    record,
                    request.activity.event_id.as_str(),
                    &change,
                )
                .await
                .unwrap();
        }
        let app = crate::http::management::management_router(state);
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
        let page: HistoryPage =
            serde_json::from_slice(&to_bytes(first.into_body(), 1_048_576).await.unwrap()).unwrap();
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        let path = format!(
            "/v1/channels/{}/activity/history/events/{}/details?cursor={}",
            request.channel.id,
            item.history_event_id,
            item.detail_cursor.as_ref().unwrap()
        );
        let details = app.clone().oneshot(send(path.clone())).await.unwrap();
        assert_eq!(details.status(), StatusCode::OK);
        assert_eq!(details.headers()["cache-control"], "no-store");
        let details: history::HistoryDetails =
            serde_json::from_slice(&to_bytes(details.into_body(), 1_048_576).await.unwrap())
                .unwrap();
        assert!(!details.detail_incomplete);
        let bytes: Vec<_> = details
            .parts
            .iter()
            .flat_map(|part| STANDARD.decode(&part.data).unwrap())
            .collect();
        assert!(bytes.len() > 49_112);
        let summary: ChannelActivity = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(summary.event_id, request.activity.event_id.as_str());
        assert_eq!(summary.consumers.len(), 16);

        match refusal {
            "permission" => auth.claims.lock().unwrap()["permissions"] = serde_json::json!([]),
            "revoked" => auth.claims.lock().unwrap()["active"] = serde_json::json!(false),
            "deleted" => {
                server.memory.channels.lock().unwrap().remove(record);
            }
            _ => unreachable!(),
        }
        let denied = app.oneshot(send(path)).await.unwrap();
        assert!(
            matches!(
                denied.status(),
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
            ),
            "{refusal}: {}",
            denied.status()
        );
    }
}
