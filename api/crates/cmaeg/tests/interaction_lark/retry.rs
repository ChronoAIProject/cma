use super::*;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn lark_and_feishu_cas_exhaustion_retries_without_losing_or_duplicating_the_click() {
    for platform in ["lark", "feishu"] {
        let cards = Cards::new(platform, false).await;
        let mut prepared = cards.prepare().await;
        cards.bind(&mut prepared).await;
        let locator = data(&prepared, 1);
        let click = callback(platform, &locator, "contended-click");

        cards._records.put_conflicts.store(2, Ordering::SeqCst);
        let mut wrong_actor = callback(platform, &locator, "contended-click");
        wrong_actor.payload["sender"]["platform_id"] = "ou_intruder".into();
        wrong_actor.payload["raw_platform_data"]["event"]["operator"]["open_id"] =
            "ou_intruder".into();
        assert_eq!(
            cards.send(&wrong_actor).await.1["reason"],
            "interaction_expired_or_invalid"
        );
        assert_eq!(cards._records.put_conflicts.load(Ordering::SeqCst), 2);

        let (status, failure) = cards.send(&click).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{platform}");
        assert_eq!(failure["detail"], "interaction_store_unavailable");
        assert_eq!(cards._records.put_conflicts.load(Ordering::SeqCst), 0);
        assert!(cards
            .store
            .load(&prepared.card.id)
            .await
            .unwrap()
            .0
            .consumed
            .is_none());
        assert!(cards
            .app
            .fixture
            .recorder
            .requests
            .lock()
            .unwrap()
            .is_empty());

        // Retry still checks current source policy before consuming the card.
        cards.groups("deny").await;
        assert_eq!(cards.send(&click).await.1["reason"], "group_denied");
        cards.groups("mention").await;
        cards._records.put_conflicts.store(1, Ordering::SeqCst);
        let accepted = cards.send(&click).await;
        assert_eq!(accepted.0, StatusCode::ACCEPTED);
        assert_eq!(accepted.1["status"], "accepted");
        assert_eq!(cards._records.put_conflicts.load(Ordering::SeqCst), 0);
        let (consumed, version) = cards.store.load(&prepared.card.id).await.unwrap();
        assert!(consumed.consumed.is_some());

        // A later relay timestamp cannot change the retained logical event.
        let mut replay = callback(platform, &locator, "contended-click");
        replay.payload["timestamp"] = "2026-10-01T00:00:00Z".into();
        assert_eq!(cards.send(&replay).await, accepted);
        let (replayed, replay_version) = cards.store.load(&prepared.card.id).await.unwrap();
        assert_eq!(replay_version, version);
        assert_eq!(
            serde_json::to_value(replayed.consumed).unwrap(),
            serde_json::to_value(consumed.consumed).unwrap()
        );
        {
            let requests = cards.app.fixture.recorder.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].activity, requests[1].activity);
            assert_eq!(requests[0].idempotency_key, requests[1].idempotency_key);
            assert_eq!(requests[0].conversation_id, requests[1].conversation_id);
            let ActivityKind::Interaction(result) = &requests[0].activity.kind else {
                panic!("expected the retained interaction");
            };
            assert_eq!(result.action_id, "b");
            assert_eq!(result.values["selected"], json!(["a", "b"]));
            assert_eq!(result.values["revision"], 1);
        }

        let other = callback(platform, &locator, "different-click");
        assert_eq!(
            cards.send(&other).await.1["reason"],
            "interaction_expired_or_invalid"
        );
        assert_eq!(cards.app.fixture.recorder.requests.lock().unwrap().len(), 2);
        assert_eq!(
            cards.store.load(&prepared.card.id).await.unwrap().1,
            version
        );
    }
}
