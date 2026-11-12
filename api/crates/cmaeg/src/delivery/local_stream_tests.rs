use super::*;
use crate::delivery::sync::{SyncRegistry, Waiter};
use axum::response::{IntoResponse, Sse};
use std::sync::Arc;

fn fixture(opt_in: bool) -> (CustomDelivery, EventRefPayload, Waiter) {
    let registry = Arc::new(SyncRegistry::default());
    let (id, waiter) = if opt_in {
        registry.register_with_media_responses(true)
    } else {
        registry.register()
    }
    .unwrap();
    let event = EventRefPayload {
        telegram_admission: None,
        turn_trace: None,
        v: 1,
        activity_sha256: None,
        channel_id: "cmaeg1.ch.stream-fixture".parse().unwrap(),
        record_id: "record-fixture".parse().unwrap(),
        event_id: "event-fixture".parse().unwrap(),
        partition_key: "partition-fixture".into(),
        conversation_id: "conversation-fixture".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::Sync { waiter: id },
        consumer: None,
        lineage: None,
    };
    (
        CustomDelivery::new("https://gateway.example".into(), registry),
        event,
        waiter,
    )
}

async fn frames(waiter: Waiter) -> Vec<(String, Value)> {
    let response = Sse::new(waiter.stream()).into_response();
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        axum::body::to_bytes(response.into_body(), 128 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .split("\n\n")
        .filter(|frame| !frame.is_empty())
        .map(|frame| {
            let name = frame
                .lines()
                .find_map(|v| v.strip_prefix("event: "))
                .unwrap();
            let data = frame
                .lines()
                .find_map(|v| v.strip_prefix("data: "))
                .unwrap();
            (name.into(), serde_json::from_str(data).unwrap())
        })
        .collect()
}

#[tokio::test]
async fn media_enabled_legacy_waiter_preserves_exact_success_and_failure_terminal_frames() {
    for effect in [
        CustomStreamEffect::Completed {
            final_text: Some("Original answer".into()),
        },
        CustomStreamEffect::Failed,
    ] {
        let (custom, event, waiter) = fixture(false);
        assert_eq!(
            custom.local_media_response(&event, &json!({"text":"early media"})),
            LocalStreamOutcome::Unavailable
        );
        assert_eq!(
            custom.local_response(&event, &effect, "response-original"),
            LocalStreamOutcome::Buffered
        );
        assert_eq!(
            custom.finish_media(&event, false),
            LocalStreamOutcome::Unavailable
        );
        let (name, data) = effect.frame(&event);
        assert_eq!(frames(waiter).await, vec![(name.into(), data)]);
        assert!(custom.waiters.is_empty());
    }
}

#[tokio::test]
async fn opted_in_media_stream_preserves_original_final_fields_and_waits_for_media() {
    let (custom, event, waiter) = fixture(true);
    let effect = CustomStreamEffect::Completed {
        final_text: Some("Original answer".into()),
    };
    custom.local_response(&event, &effect, "response-original");
    assert!(!custom.waiters.is_empty());
    let media = json!({"response_id":"response-media","text":"Attachment answer"});
    assert_eq!(
        custom.local_media_response(&event, &media),
        LocalStreamOutcome::Buffered
    );
    assert_eq!(
        custom.finish_media(&event, false),
        LocalStreamOutcome::Buffered
    );
    let actual = frames(waiter).await;
    assert_eq!(
        actual
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [
            "cmaeg.response.completed",
            "cmaeg.media.response",
            "cmaeg.completed"
        ]
    );
    assert_eq!(actual[0].1["response_id"], "response-original");
    assert_eq!(actual[0].1["media_pending"], true);
    assert_eq!(actual[1].1, media);
    let (_, mut expected) = effect.frame(&event);
    expected["media_complete"] = true.into();
    expected["status"] = "completed".into();
    assert_eq!(actual[2].1, expected);
}

#[tokio::test]
async fn opted_in_original_failure_cannot_become_completed_when_media_succeeds() {
    let (custom, event, waiter) = fixture(true);
    let effect = CustomStreamEffect::Failed;
    custom.local_response(&event, &effect, "response-original");
    custom.finish_media(&event, false);
    let actual = frames(waiter).await;
    assert_eq!(actual[0].0, "cmaeg.response.failed");
    assert_eq!(actual[1].0, "cmaeg.failed");
    let (_, original) = effect.frame(&event);
    assert_eq!(actual[1].1["error"], original["error"]);
    assert!(actual[1].1.get("text").is_some());
    assert!(actual[1].1.get("message").is_some());
    assert_eq!(actual[1].1["media_complete"], false);
}

#[tokio::test(start_paused = true)]
async fn media_wait_deadline_starts_after_original_terminal_not_its_first_delta() {
    use futures_util::StreamExt;
    use tokio::time::{advance, Duration, Instant};
    let (custom, event, waiter) = fixture(true);
    let mut stream = Box::pin(waiter.stream());
    custom.local_response(
        &event,
        &CustomStreamEffect::Delta {
            text: "first".into(),
        },
        "original",
    );
    assert!(stream.next().await.is_some());
    advance(Duration::from_secs(180)).await;
    custom.local_response(
        &event,
        &CustomStreamEffect::Completed {
            final_text: Some("done".into()),
        },
        "original",
    );
    assert!(stream.next().await.is_some());
    let after_original = Instant::now();
    let response = Sse::new(stream).into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(after_original.elapsed(), Duration::from_secs(120));
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("event: cmaeg.media.pending"), "{text}");
}
